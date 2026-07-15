import assert from "node:assert/strict";
import test from "node:test";

import PhuxPlugin, {
  DEFAULT_SHORT_TIMEOUT_MS,
  MAX_MODEL_BYTES,
  MAX_MODEL_LINES,
  PhuxCli,
  PhuxPlugin as NamedPhuxPlugin,
} from "../dist/index.js";

const screen = {
  schema_version: 1,
  pane: 7,
  cols: 80,
  rows: 2,
  cursor: null,
  lines: ["hello", "prompt"],
  scrollback: ["older"],
};

function context(sessionID = "public-session", signal = new AbortController().signal) {
  return {
    sessionID,
    messageID: "message",
    agent: "build",
    directory: "/project",
    worktree: "/project",
    abort: signal,
    metadata() {},
    async ask() {},
  };
}

function completed(stdout = "", exitCode = 0) {
  return { termination: "completed", exitCode, stdout, stderr: "" };
}

test("public plugin loads six tools without invoking phux", async () => {
  let calls = 0;
  const cli = new PhuxCli({ runner: async () => {
    calls += 1;
    return completed();
  } });

  assert.equal(PhuxPlugin, NamedPhuxPlugin);
  const hooks = await PhuxPlugin({}, { cli, env: {} });

  assert.deepEqual(Object.keys(hooks.tool).sort(), [
    "phux_create",
    "phux_list",
    "phux_run",
    "phux_send_keys",
    "phux_snapshot",
    "phux_wait",
  ]);
  assert.equal(typeof hooks.event, "function");
  assert.equal(typeof hooks.dispose, "function");
  assert.equal(calls, 0);
  await hooks.dispose();
  assert.equal(calls, 0);
});

test("tools preserve target precedence, command shape, deadlines, cancellation, and bounded results", async () => {
  const requests = [];
  const cli = new PhuxCli({
    executable: "/opt/bin/phux",
    socket: "/tmp/phux.sock",
    runner: async (request) => {
      requests.push(request);
      switch (request.args[0]) {
        case "ls":
          return completed(JSON.stringify({ schema_version: 1, sessions: [{ name: "shared", windows: 1, attached: false }] }));
        case "new":
          return completed(JSON.stringify({ session: "made", terminal_id: 44 }));
        case "snapshot":
          return completed(JSON.stringify(screen));
        case "send-keys":
          return completed();
        case "run":
          return completed(JSON.stringify({ command: request.args.at(-1), exit_code: 0, output: "x".repeat(50_000), duration_ms: 12, truncated: false }));
        case "wait":
          return completed(JSON.stringify(screen));
        case "agent":
          if (request.args[1] === "set") {
            const record = {
              name: request.args[request.args.indexOf("--name") + 1],
              kind: request.args[request.args.indexOf("--kind") + 1],
              state: request.args[request.args.indexOf("--state") + 1],
              attention: request.args[request.args.indexOf("--attention") + 1],
              session: request.args[request.args.indexOf("--session") + 1],
            };
            return completed(`@44\t${JSON.stringify(record)}`);
          }
          return completed(JSON.stringify({ schema_version: 1, agents: [] }));
        default:
          throw new Error(`unexpected args: ${request.args.join(" ")}`);
      }
    },
  });
  const hooks = await PhuxPlugin({}, { cli, env: { PHUX_TARGET: "@9" } });
  const tools = hooks.tool;
  const toolContext = context();

  const listed = await tools.phux_list.execute({}, toolContext);
  assert.equal(listed.metadata.count, 1);

  await tools.phux_snapshot.execute({}, toolContext);
  await tools.phux_snapshot.execute({ target: "@10" }, toolContext);
  await tools.phux_create.execute({ name: "made" }, toolContext);
  await tools.phux_send_keys.execute({ keys: ["C-c", "literal"] }, toolContext);
  const run = await tools.phux_run.execute({ command: "printf '%s' one two" }, toolContext);
  const waited = await tools.phux_wait.execute({}, toolContext);

  const snapshots = requests.filter((request) => request.args[0] === "snapshot");
  assert.equal(snapshots[0].args.at(-1), "@9", "PHUX_TARGET is the initial fallback");
  assert.equal(snapshots[1].args.at(-1), "@10", "an explicit target wins");
  const send = requests.find((request) => request.args[0] === "send-keys");
  assert.deepEqual(send.args.slice(-3), ["@44", "C-c", "literal"], "create auto-selects its seed pane");

  const runRequest = requests.find((request) => request.args[0] === "run");
  assert.equal(runRequest.args.at(-2), "@44");
  assert.equal(runRequest.args.at(-1), "printf '%s' one two", "run passes one command string argument");
  assert.equal(runRequest.timeoutMs, undefined, "long operations are not given an implicit local deadline");
  assert.equal(runRequest.signal, toolContext.abort);
  assert.equal(Buffer.byteLength(run.output), MAX_MODEL_BYTES);
  assert.ok(run.output.split("\n").length <= MAX_MODEL_LINES);
  assert.match(run.output, /OpenCode adapter truncated terminal output/);
  assert.equal(run.metadata.modelOutputTruncated, true);

  const waitRequest = requests.find((request) => request.args[0] === "wait");
  assert.equal(waitRequest.args.includes("--until"), false);
  assert.equal(waitRequest.args.includes("--idle"), false);
  assert.equal(waitRequest.args.includes("--timeout"), false);
  assert.equal(waitRequest.timeoutMs, undefined, "omitted wait deadlines mean indefinite");
  assert.equal(waited.metadata.outcome, "satisfied");

  const snapshotRequest = snapshots[0];
  assert.equal(snapshotRequest.timeoutMs, DEFAULT_SHORT_TIMEOUT_MS);
  assert.equal(snapshotRequest.signal, toolContext.abort);
  await hooks.dispose();
});

test("public argument contracts reject invalid forms and wait enforces exclusive conditions", async () => {
  const cli = new PhuxCli({ runner: async () => completed() });
  const hooks = await PhuxPlugin({}, { cli, env: {} });
  const z = (await import("@opencode-ai/plugin")).tool.schema;

  const runSchema = z.object(hooks.tool.phux_run.args).strict();
  assert.equal(runSchema.safeParse({ command: ["echo", "no"] }).success, false);
  assert.equal(runSchema.safeParse({ command: "echo yes", surprise: true }).success, false);
  assert.equal(runSchema.safeParse({ command: "   " }).success, false);

  const waitSchema = z.object(hooks.tool.phux_wait.args).strict();
  assert.equal(waitSchema.safeParse({ idle_ms: -1 }).success, false);
  assert.equal(waitSchema.safeParse({ timeout_seconds: 0 }).success, false);
  await assert.rejects(
    hooks.tool.phux_wait.execute({ target: "@1", until: "done", idle_ms: 10 }, context()),
    /either until or idle_ms/,
  );
  await assert.rejects(
    hooks.tool.phux_snapshot.execute({}, context()),
    /Pass target explicitly.*PHUX_TARGET/,
  );
  await hooks.dispose();
});

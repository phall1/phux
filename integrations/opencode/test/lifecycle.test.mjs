import assert from "node:assert/strict";
import test from "node:test";

import PhuxPlugin, { PhuxCli } from "../dist/index.js";

function completed(stdout = "", exitCode = 0) {
  return { termination: "completed", exitCode, stdout, stderr: "" };
}

function option(args, name) {
  return args[args.indexOf(name) + 1];
}

test("documented session status events publish honest owner-labelled working and idle states", async () => {
  const requests = [];
  let record;
  const cli = new PhuxCli({ runner: async (request) => {
    requests.push(request);
    if (request.args[0] === "agent" && request.args[1] === "show") {
      return completed(JSON.stringify({ schema_version: 1, agents: [] }));
    }
    if (request.args[0] !== "agent" || request.args[1] !== "set") {
      throw new Error(`unexpected lifecycle request: ${request.args.join(" ")}`);
    }
    record = {
      name: option(request.args, "--name"),
      kind: option(request.args, "--kind"),
      state: option(request.args, "--state"),
      attention: option(request.args, "--attention"),
      session: option(request.args, "--session"),
    };
    return completed(`@5\t${JSON.stringify(record)}`);
  } });
  const hooks = await PhuxPlugin({}, { cli, env: { PHUX_TARGET: "@5" }, lifecycleTimeoutMs: 321 });

  await hooks.event({ event: {
    type: "session.status",
    properties: { sessionID: "session-public-1", status: { type: "busy" } },
  } });
  assert.deepEqual(record, {
    name: "opencode",
    kind: "opencode",
    state: "working",
    attention: "normal",
    session: "opencode:session-public-1",
  });

  await hooks.event({ event: {
    type: "session.idle",
    properties: { sessionID: "session-public-1" },
  } });
  assert.equal(record.state, "idle");
  assert.equal(record.attention, "low");
  assert.equal(requests.every((request) => request.timeoutMs === 321), true);
  assert.equal(requests.every((request) => request.signal instanceof AbortSignal), true);
  await hooks.dispose();
});

test("session deletion clears only a declaration still owned by that public session", async () => {
  const requests = [];
  let record;
  let exposeOwner = true;
  const cli = new PhuxCli({ runner: async (request) => {
    requests.push(request);
    if (request.args[0] !== "agent") throw new Error("expected agent command");
    if (request.args[1] === "set") {
      record = {
        name: option(request.args, "--name"),
        kind: option(request.args, "--kind"),
        state: option(request.args, "--state"),
        attention: option(request.args, "--attention"),
        session: option(request.args, "--session"),
      };
      return completed(`@6\t${JSON.stringify(record)}`);
    }
    if (request.args[1] === "show") {
      const observed = exposeOwner ? record : { ...record, session: "opencode:someone-else" };
      return completed(JSON.stringify({
        schema_version: 1,
        agents: [{
          terminal: "@6",
          session: "shared",
          window: "window-0",
          agent: { id: "declared", label: "opencode", kind: "declared" },
          state: "idle",
          confidence: 1,
          attention: "low",
          title: null,
          cwd: null,
          sources: [{ kind: "agent_record", signal: "declared", confidence: 1, observed: JSON.stringify(observed) }],
          explanation: "declared record",
        }],
      }));
    }
    if (request.args[1] === "clear") return completed("@6\t-");
    throw new Error(`unexpected agent command: ${request.args.join(" ")}`);
  } });
  const hooks = await PhuxPlugin({}, { cli, env: { PHUX_TARGET: "@6" } });

  await hooks.event({ event: {
    type: "session.status",
    properties: { sessionID: "owned", status: { type: "busy" } },
  } });
  await hooks.event({ event: {
    type: "session.deleted",
    properties: { info: { id: "owned" } },
  } });
  assert.equal(requests.some((request) => request.args[1] === "show"), true);
  assert.equal(requests.some((request) => request.args[1] === "clear"), true);

  requests.length = 0;
  exposeOwner = false;
  await hooks.event({ event: {
    type: "session.status",
    properties: { sessionID: "not-owner", status: { type: "busy" } },
  } });
  await hooks.dispose();
  assert.equal(requests.some((request) => request.args[1] === "show"), true);
  assert.equal(requests.some((request) => request.args[1] === "clear"), false, "dispose preserves a replacement owner's declaration");
});

test("retry and unrelated public events do not invent lifecycle transitions", async () => {
  let calls = 0;
  const cli = new PhuxCli({ runner: async () => {
    calls += 1;
    return completed();
  } });
  const hooks = await PhuxPlugin({}, { cli, env: { PHUX_TARGET: "@8" } });

  await hooks.event({ event: {
    type: "session.status",
    properties: { sessionID: "retrying", status: { type: "retry", attempt: 1, message: "later", next: 1 } },
  } });
  await hooks.event({ event: { type: "file.edited", properties: { file: "x" } } });
  await hooks.dispose();
  assert.equal(calls, 0);
});

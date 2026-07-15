import assert from "node:assert/strict";
import test from "node:test";

import { PhuxCli } from "../src/adapter.js";
import { PhuxError } from "../src/errors.js";
import type { ProcessResult, ProcessRunner, RunRequest } from "../src/runner.js";

const completed = (stdout: string, exitCode = 0, stderr = ""): ProcessResult => ({
  termination: "completed",
  exitCode,
  stdout,
  stderr,
});

function fakeRunner(result: ProcessResult): { runner: ProcessRunner; requests: RunRequest[] } {
  const requests: RunRequest[] = [];
  return {
    requests,
    runner: async (request) => {
      requests.push(request);
      return result;
    },
  };
}

function expectCode(code: PhuxError["code"]): (error: unknown) => boolean {
  return (error) => error instanceof PhuxError && error.code === code;
}

const screenJson = JSON.stringify({
  schema_version: 3,
  pane: 1,
  cols: 80,
  rows: 1,
  cursor: { x: 0, y: 0, visible: true },
  lines: ["ready"],
  scrollback: [],
});

test("ls parses the documented CLI shape and passes flags before positionals", async () => {
  const fake = fakeRunner(completed(JSON.stringify({
    schema_version: 1,
    sessions: [{ name: "work", windows: 2, attached: false }],
  })));
  const cli = new PhuxCli({ runner: fake.runner, executable: "/opt/bin/phux", socket: "/tmp/p.sock" });

  const result = await cli.ls();

  assert.deepEqual(result.sessions, [{ name: "work", windows: 2, attached: false }]);
  assert.deepEqual(fake.requests[0]?.args, ["ls", "--json", "--socket", "/tmp/p.sock"]);
});

test("agentList inventories canonical panes and preserves owning sessions", async () => {
  const fake = fakeRunner(completed(JSON.stringify({
    schema_version: 1,
    agents: [{
      terminal: "@3",
      session: "work",
      window: "window-0",
      agent: { id: "codex", label: "Codex", kind: "codex" },
      state: "working",
      confidence: 0.9,
      attention: "normal",
      title: null,
      cwd: "/repo",
      sources: [],
      explanation: "working cue",
    }],
  })));
  const cli = new PhuxCli({ runner: fake.runner, socket: "/tmp/p.sock" });

  const result = await cli.agentList();

  assert.equal(result.agents[0]?.terminal, "@3");
  assert.equal(result.agents[0]?.session, "work");
  assert.deepEqual(fake.requests[0]?.args, ["agent", "list", "--json", "--socket", "/tmp/p.sock"]);
});

test("wait preserves the final screen for exit 0 and specialized exit 124", async () => {
  const satisfied = new PhuxCli({ runner: fakeRunner(completed(screenJson, 0)).runner });
  assert.deepEqual(await satisfied.wait(), {
    outcome: "satisfied",
    screen: {
      schema_version: 3,
      pane: 1,
      cols: 80,
      rows: 1,
      cursor: { x: 0, y: 0, visible: true },
      lines: ["ready"],
      scrollback: [],
    },
  });

  const timedOut = new PhuxCli({ runner: fakeRunner(completed(screenJson, 124)).runner });
  const outcome = await timedOut.wait();
  assert.equal(outcome.outcome, "timed_out");
  assert.deepEqual(outcome.screen.lines, ["ready"]);
});

test("wait keeps unrelated nonzero exits as command failures", async () => {
  const cli = new PhuxCli({ runner: fakeRunner(completed("", 1, "no server")).runner });
  await assert.rejects(cli.wait(), expectCode("command_failed"));
});

test("run treats a documented nonzero child exit as typed data", async () => {
  const fake = fakeRunner(completed(JSON.stringify({
    command: "false",
    exit_code: 7,
    output: "failure",
    duration_ms: 12,
    truncated: false,
  }), 7));
  const cli = new PhuxCli({ runner: fake.runner, socket: "/tmp/p.sock" });

  const result = await cli.run("work", ["false"], { phuxTimeoutSeconds: 30 });

  assert.equal(result.exit_code, 7);
  assert.deepEqual(fake.requests[0]?.args, [
    "run", "--json", "--timeout", "30", "--socket", "/tmp/p.sock", "work", "false",
  ]);
});

test("malformed JSON is normalized", async () => {
  const cli = new PhuxCli({ runner: fakeRunner(completed("not-json")).runner });
  await assert.rejects(cli.ls(), expectCode("malformed_json"));
});

test("run wrapper failures without JSON stay command failures", async () => {
  const cli = new PhuxCli({ runner: fakeRunner(completed("", 125, "sentinel timed out")).runner });
  await assert.rejects(cli.run("work", ["sleep", "10"]), expectCode("command_failed"));
});

test("ordinary nonzero exits include the phux diagnostic", async () => {
  const cli = new PhuxCli({ runner: fakeRunner(completed("", 1, "no server running")).runner });
  await assert.rejects(
    cli.ls(),
    (error) => error instanceof PhuxError &&
      error.code === "command_failed" &&
      error.exitCode === 1 &&
      error.message.includes("no server running"),
  );
});

test("abort and local timeout are distinct normalized errors", async () => {
  const aborted = new PhuxCli({
    runner: fakeRunner({ termination: "aborted", exitCode: null, stdout: "", stderr: "" }).runner,
  });
  const timedOut = new PhuxCli({
    runner: fakeRunner({ termination: "timed_out", exitCode: null, stdout: "", stderr: "" }).runner,
  });

  await assert.rejects(aborted.ls(), expectCode("aborted"));
  await assert.rejects(timedOut.ls(), expectCode("timeout"));
});

test("output overflow is exposed as a typed actionable error", async () => {
  const overflow: ProcessResult = {
    termination: "output_limit",
    outputLimit: "stdout",
    exitCode: null,
    stdout: "partial",
    stderr: "",
  };
  const cli = new PhuxCli({ runner: fakeRunner(overflow).runner, maxStdoutBytes: 7 });
  await assert.rejects(
    cli.ls(),
    (error) => error instanceof PhuxError &&
      error.code === "output_limit" &&
      error.message.includes("7-byte stdout"),
  );
});

test("CLI parsers reject similar MCP response shapes", async () => {
  const mcpLs = new PhuxCli({ runner: fakeRunner(completed(JSON.stringify({
    schema_version: 1,
    sessions: [{ name: "work", window_count: 2, attached_client_count: 0 }],
  }))).runner });
  await assert.rejects(mcpLs.ls(), expectCode("invalid_response"));

  const mcpRun = new PhuxCli({ runner: fakeRunner(completed(JSON.stringify({
    outcome: "timed_out",
    command: "sleep 10",
    duration_ms: 1_000,
  }), 125)).runner });
  await assert.rejects(mcpRun.run("work", ["sleep", "10"]), expectCode("invalid_response"));
});

test("probe reports a validated version and missing executables without throwing", async () => {
  const present = new PhuxCli({ runner: fakeRunner(completed("phux 0.1.0\n")).runner });
  assert.deepEqual(await present.probe(), {
    available: true,
    version: "0.1.0",
    rawVersion: "phux 0.1.0",
  });

  const missing: ProcessRunner = async () => {
    const error = Object.assign(new Error("spawn phux ENOENT"), { code: "ENOENT" });
    throw error;
  };
  const result = await new PhuxCli({ runner: missing }).probe();
  assert.equal(result.available, false);
  assert.match(result.reason ?? "", /install phux/);
});

import assert from "node:assert/strict";
import test from "node:test";

import { nodeProcessRunner } from "../src/runner.js";

test("node runner passes metacharacters as literal argv without a shell", async () => {
  const literal = "$(printf injected); *; echo nope";
  const result = await nodeProcessRunner({
    executable: process.execPath,
    args: ["-e", "process.stdout.write(process.argv[1])", literal],
  });

  assert.equal(result.termination, "completed");
  assert.equal(result.exitCode, 0);
  assert.equal(result.stdout, literal);
});

test("node runner propagates AbortSignal", async () => {
  const controller = new AbortController();
  const pending = nodeProcessRunner({
    executable: process.execPath,
    args: ["-e", "setInterval(() => {}, 1000)"],
    signal: controller.signal,
  });
  setTimeout(() => controller.abort(), 20);
  const result = await pending;
  assert.equal(result.termination, "aborted");
});

test("node runner enforces timeout", async () => {
  const result = await nodeProcessRunner({
    executable: process.execPath,
    args: ["-e", "setInterval(() => {}, 1000)"],
    timeoutMs: 20,
  });
  assert.equal(result.termination, "timed_out");
});

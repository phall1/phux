import assert from "node:assert/strict";
import test from "node:test";

import type { AgentToolResult, ExtensionAPI, Theme } from "@earendil-works/pi-coding-agent";

import { PhuxCli } from "../src/adapter.js";
import type { ProcessResult, RunRequest } from "../src/runner.js";
import { PhuxTargetStore } from "../src/target-store.js";
import {
  PhuxCreateParams,
  PhuxListParams,
  PhuxRunParams,
  PhuxSendKeysParams,
  PhuxSnapshotParams,
  PhuxWaitParams,
  registerPhuxTools,
  resolveTarget,
  type PhuxToolDetails,
} from "../src/tools.js";

interface CapturedTool {
  readonly name: string;
  readonly parameters: unknown;
  execute(id: string, params: Record<string, unknown>, signal?: AbortSignal): Promise<AgentToolResult<PhuxToolDetails>>;
  renderCall?: (args: Record<string, unknown>, theme: Theme, context: unknown) => { render(width: number): string[] };
  renderResult?: (result: AgentToolResult<PhuxToolDetails>, options: unknown, theme: Theme, context: unknown) => { render(width: number): string[] };
}

interface ObjectSchema {
  readonly additionalProperties?: boolean;
  readonly properties?: Record<string, { readonly minItems?: number }>;
}

const completed = (stdout: string, exitCode = 0): ProcessResult => ({
  termination: "completed",
  exitCode,
  stdout,
  stderr: "",
});

function fixture(): {
  readonly tools: Map<string, CapturedTool>;
  readonly requests: RunRequest[];
  readonly store: PhuxTargetStore;
} {
  const tools = new Map<string, CapturedTool>();
  const requests: RunRequest[] = [];
  const cli = new PhuxCli({
    socket: "/tmp/phux.sock",
    runner: async (request) => {
      requests.push(request);
      switch (request.args[0]) {
        case "new": return completed(JSON.stringify({ session: "fresh", terminal_id: 9 }));
        case "snapshot": return completed(JSON.stringify({
          schema_version: 3,
          pane: 9,
          cols: 80,
          rows: 1,
          cursor: null,
          lines: ["ready"],
          scrollback: [],
        }));
        case "send-keys": return completed("");
        case "run": return completed(JSON.stringify({
          command: "echo ok", exit_code: 0, output: "ok", duration_ms: 4, truncated: false,
        }));
        case "wait": return completed(JSON.stringify({
          schema_version: 3,
          pane: 9,
          cols: 80,
          rows: 1,
          cursor: null,
          lines: ["done"],
          scrollback: [],
        }));
        case "ls": return completed(JSON.stringify({ schema_version: 1, sessions: [] }));
        default: throw new Error(`unexpected argv ${JSON.stringify(request.args)}`);
      }
    },
  });
  const store = new PhuxTargetStore({ appendEntry: () => {} }, cli);
  const api = {
    registerTool: (definition: unknown) => {
      const tool = definition as CapturedTool;
      tools.set(tool.name, tool);
    },
  } as unknown as ExtensionAPI;
  registerPhuxTools(api, cli, store);
  return { tools, requests, store };
}

function tool(tools: Map<string, CapturedTool>, name: string): CapturedTool {
  const found = tools.get(name);
  assert.notEqual(found, undefined);
  return found as CapturedTool;
}

const theme = {
  fg: (_color: string, value: string) => value,
  bold: (value: string) => value,
} as unknown as Theme;

test("all tool schemas are strict and argv arrays are non-empty", () => {
  for (const schema of [
    PhuxListParams, PhuxCreateParams, PhuxSnapshotParams,
    PhuxSendKeysParams, PhuxRunParams, PhuxWaitParams,
  ]) {
    assert.equal((schema as ObjectSchema).additionalProperties, false);
  }
  assert.equal((PhuxCreateParams as ObjectSchema).properties?.command?.minItems, 1);
  assert.equal((PhuxSendKeysParams as ObjectSchema).properties?.keys?.minItems, 1);
  assert.equal((PhuxRunParams as ObjectSchema).properties?.command?.minItems, 1);
});

test("create uses new --json, selects the @id, and records reconstruction details", async () => {
  const { tools, requests, store } = fixture();
  const result = await tool(tools, "phux_create").execute("1", {
    name: "fresh", cwd: "/repo", command: ["bash", "-lc", "echo ok"],
  });

  assert.deepEqual(requests[0]?.args, [
    "new", "--json", "-s", "fresh", "--cwd", "/repo", "--socket", "/tmp/phux.sock",
    "--", "bash", "-lc", "echo ok",
  ]);
  assert.equal(store.snapshot.selection?.selector, "@9");
  assert.deepEqual(result.details?.selection, store.snapshot.selection);
  assert.equal(result.details?.target, "@9");
});

test("targeted tools reject an unavailable implicit target and never fall back to phux focus", async () => {
  const { tools, requests, store } = fixture();
  assert.throws(() => resolveTarget(undefined, store), /No phux target is selected/);
  await assert.rejects(tool(tools, "phux_snapshot").execute("2", {}), /Pass target explicitly/);
  assert.equal(requests.length, 0);

  const result = await tool(tools, "phux_snapshot").execute("3", { target: "@44", scrollback: 20 });
  assert.deepEqual(requests[0]?.args, [
    "snapshot", "--json", "--scrollback", "20", "--socket", "/tmp/phux.sock", "@44",
  ]);
  assert.match(result.content[0]?.type === "text" ? result.content[0].text : "", /ready/);
  assert.equal(result.details?.target, "@44");
});

test("send, run, and wait map documented argv and propagate cancellation/timeouts", async () => {
  const { tools, requests } = fixture();
  const controller = new AbortController();

  await tool(tools, "phux_send_keys").execute("4", {
    target: "@9", keys: ["C-c", "Enter"], local_timeout_ms: 500,
  }, controller.signal);
  await tool(tools, "phux_run").execute("5", {
    target: "@9", command: ["echo", "ok"], timeout_seconds: 30,
  }, controller.signal);
  const waited = await tool(tools, "phux_wait").execute("6", {
    target: "@9", until: "done", idle_ms: 100, timeout_seconds: 2,
  }, controller.signal);

  assert.deepEqual(requests[0]?.args, ["send-keys", "--socket", "/tmp/phux.sock", "@9", "C-c", "Enter"]);
  assert.equal(requests[0]?.timeoutMs, 500);
  assert.equal(requests[0]?.signal, controller.signal);
  assert.deepEqual(requests[1]?.args, [
    "run", "--json", "--timeout", "30", "--socket", "/tmp/phux.sock", "@9", "echo", "ok",
  ]);
  assert.deepEqual(requests[2]?.args, [
    "wait", "--json", "--until", "done", "--idle", "100", "--timeout", "2",
    "--socket", "/tmp/phux.sock", "@9",
  ]);
  assert.equal(waited.details?.outcome, "satisfied");
});

test("custom renderers show compact call and result summaries", async () => {
  const { tools } = fixture();
  const definition = tool(tools, "phux_create");
  const result = await definition.execute("7", { name: "fresh" });

  assert.match(definition.renderCall?.({ name: "fresh" }, theme, {}).render(80).join("\n") ?? "", /phux create fresh/);
  assert.match(definition.renderResult?.(result, {}, theme, {}).render(80).join("\n") ?? "", /created fresh:window-0 @9/);
  assert.doesNotMatch(definition.renderResult?.(result, {}, theme, {}).render(80).join("\n") ?? "", /terminal output/);
});

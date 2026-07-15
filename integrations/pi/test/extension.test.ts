import assert from "node:assert/strict";
import test from "node:test";

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

import { PhuxCli } from "../src/adapter.js";
import {
  formatAttachHandoff,
  formatDetailedStatus,
  registerPhuxExtension,
} from "../src/extension.js";
import type { PhuxTargetSnapshot } from "../src/target-store.js";

const selected: PhuxTargetSnapshot = {
  selection: {
    version: 1,
    selector: "@3",
    session: "work space;$(bad)",
    window: "window-0",
    display: "work space:window-0 @3 - Codex",
  },
  availability: "stale",
  reason: "pane @3 is no longer present",
};

test("attach handoff is an argv presentation only and retains pane navigation", () => {
  const message = formatAttachHandoff(selected);

  assert.match(message, /\["phux","attach","work space;\$\(bad\)"\]/);
  assert.match(message, /navigate in phux to pane @3/);
  assert.match(message, /does not execute attach/);
  assert.match(message, /without fallback/);
  assert.doesNotMatch(message, /token|cwd|title/i);
});

test("detailed status exposes stale state instead of choosing another pane", () => {
  assert.equal(
    formatDetailedStatus(selected),
    "phux: work space:window-0 @3 - Codex (stale)\npane @3 is no longer present",
  );
});

test("registers Pi-native commands and tolerates custom UI being unavailable", async () => {
  const commands = new Map<string, (args: string, ctx: ExtensionContext) => Promise<void>>();
  const events: string[] = [];
  const tools: string[] = [];
  let appended = 0;
  const api = {
    appendEntry: () => { appended++; },
    on: (name: string) => { events.push(name); },
    registerTool: (tool: { name: string }) => { tools.push(tool.name); },
    registerCommand: (name: string, options: { handler: (args: string, ctx: ExtensionContext) => Promise<void> }) => {
      commands.set(name, options.handler);
    },
  } as unknown as ExtensionAPI;
  const cli = new PhuxCli({
    runner: async () => ({
      termination: "completed",
      exitCode: 0,
      stderr: "",
      stdout: JSON.stringify({
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
      }),
    }),
  });
  registerPhuxExtension(api, { cli });

  assert.deepEqual([...commands.keys()], ["phux", "phux-status", "phux-attach"]);
  assert.deepEqual(tools, [
    "phux_list", "phux_create", "phux_snapshot", "phux_send_keys", "phux_run", "phux_wait",
  ]);
  assert.deepEqual(events, ["session_start", "session_tree"]);

  let customCalls = 0;
  const ctx = {
    hasUI: true,
    signal: undefined,
    ui: {
      custom: async () => { customCalls++; return undefined; },
      setStatus: () => {},
      notify: () => {},
    },
  } as unknown as ExtensionContext;
  await commands.get("phux")?.("", ctx);
  assert.equal(customCalls, 1);
  assert.equal(appended, 0);
});

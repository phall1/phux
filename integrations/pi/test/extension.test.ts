import assert from "node:assert/strict";
import test from "node:test";

import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";

import {
  formatAttachHandoff,
  formatDetailedStatus,
  isInteractiveContext,
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

test("TUI guard honors mode when supplied and falls back to Pi hasUI", () => {
  assert.equal(isInteractiveContext({ mode: "rpc", hasUI: true } as unknown as ExtensionContext), false);
  assert.equal(isInteractiveContext({ mode: "interactive", hasUI: false } as unknown as ExtensionContext), true);
  assert.equal(isInteractiveContext({ hasUI: true } as unknown as ExtensionContext), true);
  assert.equal(isInteractiveContext({ hasUI: false } as unknown as ExtensionContext), false);
});

test("registers Pi-native commands and does not open TUI in RPC mode", async () => {
  const commands = new Map<string, (args: string, ctx: ExtensionContext) => Promise<void>>();
  const events: string[] = [];
  const api = {
    appendEntry: () => {},
    on: (name: string) => { events.push(name); },
    registerCommand: (name: string, options: { handler: (args: string, ctx: ExtensionContext) => Promise<void> }) => {
      commands.set(name, options.handler);
    },
  } as unknown as ExtensionAPI;
  registerPhuxExtension(api);

  assert.deepEqual([...commands.keys()], ["phux", "phux-status", "phux-attach"]);
  assert.deepEqual(events, ["session_start", "session_tree"]);

  let customCalls = 0;
  const ctx = {
    mode: "rpc",
    hasUI: true,
    ui: { custom: async () => { customCalls++; } },
  } as unknown as ExtensionContext;
  await commands.get("phux")?.("", ctx);
  assert.equal(customCalls, 0);
});

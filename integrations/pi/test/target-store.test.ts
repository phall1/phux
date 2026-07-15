import assert from "node:assert/strict";
import test from "node:test";

import type { AgentPane } from "../src/schemas.js";
import {
  PHUX_TARGET_ENTRY,
  PhuxTargetStore,
  formatTargetStatus,
  type PhuxTargetSelection,
} from "../src/target-store.js";

const pane: AgentPane = {
  terminal: "@3",
  session: "work",
  window: "window-0",
  agent: { id: "codex", label: "Codex", kind: "codex" },
  state: "working",
  confidence: 0.9,
  attention: "normal",
  title: "do not persist this title",
  cwd: "/repo",
  sources: [],
  explanation: "working cue",
};

const saved: PhuxTargetSelection = {
  version: 1,
  selector: "@3",
  session: "work",
  window: "window-0",
  display: "work:window-0 @3 - Codex",
};

test("selection persists a versioned canonical target with ownership fields", () => {
  const entries: Array<{ customType: string; data: unknown }> = [];
  const store = new PhuxTargetStore(
    { appendEntry: (customType, data) => entries.push({ customType, data }) },
    { agentList: async () => ({ agents: [pane] }) },
  );

  const selection = store.select(pane);

  assert.deepEqual(selection, saved);
  assert.deepEqual(entries, [{ customType: PHUX_TARGET_ENTRY, data: saved }]);
  assert.equal(store.snapshot.availability, "available");
});

test("restore uses only the latest selection on the supplied branch", async () => {
  const otherBranch = { ...saved, selector: "@1", display: "other @1" };
  const store = new PhuxTargetStore(
    { appendEntry: () => {} },
    { agentList: async () => ({ agents: [pane] }) },
  );

  store.restoreFromBranch([
    { type: "custom", customType: PHUX_TARGET_ENTRY, data: otherBranch },
    { type: "message" },
    { type: "custom", customType: PHUX_TARGET_ENTRY, data: saved },
  ]);
  await store.refresh();

  assert.deepEqual(store.snapshot.selection, saved);
  assert.equal(store.snapshot.availability, "available");
});

test("missing restored panes stay selected and are reported stale without fallback", async () => {
  const store = new PhuxTargetStore(
    { appendEntry: () => {} },
    { agentList: async () => ({ agents: [{ ...pane, terminal: "@9" }] }) },
  );
  store.restoreFromBranch([{ type: "custom", customType: PHUX_TARGET_ENTRY, data: saved }]);

  await store.refresh();

  assert.equal(store.snapshot.selection?.selector, "@3");
  assert.equal(store.snapshot.availability, "stale");
  assert.match(formatTargetStatus(store.snapshot), /stale/);
});

test("inventory failures preserve the target and expose unavailable state", async () => {
  const store = new PhuxTargetStore(
    { appendEntry: () => {} },
    { agentList: async () => { throw new Error("server offline"); } },
  );
  store.restoreFromBranch([{ type: "custom", customType: PHUX_TARGET_ENTRY, data: saved }]);

  await store.refresh();

  assert.equal(store.snapshot.selection?.selector, "@3");
  assert.equal(store.snapshot.availability, "unavailable");
  assert.equal(store.snapshot.reason, "server offline");
});

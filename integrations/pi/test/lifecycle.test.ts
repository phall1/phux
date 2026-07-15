import assert from "node:assert/strict";
import test from "node:test";

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

import {
  PhuxLifecycle,
  registerPhuxLifecycle,
  type LifecycleTimers,
  type PhuxLifecycleAdapter,
} from "../src/lifecycle.js";
import type { AgentPane, AgentRecord, AgentStateList } from "../src/schemas.js";
import { PhuxTargetStore, type PhuxTargetSelection } from "../src/target-store.js";

const target: PhuxTargetSelection = {
  version: 1,
  selector: "@3",
  session: "work",
  window: "window-0",
  display: "work:window-0 @3",
};

class FakeTimers implements LifecycleTimers {
  private next = 0;
  private readonly callbacks = new Map<number, () => void>();

  setTimeout(callback: () => void): number {
    const id = ++this.next;
    this.callbacks.set(id, callback);
    return id;
  }

  clearTimeout(handle: unknown): void {
    this.callbacks.delete(handle as number);
  }

  runAll(): void {
    const pending = [...this.callbacks.values()];
    this.callbacks.clear();
    for (const callback of pending) callback();
  }
}

class FakeAdapter implements PhuxLifecycleAdapter {
  readonly sets: Array<{ target: string; record: AgentRecord }> = [];
  readonly shows: string[] = [];
  readonly clears: string[] = [];
  record: AgentRecord | null = null;
  failSet = false;

  async agentSet(selector: string, record: AgentRecord): Promise<AgentRecord> {
    this.sets.push({ target: selector, record });
    if (this.failSet) throw new Error("phux absent");
    this.record = record;
    return record;
  }

  async agentShow(options: { readonly target: string }): Promise<AgentStateList> {
    this.shows.push(options.target);
    return projection(this.record);
  }

  async agentClear(selector: string): Promise<void> {
    this.clears.push(selector);
    this.record = null;
  }
}

function projection(record: AgentRecord | null): AgentStateList {
  const source = record === null ? [] : [{
    kind: "agent_record",
    signal: "phux.agent/v1 metadata record",
    confidence: 0.98,
    observed: JSON.stringify(record),
  }];
  return {
    schema_version: 1,
    agents: [{
      terminal: target.selector,
      session: target.session,
      window: target.window,
      agent: { id: "pi", label: "pi", kind: "declared" },
      state: record?.state ?? "unknown",
      confidence: 0.98,
      attention: record?.attention ?? "normal",
      title: null,
      cwd: null,
      sources: source,
      explanation: "test",
    }],
  };
}

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

test("overlapping generations are serialized and end at the newest state", async () => {
  const timers = new FakeTimers();
  const calls: AgentRecord[] = [];
  let releaseFirst: (() => void) | undefined;
  const first = new Promise<void>((resolve) => { releaseFirst = resolve; });
  const adapter: PhuxLifecycleAdapter = {
    agentShow: async () => projection(null),
    agentClear: async () => {},
    agentSet: async (_selector, record) => {
      calls.push(record);
      if (calls.length === 1) await first;
      return record;
    },
  };
  const lifecycle = new PhuxLifecycle({ cli: adapter, timers, debounceMs: 5 });

  lifecycle.start("session-1", target);
  lifecycle.setState("working");
  timers.runAll();
  await flush();
  assert.equal(calls[0]?.state, "working");

  lifecycle.setState("idle");
  timers.runAll();
  await flush();
  assert.equal(calls.length, 1, "the second write waits for the first");
  releaseFirst?.();
  await lifecycle.settled();

  assert.deepEqual(calls.map((record) => record.state), ["working", "idle"]);
  assert.deepEqual(calls[1], {
    name: "pi",
    kind: "pi",
    state: "idle",
    attention: "low",
    session: "pi:session-1",
  });
});

test("phux failures stay best-effort and a later transition retries", async () => {
  const timers = new FakeTimers();
  const adapter = new FakeAdapter();
  adapter.failSet = true;
  const errors: unknown[] = [];
  const lifecycle = new PhuxLifecycle({ cli: adapter, timers, onError: (error) => errors.push(error) });

  lifecycle.start("session-1", target);
  timers.runAll();
  await lifecycle.settled();
  assert.equal(errors.length, 1);

  adapter.failSet = false;
  lifecycle.setState("working");
  timers.runAll();
  await lifecycle.settled();
  assert.equal(adapter.sets.length, 2);
  assert.equal(adapter.record?.state, "working");
});

test("shutdown reads provenance and does not clear another owner's record", async () => {
  const timers = new FakeTimers();
  const adapter = new FakeAdapter();
  const lifecycle = new PhuxLifecycle({ cli: adapter, timers });
  lifecycle.start("ours", target);
  timers.runAll();
  await lifecycle.settled();
  adapter.record = {
    name: "pi",
    kind: "pi",
    state: "idle",
    attention: "low",
    session: "pi:someone-else",
  };

  await lifecycle.shutdown();

  assert.deepEqual(adapter.shows, ["@3"]);
  assert.deepEqual(adapter.clears, []);
});

test("reload cancels resources without clear or re-set flicker", async () => {
  const timers = new FakeTimers();
  const adapter = new FakeAdapter();
  const lifecycle = new PhuxLifecycle({ cli: adapter, timers });

  lifecycle.start("session-1", target, true);
  timers.runAll();
  await lifecycle.shutdown(true);

  assert.equal(adapter.sets.length, 0);
  assert.equal(adapter.shows.length, 0);
  assert.equal(adapter.clears.length, 0);
});

test("true target departure and quit clear only the owned declaration", async () => {
  const timers = new FakeTimers();
  const adapter = new FakeAdapter();
  const lifecycle = new PhuxLifecycle({ cli: adapter, timers });
  lifecycle.start("session-1", target);
  timers.runAll();
  await lifecycle.settled();

  lifecycle.setTarget(null);
  timers.runAll();
  await lifecycle.settled();

  assert.deepEqual(adapter.clears, ["@3"]);
  assert.equal(adapter.record, null);
  await lifecycle.shutdown();
  assert.deepEqual(adapter.clears, ["@3"]);
});

test("registration maps agent_start to working and agent_settled to idle, not agent_end", async () => {
  const timers = new FakeTimers();
  const adapter = new FakeAdapter();
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const pi = {
    on: (name: string, handler: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, handler),
  } as unknown as ExtensionAPI;
  const pane = projection(null).agents[0] as AgentPane;
  const store = new PhuxTargetStore({ appendEntry: () => {} }, { agentList: async () => ({ agents: [pane] }) });
  store.select(pane);
  registerPhuxLifecycle(pi, store, { cli: adapter, timers });
  const ctx = {
    sessionManager: { getSessionId: () => "session-1" },
  } as unknown as ExtensionContext;

  handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
  handlers.get("agent_start")?.({ type: "agent_start" }, ctx);
  timers.runAll();
  await flush();
  assert.equal(adapter.sets.at(-1)?.record.state, "working");
  assert.equal(handlers.has("agent_end"), false);

  handlers.get("agent_settled")?.({ type: "agent_settled" }, ctx);
  timers.runAll();
  await flush();
  assert.equal(adapter.sets.at(-1)?.record.state, "idle");
});

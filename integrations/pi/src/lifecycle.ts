import type {
  ExtensionAPI,
  ExtensionContext,
  SessionShutdownEvent,
  SessionStartEvent,
} from "@earendil-works/pi-coding-agent";

import { PhuxCli } from "./adapter.js";
import {
  parseAgentRecord,
  type AgentRecord,
  type AgentStateList,
  type AgentAttention,
} from "./schemas.js";
import type { PhuxTargetSelection, PhuxTargetStore } from "./target-store.js";

export type PiLifecycleState = "idle" | "working";

export interface PhuxLifecycleAdapter {
  agentShow(options: { readonly target: string }): Promise<AgentStateList>;
  agentSet(target: string, record: AgentRecord): Promise<AgentRecord>;
  agentClear(target: string): Promise<void>;
}

export interface LifecycleTimers {
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface PhuxLifecycleOptions {
  readonly cli?: PhuxLifecycleAdapter;
  readonly debounceMs?: number;
  readonly timers?: LifecycleTimers;
  readonly onError?: (error: unknown) => void;
}

interface Binding {
  readonly target: PhuxTargetSelection;
  readonly owner: string;
  readonly state: PiLifecycleState;
  readonly attention: AgentAttention;
}

const systemTimers: LifecycleTimers = {
  setTimeout: (callback, delayMs) => setTimeout(callback, delayMs),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/**
 * Serialized, latest-generation lifecycle writer. The class is host-independent
 * so ordering, failures, and shutdown behavior can be tested without Pi.
 */
export class PhuxLifecycle {
  private readonly cli: PhuxLifecycleAdapter;
  private readonly debounceMs: number;
  private readonly timers: LifecycleTimers;
  private readonly onError: (error: unknown) => void;
  private timer: unknown;
  private tail: Promise<void> = Promise.resolve();
  private generation = 0;
  private active = false;
  private preserveOnStop = false;
  private owner: string | null = null;
  private target: PhuxTargetSelection | null = null;
  private state: PiLifecycleState = "idle";
  private desired: Binding | null = null;
  private applied: Binding | null = null;
  /** A target on which a write was attempted; ownership is still checked before clearing. */
  private owned: Binding | null = null;

  constructor(options: PhuxLifecycleOptions = {}) {
    this.cli = options.cli ?? new PhuxCli();
    this.debounceMs = options.debounceMs ?? 25;
    if (!Number.isSafeInteger(this.debounceMs) || this.debounceMs < 0) {
      throw new RangeError("debounceMs must be a non-negative safe integer");
    }
    this.timers = options.timers ?? systemTimers;
    this.onError = options.onError ?? (() => {});
  }

  start(sessionId: string, target: PhuxTargetSelection | null, reload = false): void {
    this.active = true;
    this.preserveOnStop = false;
    this.owner = `pi:${sessionId}`;
    this.target = target;
    this.state = "idle";
    this.desired = this.binding();
    this.generation += 1;
    if (reload) {
      // The previous extension instance deliberately left this declaration in
      // place. Adopt it without a clear/set flicker; the next real transition
      // will confirm the current state.
      this.applied = this.desired;
      this.owned = this.desired;
      return;
    }
    this.schedule();
  }

  setTarget(target: PhuxTargetSelection | null): void {
    if (!this.active) return;
    this.target = target;
    this.transition();
  }

  setState(state: PiLifecycleState): void {
    if (!this.active) return;
    this.state = state;
    this.transition();
  }

  /** Stop timers and either preserve on reload or clear only our declaration. */
  async shutdown(reload = false): Promise<void> {
    this.cancelTimer();
    this.active = false;
    this.preserveOnStop = reload;
    this.generation += 1;
    if (!reload) {
      this.desired = null;
      this.enqueue();
    }
    await this.tail;
  }

  /** Wait for all work that is currently queued (primarily for tests). */
  async settled(): Promise<void> {
    await this.tail;
  }

  private transition(): void {
    this.desired = this.binding();
    this.generation += 1;
    this.schedule();
  }

  private binding(): Binding | null {
    if (this.owner === null || this.target === null) return null;
    return {
      target: this.target,
      owner: this.owner,
      state: this.state,
      attention: this.state === "working" ? "normal" : "low",
    };
  }

  private schedule(): void {
    this.cancelTimer();
    this.timer = this.timers.setTimeout(() => {
      this.timer = undefined;
      this.enqueue();
    }, this.debounceMs);
  }

  private cancelTimer(): void {
    if (this.timer === undefined) return;
    this.timers.clearTimeout(this.timer);
    this.timer = undefined;
  }

  private enqueue(): void {
    this.tail = this.tail.then(() => this.reconcile()).catch((error: unknown) => {
      this.onError(error);
    });
  }

  private async reconcile(): Promise<void> {
    while (true) {
      if (!this.active && this.preserveOnStop) return;
      const generation = this.generation;
      const desired = this.desired;

      if (this.owned !== null && (desired === null || !sameOwnerTarget(this.owned, desired))) {
        const old = this.owned;
        const released = await this.clearOwned(old);
        if (!released) return;
        if (this.owned === old) this.owned = null;
        if (this.applied !== null && sameOwnerTarget(this.applied, old)) this.applied = null;
        if (!this.active && this.preserveOnStop) return;
        if (generation !== this.generation) continue;
      }

      if (desired === null) return;
      if (this.applied !== null && sameBinding(this.applied, desired)) return;

      this.owned = desired;
      try {
        await this.cli.agentSet(desired.target.selector, lifecycleRecord(desired));
      } catch (error) {
        this.onError(error);
        return;
      }
      this.applied = desired;
      if (!this.active && this.preserveOnStop) return;
      if (generation !== this.generation) continue;
      return;
    }
  }

  private async clearOwned(binding: Binding): Promise<boolean> {
    try {
      const projection = await this.cli.agentShow({ target: binding.target.selector });
      const pane = projection.agents.find((candidate) =>
        candidate.terminal === binding.target.selector &&
        candidate.session === binding.target.session &&
        candidate.window === binding.target.window);
      const source = pane?.sources.find((candidate) => candidate.kind === "agent_record");
      if (source === undefined) return true;
      const record = parseAgentRecord(JSON.parse(source.observed), "$.sources[].observed");
      if (record.name !== "pi" || record.kind !== "pi" || record.session !== binding.owner) return true;
      await this.cli.agentClear(binding.target.selector);
      return true;
    } catch (error) {
      // Lifecycle reporting must never make Pi startup, transitions, or exit fail.
      this.onError(error);
      return false;
    }
  }
}

export interface RegisteredPhuxLifecycle {
  readonly lifecycle: PhuxLifecycle;
}

/** Register Pi lifecycle hooks around the already-shared target store. */
export function registerPhuxLifecycle(
  pi: ExtensionAPI,
  store: PhuxTargetStore,
  options: PhuxLifecycleOptions = {},
): RegisteredPhuxLifecycle {
  const lifecycle = new PhuxLifecycle(options);
  let unsubscribe = store.subscribe((selection) => lifecycle.setTarget(selection));

  pi.on("session_start", (event: SessionStartEvent, ctx: ExtensionContext) => {
    lifecycle.start(
      ctx.sessionManager.getSessionId(),
      store.snapshot.selection,
      event.reason === "reload",
    );
  });
  pi.on("agent_start", () => lifecycle.setState("working"));
  pi.on("agent_settled", () => lifecycle.setState("idle"));
  pi.on("session_shutdown", async (event: SessionShutdownEvent) => {
    unsubscribe();
    unsubscribe = () => {};
    await lifecycle.shutdown(event.reason === "reload");
  });

  return { lifecycle };
}

function lifecycleRecord(binding: Binding): AgentRecord {
  return {
    name: "pi",
    kind: "pi",
    state: binding.state,
    attention: binding.attention,
    session: binding.owner,
  };
}

function sameOwnerTarget(left: Binding, right: Binding): boolean {
  return left.owner === right.owner &&
    left.target.selector === right.target.selector &&
    left.target.session === right.target.session &&
    left.target.window === right.target.window;
}

function sameBinding(left: Binding, right: Binding): boolean {
  return sameOwnerTarget(left, right) && left.state === right.state && left.attention === right.attention;
}

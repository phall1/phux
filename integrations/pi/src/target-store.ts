import type { AgentPane } from "./schemas.js";

export const PHUX_TARGET_ENTRY = "phux-target";
export const PHUX_TARGET_VERSION = 1 as const;

export interface PhuxTargetSelection {
  readonly version: typeof PHUX_TARGET_VERSION;
  /** Canonical pane selector emitted by phux, such as @3 or host/@3. */
  readonly selector: string;
  /** Owning phux session, used for the human attach handoff. */
  readonly session: string;
  readonly window: string;
  /** Stable human-facing label captured when the target was selected. */
  readonly display: string;
}

export type TargetAvailability = "unselected" | "available" | "stale" | "unavailable";

export interface PhuxTargetSnapshot {
  readonly selection: PhuxTargetSelection | null;
  readonly availability: TargetAvailability;
  readonly reason?: string;
}

export interface BranchEntry {
  readonly type: string;
  readonly customType?: string;
  readonly data?: unknown;
}

export interface TargetInventory {
  agentList(options?: { readonly signal?: AbortSignal }): Promise<{ readonly agents: readonly AgentPane[] }>;
}

export interface TargetPersistence {
  appendEntry<T>(customType: string, data: T): void;
}

export type PhuxTargetListener = (selection: PhuxTargetSelection | null) => void;

export class PhuxTargetStore {
  private snapshotValue: PhuxTargetSnapshot = { selection: null, availability: "unselected" };
  private panesValue: readonly AgentPane[] = [];
  private readonly listeners = new Set<PhuxTargetListener>();
  private publishedSelection: PhuxTargetSelection | null = null;

  constructor(
    private readonly persistence: TargetPersistence,
    private readonly inventory: TargetInventory,
  ) {}

  get snapshot(): PhuxTargetSnapshot {
    return this.snapshotValue;
  }

  get panes(): readonly AgentPane[] {
    return this.panesValue;
  }

  subscribe(listener: PhuxTargetListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  restoreFromBranch(entries: readonly BranchEntry[]): void {
    let restored: PhuxTargetSelection | null = null;
    for (let index = entries.length - 1; index >= 0; index--) {
      const entry = entries[index];
      if (entry?.type !== "custom" || entry.customType !== PHUX_TARGET_ENTRY) continue;
      restored = parseSelection(entry.data);
      break;
    }
    this.panesValue = [];
    this.snapshotValue = restored === null
      ? { selection: null, availability: "unselected" }
      : { selection: restored, availability: "unavailable", reason: "target has not been checked yet" };
    this.publishAvailableSelection();
  }

  async refresh(signal?: AbortSignal): Promise<PhuxTargetSnapshot> {
    try {
      const result = await this.inventory.agentList(signal === undefined ? {} : { signal });
      this.panesValue = result.agents;
      const selection = this.snapshotValue.selection;
      if (selection === null) {
        this.snapshotValue = { selection: null, availability: "unselected" };
      } else {
        const pane = result.agents.find((candidate) => candidate.terminal === selection.selector);
        if (pane === undefined) {
          this.snapshotValue = {
            selection,
            availability: "stale",
            reason: `pane ${selection.selector} is no longer present`,
          };
        } else if (pane.session !== selection.session || pane.window !== selection.window) {
          this.snapshotValue = {
            selection,
            availability: "stale",
            reason: `pane ${selection.selector} now belongs to ${pane.session}:${pane.window}; expected ${selection.session}:${selection.window}`,
          };
        } else {
          this.snapshotValue = { selection, availability: "available" };
        }
      }
    } catch (error) {
      this.panesValue = [];
      this.snapshotValue = {
        selection: this.snapshotValue.selection,
        availability: "unavailable",
        reason: error instanceof Error ? error.message : String(error),
      };
    }
    this.publishAvailableSelection();
    return this.snapshotValue;
  }

  select(pane: AgentPane): PhuxTargetSelection {
    return this.persist({
      version: PHUX_TARGET_VERSION,
      selector: pane.terminal,
      session: pane.session,
      window: pane.window,
      display: formatPaneDisplay(pane),
    });
  }

  /** Select the documented seed pane created by `phux new --json`. */
  selectCreated(session: string, terminalId: number): PhuxTargetSelection {
    const selector = `@${String(terminalId)}`;
    return this.persist({
      version: PHUX_TARGET_VERSION,
      selector,
      session,
      window: "window-0",
      display: `${session}:window-0 ${selector}`,
    });
  }

  private persist(selection: PhuxTargetSelection): PhuxTargetSelection {
    this.persistence.appendEntry(PHUX_TARGET_ENTRY, selection);
    this.snapshotValue = { selection, availability: "available" };
    this.publishAvailableSelection();
    return selection;
  }

  private publishAvailableSelection(): void {
    const selection = this.snapshotValue.availability === "available"
      ? this.snapshotValue.selection
      : null;
    if (sameSelection(this.publishedSelection, selection)) return;
    this.publishedSelection = selection;
    for (const listener of this.listeners) listener(selection);
  }
}

function sameSelection(
  left: PhuxTargetSelection | null,
  right: PhuxTargetSelection | null,
): boolean {
  if (left === null || right === null) return left === right;
  return left.selector === right.selector && left.session === right.session && left.window === right.window;
}

export function formatPaneDisplay(pane: AgentPane): string {
  const agent = pane.agent.label.trim();
  return `${pane.session}:${pane.window} ${pane.terminal}${agent.length === 0 ? "" : ` - ${agent}`}`;
}

export function formatTargetStatus(snapshot: PhuxTargetSnapshot): string {
  const selection = snapshot.selection;
  if (selection === null) {
    return snapshot.availability === "unavailable" ? "phux: unavailable" : "phux: no target";
  }
  const suffix = snapshot.availability === "available" ? "" : ` (${snapshot.availability})`;
  return `phux: ${selection.display}${suffix}`;
}

function parseSelection(value: unknown): PhuxTargetSelection | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return null;
  const row = value as Record<string, unknown>;
  if (row.version !== PHUX_TARGET_VERSION ||
      typeof row.selector !== "string" || row.selector.length === 0 ||
      typeof row.session !== "string" || row.session.length === 0 ||
      typeof row.window !== "string" || row.window.length === 0 ||
      typeof row.display !== "string" || row.display.length === 0) {
    return null;
  }
  return {
    version: PHUX_TARGET_VERSION,
    selector: row.selector,
    session: row.session,
    window: row.window,
    display: row.display,
  };
}

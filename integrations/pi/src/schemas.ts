export interface SessionSummary {
  readonly name: string;
  readonly windows: number;
  readonly attached: boolean;
}

export interface SessionList {
  readonly schema_version: 1 | 2;
  readonly sessions: readonly SessionSummary[];
  /** Canonical selectors for every addressable terminal (v2; empty for v1). */
  readonly terminals: readonly string[];
}

export interface CursorState {
  readonly x: number;
  readonly y: number;
  readonly visible: boolean;
}

export type CellColor =
  | { readonly kind: "default" }
  | { readonly kind: "palette"; readonly index: number }
  | { readonly kind: "rgb"; readonly r: number; readonly g: number; readonly b: number };

export interface CellStyle {
  readonly bold: boolean;
  readonly faint: boolean;
  readonly italic: boolean;
  readonly underline: boolean;
  readonly blink: boolean;
  readonly inverse: boolean;
  readonly invisible: boolean;
  readonly strikethrough: boolean;
  readonly overline: boolean;
  readonly fg: CellColor;
  readonly bg: CellColor;
}

export interface CellInfo {
  readonly col: number;
  readonly row: number;
  readonly semantic?: "output" | "input" | "prompt";
  readonly style: CellStyle;
}

export interface ScreenState {
  readonly schema_version: 1 | 2 | 3;
  readonly pane: number;
  readonly cols: number;
  readonly rows: number;
  readonly cursor: CursorState | null;
  readonly lines: readonly string[];
  readonly scrollback: readonly string[];
  readonly cells?: readonly CellInfo[];
}

export interface RunResult {
  readonly command: string;
  readonly exit_code: number;
  readonly output: string;
  readonly duration_ms: number;
  readonly truncated: boolean;
}

export interface CreateResult {
  readonly session: string;
  readonly terminal_id: number;
}

export type AgentKind = "codex" | "claude" | "plugin" | "declared" | "unknown";
export type AgentState = "unknown" | "idle" | "working" | "blocked" | "done";
export type AgentAttention = "none" | "low" | "normal" | "high";

export interface AgentIdentity {
  readonly id: string;
  readonly label: string;
  readonly kind: AgentKind;
}

export interface AgentSource {
  readonly kind: string;
  readonly signal: string;
  readonly confidence: number;
  readonly observed: string;
}

export interface AgentPane {
  /** Canonical phux pane selector, for example @3 or host/@3. */
  readonly terminal: string;
  readonly session: string;
  readonly window: string;
  readonly agent: AgentIdentity;
  readonly state: AgentState;
  readonly confidence: number;
  readonly attention: AgentAttention;
  readonly title: string | null;
  readonly cwd: string | null;
  readonly sources: readonly AgentSource[];
  readonly explanation: string;
}

export interface AgentStateList {
  readonly schema_version: 1;
  readonly agents: readonly AgentPane[];
}

/** The complete declared `phux.agent/v1` record written by `phux agent set`. */
export interface AgentRecord {
  readonly name: string;
  readonly kind: string;
  readonly state: AgentState;
  readonly attention: AgentAttention;
  readonly session: string;
}

export class SchemaValidationError extends Error {
  constructor(readonly path: string, expectation: string) {
    super(`${path} must be ${expectation}`);
    this.name = "SchemaValidationError";
  }
}

type RecordValue = Record<string, unknown>;

function record(value: unknown, path: string): RecordValue {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new SchemaValidationError(path, "an object");
  }
  return value as RecordValue;
}

function string(value: unknown, path: string): string {
  if (typeof value !== "string") throw new SchemaValidationError(path, "a string");
  return value;
}

function boolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") throw new SchemaValidationError(path, "a boolean");
  return value;
}

function nullableString(value: unknown, path: string): string | null {
  return value === null ? null : string(value, path);
}

function numberInRange(value: unknown, path: string, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < min || value > max) {
    throw new SchemaValidationError(path, `a number from ${min} through ${max}`);
  }
  return value;
}

function oneOf<const T extends readonly string[]>(value: unknown, path: string, values: T): T[number] {
  if (typeof value !== "string" || !values.includes(value)) {
    throw new SchemaValidationError(path, values.map((item) => JSON.stringify(item)).join(", "));
  }
  return value as T[number];
}

function integer(value: unknown, path: string, min: number, max = Number.MAX_SAFE_INTEGER): number {
  if (!Number.isSafeInteger(value) || (value as number) < min || (value as number) > max) {
    throw new SchemaValidationError(path, `an integer from ${min} through ${max}`);
  }
  return value as number;
}

function strings(value: unknown, path: string): string[] {
  if (!Array.isArray(value)) throw new SchemaValidationError(path, "an array of strings");
  return value.map((item, index) => string(item, `${path}[${index}]`));
}

export function parseSessionList(value: unknown): SessionList {
  const root = record(value, "$ (phux ls --json CLI shape)");
  if (root.schema_version !== 1 && root.schema_version !== 2) {
    throw new SchemaValidationError("$.schema_version", "a supported value (1 or 2)");
  }
  const schema = root.schema_version;
  if (!Array.isArray(root.sessions)) {
    throw new SchemaValidationError("$.sessions", "an array");
  }
  const sessions = root.sessions.map((item, index): SessionSummary => {
    const row = record(item, `$.sessions[${index}]`);
    const name = string(row.name, `$.sessions[${index}].name`);
    if (name.length === 0) throw new SchemaValidationError(`$.sessions[${index}].name`, "non-empty");
    return {
      name,
      windows: integer(row.windows, `$.sessions[${index}].windows`, 0),
      attached: boolean(row.attached, `$.sessions[${index}].attached`),
    };
  });
  const terminals = schema === 1 && root.terminals === undefined
    ? []
    : strings(root.terminals, "$.terminals");
  return { schema_version: schema, sessions, terminals };
}

function parseColor(value: unknown, path: string): CellColor {
  const color = record(value, path);
  if (color.kind === "default") return { kind: "default" };
  if (color.kind === "palette") {
    return { kind: "palette", index: integer(color.index, `${path}.index`, 0, 255) };
  }
  if (color.kind === "rgb") {
    return {
      kind: "rgb",
      r: integer(color.r, `${path}.r`, 0, 255),
      g: integer(color.g, `${path}.g`, 0, 255),
      b: integer(color.b, `${path}.b`, 0, 255),
    };
  }
  throw new SchemaValidationError(`${path}.kind`, '"default", "palette", or "rgb"');
}

function parseStyle(value: unknown, path: string): CellStyle {
  const style = record(value, path);
  return {
    bold: boolean(style.bold, `${path}.bold`),
    faint: boolean(style.faint, `${path}.faint`),
    italic: boolean(style.italic, `${path}.italic`),
    underline: boolean(style.underline, `${path}.underline`),
    blink: boolean(style.blink, `${path}.blink`),
    inverse: boolean(style.inverse, `${path}.inverse`),
    invisible: boolean(style.invisible, `${path}.invisible`),
    strikethrough: boolean(style.strikethrough, `${path}.strikethrough`),
    overline: boolean(style.overline, `${path}.overline`),
    fg: parseColor(style.fg, `${path}.fg`),
    bg: parseColor(style.bg, `${path}.bg`),
  };
}

export function parseScreenState(value: unknown): ScreenState {
  const root = record(value, "$ (phux snapshot/wait --json CLI shape)");
  const schema = integer(root.schema_version, "$.schema_version", 1, 3) as 1 | 2 | 3;
  const cols = integer(root.cols, "$.cols", 0, 65_535);
  const rows = integer(root.rows, "$.rows", 0, 65_535);
  const lines = strings(root.lines, "$.lines");
  if (lines.length !== rows) {
    throw new SchemaValidationError("$.lines", `an array with exactly $.rows (${rows}) entries`);
  }

  let cursor: CursorState | null = null;
  if (root.cursor !== null) {
    const rawCursor = record(root.cursor, "$.cursor");
    cursor = {
      x: integer(rawCursor.x, "$.cursor.x", 0, Math.max(0, cols - 1)),
      y: integer(rawCursor.y, "$.cursor.y", 0, Math.max(0, rows - 1)),
      visible: boolean(rawCursor.visible, "$.cursor.visible"),
    };
  }

  const scrollback = root.scrollback === undefined ? [] : strings(root.scrollback, "$.scrollback");
  if (root.cells === undefined) {
    return {
      schema_version: schema,
      pane: integer(root.pane, "$.pane", 0, 4_294_967_295),
      cols,
      rows,
      cursor,
      lines,
      scrollback,
    };
  }
  if (!Array.isArray(root.cells)) throw new SchemaValidationError("$.cells", "an array");
  let previous = -1;
  const cells = root.cells.map((item, index): CellInfo => {
    const path = `$.cells[${index}]`;
    const cell = record(item, path);
    const col = integer(cell.col, `${path}.col`, 0, Math.max(0, cols - 1));
    const row = integer(cell.row, `${path}.row`, 0, Math.max(0, rows - 1));
    const position = row * cols + col;
    if (position <= previous) throw new SchemaValidationError(path, "strictly row-major with no duplicate cells");
    previous = position;
    const semantic = cell.semantic;
    if (semantic !== undefined && semantic !== "output" && semantic !== "input" && semantic !== "prompt") {
      throw new SchemaValidationError(`${path}.semantic`, '"output", "input", or "prompt"');
    }
    const result: CellInfo = { col, row, style: parseStyle(cell.style, `${path}.style`) };
    return semantic === undefined ? result : { ...result, semantic };
  });
  return {
    schema_version: schema,
    pane: integer(root.pane, "$.pane", 0, 4_294_967_295),
    cols,
    rows,
    cursor,
    lines,
    scrollback,
    cells,
  };
}

export function parseCreateResult(value: unknown): CreateResult {
  const root = record(value, "$ (phux new --json CLI shape)");
  const session = string(root.session, "$.session");
  if (session.length === 0) throw new SchemaValidationError("$.session", "non-empty");
  return {
    session,
    terminal_id: integer(root.terminal_id, "$.terminal_id", 0, 4_294_967_295),
  };
}

export function parseRunResult(value: unknown): RunResult {
  const root = record(value, "$ (phux run --json CLI shape)");
  return {
    command: string(root.command, "$.command"),
    exit_code: integer(root.exit_code, "$.exit_code", -2_147_483_648, 2_147_483_647),
    output: string(root.output, "$.output"),
    duration_ms: integer(root.duration_ms, "$.duration_ms", 0),
    truncated: boolean(root.truncated, "$.truncated"),
  };
}

const AGENT_KINDS = ["codex", "claude", "plugin", "declared", "unknown"] as const;
const AGENT_STATES = ["unknown", "idle", "working", "blocked", "done"] as const;
const AGENT_ATTENTION = ["none", "low", "normal", "high"] as const;
const PANE_SELECTOR = /^(?:[^/\s]+\/)?@\d+$/;

export function parseAgentRecord(value: unknown, path = "$ (phux.agent/v1 record)"): AgentRecord {
  const root = record(value, path);
  const name = string(root.name, `${path}.name`);
  if (name.trim().length === 0) throw new SchemaValidationError(`${path}.name`, "non-empty");
  const kind = string(root.kind, `${path}.kind`);
  if (kind.trim().length === 0) throw new SchemaValidationError(`${path}.kind`, "non-empty");
  const session = string(root.session, `${path}.session`);
  if (session.trim().length === 0) throw new SchemaValidationError(`${path}.session`, "non-empty");
  return {
    name,
    kind,
    state: oneOf(root.state, `${path}.state`, AGENT_STATES),
    attention: oneOf(root.attention, `${path}.attention`, AGENT_ATTENTION),
    session,
  };
}

export function parseAgentStateList(value: unknown): AgentStateList {
  const root = record(value, "$ (phux agent list --json CLI shape)");
  if (root.schema_version !== 1) {
    throw new SchemaValidationError("$.schema_version", "the supported value 1");
  }
  if (!Array.isArray(root.agents)) throw new SchemaValidationError("$.agents", "an array");

  const agents = root.agents.map((item, index): AgentPane => {
    const path = `$.agents[${index}]`;
    const row = record(item, path);
    const terminal = string(row.terminal, `${path}.terminal`);
    if (!PANE_SELECTOR.test(terminal)) {
      throw new SchemaValidationError(`${path}.terminal`, "a canonical pane selector such as @3 or host/@3");
    }
    const identity = record(row.agent, `${path}.agent`);
    if (!Array.isArray(row.sources)) throw new SchemaValidationError(`${path}.sources`, "an array");
    return {
      terminal,
      session: string(row.session, `${path}.session`),
      window: string(row.window, `${path}.window`),
      agent: {
        id: string(identity.id, `${path}.agent.id`),
        label: string(identity.label, `${path}.agent.label`),
        kind: oneOf(identity.kind, `${path}.agent.kind`, AGENT_KINDS),
      },
      state: oneOf(row.state, `${path}.state`, AGENT_STATES),
      confidence: numberInRange(row.confidence, `${path}.confidence`, 0, 1),
      attention: oneOf(row.attention, `${path}.attention`, AGENT_ATTENTION),
      title: nullableString(row.title, `${path}.title`),
      cwd: nullableString(row.cwd, `${path}.cwd`),
      sources: row.sources.map((source, sourceIndex): AgentSource => {
        const sourcePath = `${path}.sources[${sourceIndex}]`;
        const raw = record(source, sourcePath);
        return {
          kind: string(raw.kind, `${sourcePath}.kind`),
          signal: string(raw.signal, `${sourcePath}.signal`),
          confidence: numberInRange(raw.confidence, `${sourcePath}.confidence`, 0, 1),
          observed: string(raw.observed, `${sourcePath}.observed`),
        };
      }),
      explanation: string(row.explanation, `${path}.explanation`),
    };
  });
  return { schema_version: 1, agents };
}

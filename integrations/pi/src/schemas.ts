export interface SessionSummary {
  readonly name: string;
  readonly windows: number;
  readonly attached: boolean;
}

export interface SessionList {
  readonly schema_version: 1;
  readonly sessions: readonly SessionSummary[];
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
  if (root.schema_version !== 1) {
    throw new SchemaValidationError("$.schema_version", "the supported value 1");
  }
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
  return { schema_version: 1, sessions };
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

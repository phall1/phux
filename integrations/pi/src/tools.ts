import { Type } from "@earendil-works/pi-ai";
import {
  truncateTail,
  type AgentToolResult,
  type ExtensionAPI,
  type Theme,
} from "@earendil-works/pi-coding-agent";
import { Text } from "@earendil-works/pi-tui";

import { PhuxCli } from "./adapter.js";
import type { ScreenState } from "./schemas.js";
import { PhuxTargetStore, type PhuxTargetSelection } from "./target-store.js";

const TARGET = Type.Optional(Type.String({ minLength: 1, pattern: ".*\\S.*", description: "Explicit phux target selector; otherwise use the selected /phux target" }));
const LOCAL_TIMEOUT = Type.Optional(Type.Integer({ minimum: 1, maximum: 3_600_000, description: "Local subprocess timeout in milliseconds" }));
const PHUX_TIMEOUT = Type.Optional(Type.Integer({ minimum: 0, maximum: 86_400, description: "phux operation timeout in seconds; 0 waits indefinitely" }));
const NONEMPTY_ARGV = Type.Array(Type.String(), { minItems: 1, maxItems: 256 });
const STRICT = { additionalProperties: false } as const;

export const PhuxListParams = Type.Object({ local_timeout_ms: LOCAL_TIMEOUT }, STRICT);
export const PhuxCreateParams = Type.Object({
  name: Type.String({ minLength: 1, pattern: ".*\\S.*", maxLength: 255 }),
  cwd: Type.Optional(Type.String({ minLength: 1, pattern: ".*\\S.*" })),
  command: Type.Optional(NONEMPTY_ARGV),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxSnapshotParams = Type.Object({
  target: TARGET,
  scrollback: Type.Optional(Type.Integer({ minimum: 0, maximum: 100_000 })),
  cells: Type.Optional(Type.Boolean()),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxSendKeysParams = Type.Object({
  target: TARGET,
  keys: Type.Array(Type.String({ minLength: 1 }), { minItems: 1, maxItems: 256 }),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxRunParams = Type.Object({
  target: TARGET,
  command: NONEMPTY_ARGV,
  timeout_seconds: PHUX_TIMEOUT,
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxWaitParams = Type.Object({
  target: TARGET,
  until: Type.Optional(Type.String({ minLength: 1 })),
  idle_ms: Type.Optional(Type.Integer({ minimum: 0, maximum: 86_400_000 })),
  timeout_seconds: PHUX_TIMEOUT,
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);

const MAX_MODEL_BYTES = 12 * 1024;
const MAX_MODEL_LINES = 200;

export interface PhuxToolDetails {
  readonly operation: "list" | "create" | "snapshot" | "send_keys" | "run" | "wait";
  readonly summary: string;
  readonly target?: string;
  readonly selection?: PhuxTargetSelection;
  readonly count?: number;
  readonly exitCode?: number;
  readonly durationMs?: number;
  readonly outcome?: string;
  readonly rows?: number;
  readonly cols?: number;
  readonly modelOutputTruncated?: boolean;
  readonly phuxOutputTruncated?: boolean;
}

/** Register the six non-shell phux tools against the extension's shared target store. */
export function registerPhuxTools(pi: ExtensionAPI, cli: PhuxCli, store: PhuxTargetStore): void {
  pi.registerTool({
    name: "phux_list",
    label: "phux list",
    description: "List phux sessions. Output is compact and bounded; this never changes phux focus.",
    promptSnippet: "List shared phux terminal sessions",
    parameters: PhuxListParams,
    async execute(_id, params, signal) {
      const result = await cli.ls(execution(params, signal));
      const lines = result.sessions.map((session) =>
        `${session.name}\twindows=${String(session.windows)}\tattached=${String(session.attached)}`);
      const output = bounded(lines.join("\n") || "No phux sessions.");
      return toolResult(output.text, {
        operation: "list",
        summary: `${String(result.sessions.length)} session(s)`,
        count: result.sessions.length,
        modelOutputTruncated: output.truncated,
      });
    },
    renderCall: (_args, theme) => callText(theme, "list sessions"),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_create",
    label: "phux create",
    description: "Create a named phux session without attaching, then select and persist its seed @id for subsequent tools.",
    promptSnippet: "Create and select a shared phux terminal session",
    parameters: PhuxCreateParams,
    async execute(_id, params, signal) {
      const created = await cli.create(params.name, {
        ...(params.cwd === undefined ? {} : { cwd: params.cwd }),
        ...(params.command === undefined ? {} : { command: params.command }),
        ...execution(params, signal),
      });
      if (created.session !== params.name) {
        throw new Error(`phux new returned session ${JSON.stringify(created.session)}; expected ${JSON.stringify(params.name)}`);
      }
      const selection = store.selectCreated(created.session, created.terminal_id);
      return toolResult(`Created ${created.session} at ${selection.selector}; selected it as the default phux target.`, {
        operation: "create",
        summary: `created ${selection.display}`,
        target: selection.selector,
        selection,
      });
    },
    renderCall: (args, theme) => callText(theme, `create ${args.name}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_snapshot",
    label: "phux snapshot",
    description: "Read a phux pane without attaching or resizing. Uses an explicit target or requires the available selected /phux target. Terminal text is bounded to 200 lines and 12 KiB.",
    promptSnippet: "Read bounded output from a shared phux terminal",
    parameters: PhuxSnapshotParams,
    async execute(_id, params, signal) {
      const target = resolveTarget(params.target, store);
      const screen = await cli.snapshot({
        target,
        ...(params.scrollback === undefined ? {} : { scrollback: params.scrollback }),
        ...(params.cells === undefined ? {} : { cells: params.cells }),
        ...execution(params, signal),
      });
      return screenResult("snapshot", target, screen);
    },
    renderCall: (args, theme) => callText(theme, `snapshot ${args.target ?? "selected target"}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_send_keys",
    label: "phux send keys",
    description: "Send named keys or literal strings to a phux pane. Uses an explicit target or requires the available selected /phux target. This is not a paste operation.",
    promptSnippet: "Send keystrokes to a shared phux terminal",
    parameters: PhuxSendKeysParams,
    async execute(_id, params, signal) {
      const target = resolveTarget(params.target, store);
      await cli.sendKeys(target, params.keys, execution(params, signal));
      return toolResult(`Sent ${String(params.keys.length)} key item(s) to ${target}.`, {
        operation: "send_keys",
        summary: `sent ${String(params.keys.length)} key item(s) to ${target}`,
        target,
        count: params.keys.length,
      });
    },
    renderCall: (args, theme) => callText(theme, `send ${String(args.keys.length)} key item(s) to ${args.target ?? "selected target"}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_run",
    label: "phux run",
    description: "Run an argv command in a phux pane through phux's documented run sentinel. Uses an explicit target or requires the available selected /phux target. Output is bounded to 200 lines and 12 KiB.",
    promptSnippet: "Run a command in a shared phux terminal",
    parameters: PhuxRunParams,
    async execute(_id, params, signal) {
      const target = resolveTarget(params.target, store);
      const result = await cli.run(target, params.command, {
        ...(params.timeout_seconds === undefined ? {} : { phuxTimeoutSeconds: params.timeout_seconds }),
        ...execution(params, signal),
      });
      const header = `exit=${String(result.exit_code)} duration_ms=${String(result.duration_ms)} target=${target}`;
      const output = bounded(`${header}${result.output.length === 0 ? "" : `\n${result.output}`}`);
      return toolResult(output.text, {
        operation: "run",
        summary: `exit ${String(result.exit_code)} in ${String(result.duration_ms)} ms on ${target}`,
        target,
        exitCode: result.exit_code,
        durationMs: result.duration_ms,
        modelOutputTruncated: output.truncated,
        phuxOutputTruncated: result.truncated,
      });
    },
    renderCall: (args, theme) => callText(theme, `run ${args.command[0] ?? "command"} on ${args.target ?? "selected target"}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_wait",
    label: "phux wait",
    description: "Wait for visible text or terminal idleness. Uses an explicit target or requires the available selected /phux target. The final screen is bounded to 200 lines and 12 KiB.",
    promptSnippet: "Wait for a condition in a shared phux terminal",
    parameters: PhuxWaitParams,
    async execute(_id, params, signal) {
      const target = resolveTarget(params.target, store);
      const result = await cli.wait({
        target,
        ...(params.until === undefined ? {} : { until: params.until }),
        ...(params.idle_ms === undefined ? {} : { idleMs: params.idle_ms }),
        ...(params.timeout_seconds === undefined ? {} : { phuxTimeoutSeconds: params.timeout_seconds }),
        ...execution(params, signal),
      });
      return screenResult("wait", target, result.screen, result.outcome);
    },
    renderCall: (args, theme) => callText(theme, `wait on ${args.target ?? "selected target"}`),
    renderResult: renderSummary,
  });
}

function execution(params: { readonly local_timeout_ms?: number }, signal: AbortSignal | undefined) {
  return {
    ...(signal === undefined ? {} : { signal }),
    ...(params.local_timeout_ms === undefined ? {} : { timeoutMs: params.local_timeout_ms }),
  };
}

export function resolveTarget(explicit: string | undefined, store: PhuxTargetStore): string {
  if (explicit !== undefined) return explicit;
  const snapshot = store.snapshot;
  if (snapshot.selection === null) {
    throw new Error("No phux target is selected. Pass target explicitly or run /phux to select an available pane.");
  }
  if (snapshot.availability !== "available") {
    throw new Error(`Selected phux target ${snapshot.selection.selector} is ${snapshot.availability}${snapshot.reason === undefined ? "" : `: ${snapshot.reason}`}. Pass target explicitly or run /phux to select an available pane.`);
  }
  return snapshot.selection.selector;
}

function screenResult(
  operation: "snapshot" | "wait",
  target: string,
  screen: ScreenState,
  outcome?: string,
): AgentToolResult<PhuxToolDetails> {
  const terminal = [...screen.scrollback, ...screen.lines].join("\n");
  const header = `${operation}${outcome === undefined ? "" : ` ${outcome}`} target=${target} pane=@${String(screen.pane)} size=${String(screen.cols)}x${String(screen.rows)}`;
  const output = bounded(`${header}${terminal.length === 0 ? "" : `\n${terminal}`}`);
  return toolResult(output.text, {
    operation,
    summary: `${operation}${outcome === undefined ? "" : ` ${outcome}`} on ${target} (${String(screen.cols)}x${String(screen.rows)})`,
    target,
    ...(outcome === undefined ? {} : { outcome }),
    rows: screen.rows,
    cols: screen.cols,
    modelOutputTruncated: output.truncated,
  });
}

function bounded(text: string): { readonly text: string; readonly truncated: boolean } {
  const result = truncateTail(text, { maxBytes: MAX_MODEL_BYTES, maxLines: MAX_MODEL_LINES });
  return { text: result.content, truncated: result.truncated };
}

function toolResult(text: string, details: PhuxToolDetails): AgentToolResult<PhuxToolDetails> {
  return { content: [{ type: "text", text }], details };
}

function callText(theme: Theme, text: string): Text {
  return new Text(theme.fg("toolTitle", theme.bold(`phux ${text}`)), 0, 0);
}

function renderSummary(
  result: AgentToolResult<PhuxToolDetails>,
  _options: unknown,
  theme: Theme,
): Text {
  const details = result.details;
  if (details === undefined) return new Text(theme.fg("error", "phux tool failed"), 0, 0);
  const suffix = details.modelOutputTruncated ? " (model output truncated)" : "";
  return new Text(theme.fg(details.exitCode === undefined || details.exitCode === 0 ? "success" : "warning", `${details.summary}${suffix}`), 0, 0);
}

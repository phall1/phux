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

const TARGET = Type.Optional(Type.String({ minLength: 1, maxLength: 512, pattern: ".*\\S.*", description: "Explicit CLI selector or alias:name; otherwise use the selected /phux target" }));
const REQUIRED_TARGET = Type.String({ minLength: 1, maxLength: 512, pattern: ".*\\S.*", description: "CLI selector, alias:name, or (where documented) group:name" });
const TARGET_NAME = Type.String({ minLength: 1, maxLength: 64, pattern: "^[A-Za-z][A-Za-z0-9_-]{0,63}$" });
const LOCAL_TIMEOUT = Type.Optional(Type.Integer({ minimum: 1, maximum: 3_600_000, description: "Local subprocess timeout in milliseconds" }));
const RUN_TIMEOUT = Type.Optional(Type.Integer({ minimum: 0, maximum: 86_400, description: "phux run timeout in seconds; 0 waits indefinitely" }));
const WAIT_TIMEOUT = Type.Optional(Type.Integer({ minimum: 1, maximum: 86_400, description: "phux wait timeout in seconds; omit to wait indefinitely" }));
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
  command: Type.String({ minLength: 1, pattern: ".*\\S.*", description: "One shell command line, passed to phux as a single argument" }),
  timeout_seconds: RUN_TIMEOUT,
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxWaitParams = Type.Object({
  target: TARGET,
  until: Type.Optional(Type.String({ minLength: 1 })),
  idle_ms: Type.Optional(Type.Integer({ minimum: 0, maximum: 86_400_000 })),
  timeout_seconds: WAIT_TIMEOUT,
  local_timeout_ms: LOCAL_TIMEOUT,
}, { ...STRICT, not: { required: ["until", "idle_ms"] } });
export const PhuxPanesParams = Type.Object({}, STRICT);
export const PhuxSpawnParams = Type.Object({
  satellite: Type.Optional(Type.String({ minLength: 1, maxLength: 255, pattern: ".*\\S.*" })),
  cwd: Type.Optional(Type.String({ minLength: 1, pattern: ".*\\S.*" })),
  command: Type.Optional(NONEMPTY_ARGV),
  alias: Type.Optional(TARGET_NAME),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxLaunchParams = Type.Object({
  integration: Type.String({ minLength: 1, maxLength: 255, pattern: ".*\\S.*" }),
  cwd: Type.Optional(Type.String({ minLength: 1, pattern: ".*\\S.*" })),
  extra: Type.Optional(NONEMPTY_ARGV),
  alias: Type.Optional(TARGET_NAME),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxKillParams = Type.Object({
  target: REQUIRED_TARGET,
  confirm: Type.Literal(true, { description: "Required acknowledgement that the selector or group may destroy multiple panes" }),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxSignalParams = Type.Union([
  Type.Object({
    target: TARGET,
    signal: Type.Union([Type.Literal("interrupt"), Type.Literal("freeze"), Type.Literal("resume")]),
    local_timeout_ms: LOCAL_TIMEOUT,
  }, STRICT),
  Type.Object({
    target: REQUIRED_TARGET,
    signal: Type.Union([Type.Literal("terminate"), Type.Literal("kill")]),
    confirm: Type.Literal(true, { description: "Required acknowledgement that the selector may affect multiple processes or panes" }),
    local_timeout_ms: LOCAL_TIMEOUT,
  }, STRICT),
]);
export const PhuxTagParams = Type.Object({
  action: Type.Union([Type.Literal("ls"), Type.Literal("add"), Type.Literal("rm")]),
  target: REQUIRED_TARGET,
  tags: Type.Optional(Type.Array(Type.String({ minLength: 1, maxLength: 255, pattern: ".*\\S.*" }), { minItems: 1, maxItems: 64 })),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxAskParams = Type.Object({
  target: TARGET,
  question: Type.String({ minLength: 1, maxLength: 4096, pattern: ".*\\S.*" }),
  id: Type.Optional(Type.String({ maxLength: 255 })),
  suggestions: Type.Optional(Type.Array(Type.String({ minLength: 1, maxLength: 512 }), { maxItems: 32 })),
  elapsed_seconds: Type.Optional(Type.Integer({ minimum: 0, maximum: 31_536_000 })),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxWatchParams = Type.Object({
  target: TARGET,
  duration_ms: Type.Integer({ minimum: 50, maximum: 30_000, description: "Bounded event collection window" }),
  max_events: Type.Optional(Type.Integer({ minimum: 1, maximum: 100 })),
}, STRICT);
export const PhuxRenderedSnapshotParams = Type.Object({
  session: Type.Optional(Type.String({ minLength: 1, maxLength: 255, pattern: ".*\\S.*" })),
  cols: Type.Optional(Type.Integer({ minimum: 20, maximum: 160 })),
  rows: Type.Optional(Type.Integer({ minimum: 5, maximum: 80 })),
  local_timeout_ms: LOCAL_TIMEOUT,
}, STRICT);
export const PhuxTargetsParams = Type.Union([
  Type.Object({ action: Type.Literal("list") }, STRICT),
  Type.Object({ action: Type.Literal("set_alias"), name: TARGET_NAME, target: REQUIRED_TARGET }, STRICT),
  Type.Object({ action: Type.Literal("remove_alias"), name: TARGET_NAME }, STRICT),
  Type.Object({ action: Type.Literal("set_group"), name: TARGET_NAME, targets: Type.Array(REQUIRED_TARGET, { minItems: 1, maxItems: 64 }) }, STRICT),
  Type.Object({ action: Type.Literal("remove_group"), name: TARGET_NAME }, STRICT),
]);

export const MAX_MODEL_BYTES = 12 * 1024;
export const MAX_MODEL_LINES = 200;
const MODEL_TRUNCATION_NOTICE = `[Pi adapter truncated terminal output to the last ${String(MAX_MODEL_LINES)} lines within ${String(MAX_MODEL_BYTES)} bytes]`;
const PHUX_TRUNCATION_NOTICE = "[phux reported that terminal output was already truncated]";

export interface PhuxToolDetails {
  readonly operation: "list" | "panes" | "create" | "spawn" | "launch" | "snapshot" | "rendered_snapshot" | "send_keys" | "run" | "wait" | "kill" | "signal" | "tag" | "ask" | "watch" | "targets";
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

/** Register bounded non-shell phux tools against the extension's shared target store. */
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
      const output = boundedResult(`sessions=${String(result.sessions.length)}`, lines.join("\n") || "No phux sessions.");
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
      const target = await resolveActionTarget(params.target, store, signal);
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
      const target = await resolveActionTarget(params.target, store, signal);
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
    description: "Run one shell command line in a phux pane through phux's documented run sentinel. The command is passed as one argument. Uses an explicit target or requires the available selected /phux target. Output is bounded to 200 lines and 12 KiB.",
    promptSnippet: "Run a command in a shared phux terminal",
    parameters: PhuxRunParams,
    async execute(_id, params, signal) {
      const target = await resolveActionTarget(params.target, store, signal);
      const result = await cli.run(target, [params.command], {
        ...(params.timeout_seconds === undefined ? {} : { phuxTimeoutSeconds: params.timeout_seconds }),
        ...execution(params, signal),
      });
      const header = `run exit=${String(result.exit_code)} duration_ms=${String(result.duration_ms)} target=${target}`;
      const output = boundedResult(header, result.output, result.truncated);
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
    renderCall: (args, theme) => callText(theme, `run ${args.command} on ${args.target ?? "selected target"}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_wait",
    label: "phux wait",
    description: "Wait for visible text or terminal idleness. Uses an explicit target or requires the available selected /phux target. The final screen is bounded to 200 lines and 12 KiB.",
    promptSnippet: "Wait for a condition in a shared phux terminal",
    parameters: PhuxWaitParams,
    async execute(_id, params, signal) {
      if (params.until !== undefined && params.idle_ms !== undefined) {
        throw new Error("phux_wait accepts either until or idle_ms, not both");
      }
      const target = await resolveActionTarget(params.target, store, signal);
      const timeout = operationTimeout(params.timeout_seconds);
      const result = await cli.wait({
        target,
        ...(params.until === undefined ? {} : { until: params.until }),
        ...(params.idle_ms === undefined ? {} : { idleMs: params.idle_ms }),
        ...(timeout === undefined ? {} : { phuxTimeoutSeconds: timeout }),
        ...execution(params, signal),
      });
      return screenResult("wait", target, result.screen, result.outcome);
    },
    renderCall: (args, theme) => callText(theme, `wait on ${args.target ?? "selected target"}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_panes",
    label: "phux panes",
    description: "Inventory every pane with canonical selector, ownership, agent state, attention, title, cwd, and evidence. Output is bounded.",
    promptSnippet: "Inventory shared terminal panes and attention state",
    parameters: PhuxPanesParams,
    async execute(_id, _params, signal) {
      const snapshot = await store.refresh(signal);
      if (snapshot.availability === "unavailable") throw new Error(snapshot.reason ?? "phux pane inventory unavailable");
      const lines = store.panes.map((pane) => JSON.stringify({
        terminal: pane.terminal, session: pane.session, window: pane.window,
        agent: pane.agent, state: pane.state, attention: pane.attention,
        confidence: pane.confidence, title: pane.title, cwd: pane.cwd,
        sources: pane.sources, explanation: pane.explanation,
      }));
      const output = boundedResult(`panes=${String(store.panes.length)}`, lines.join("\n") || "No phux panes.");
      return toolResult(output.text, {
        operation: "panes", summary: `${String(store.panes.length)} pane(s)`, count: store.panes.length,
        modelOutputTruncated: output.truncated,
      });
    },
    renderCall: (_args, theme) => callText(theme, "inventory panes"),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_spawn",
    label: "phux spawn",
    description: "Spawn a pane through phux without attaching. Optionally bind its validated canonical selector to a branch-local alias.",
    promptSnippet: "Spawn and optionally name a shared terminal pane",
    parameters: PhuxSpawnParams,
    async execute(_id, params, signal) {
      const spawned = await cli.spawn({
        ...(params.satellite === undefined ? {} : { satellite: params.satellite }),
        ...(params.cwd === undefined ? {} : { cwd: params.cwd }),
        ...(params.command === undefined ? {} : { command: params.command }),
        ...execution(params, signal),
      });
      const target = spawned.satellite === null ? `@${String(spawned.terminal_id)}` : `${spawned.satellite}/@${String(spawned.terminal_id)}`;
      const aliasWarning = params.alias === undefined
        ? null
        : await tryBindSpawnedAlias(store, params.alias, target, signal);
      const aliasStatus = params.alias === undefined ? "" : aliasWarning === null
        ? ` as alias:${params.alias}`
        : `; alias:${params.alias} was not saved: ${aliasWarning}`;
      return toolResult(`Spawned ${target}${aliasStatus}.`, {
        operation: "spawn", summary: `spawned ${target}${aliasWarning === null ? "" : " (alias not saved)"}`, target,
      });
    },
    renderCall: (args, theme) => callText(theme, `spawn${args.alias === undefined ? " pane" : ` alias:${args.alias}`}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_launch",
    label: "phux launch",
    description: "Launch a configured integration through phux's versioned JSON result. Validates but never displays the resolved argv, which may contain sensitive arguments.",
    promptSnippet: "Launch a configured agent integration in a pane",
    parameters: PhuxLaunchParams,
    async execute(_id, params, signal) {
      const launched = await cli.launch(params.integration, {
        ...(params.cwd === undefined ? {} : { cwd: params.cwd }),
        ...(params.extra === undefined ? {} : { extra: params.extra }),
        ...execution(params, signal),
      });
      if (launched.integration !== params.integration) throw new Error("phux launch returned a different integration id");
      const target = `@${String(launched.terminal_id)}`;
      const aliasWarning = params.alias === undefined
        ? null
        : await tryBindSpawnedAlias(store, params.alias, target, signal);
      const aliasStatus = params.alias === undefined ? "" : aliasWarning === null
        ? ` as alias:${params.alias}`
        : `; alias:${params.alias} was not saved: ${aliasWarning}`;
      return toolResult(`Launched ${launched.integration} from plugin ${launched.plugin} at ${target}${aliasStatus}.`, {
        operation: "launch", summary: `launched ${launched.integration} at ${target}${aliasWarning === null ? "" : " (alias not saved)"}`, target,
      });
    },
    renderCall: (args, theme) => callText(theme, `launch ${args.integration}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_kill",
    label: "phux kill",
    description: "Destroy an explicit CLI selector, alias:name, or every ownership-validated pane in group:name. confirm:true is required. Raw selectors and groups may resolve to and destroy multiple panes.",
    promptSnippet: "Explicitly confirm destruction of shared terminal panes",
    parameters: PhuxKillParams,
    async execute(_id, params, signal) {
      if (params.confirm !== true) throw new Error("phux_kill requires confirm:true; selectors and groups may destroy multiple panes");
      const targets = await resolveActionTargets(params.target, store, signal);
      for (const target of targets) await cli.kill(target, execution(params, signal));
      return controlResult("kill", targets);
    },
    renderCall: (args, theme) => callText(theme, `kill ${args.target}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_signal",
    label: "phux signal",
    description: "Deliver a supported process-group signal through phux to one selector or alias. terminate and kill require an explicit target and confirm:true because selectors may affect multiple processes or panes. Use phux_kill to destroy panes.",
    promptSnippet: "Signal a process group in a shared terminal",
    parameters: PhuxSignalParams,
    async execute(_id, params, signal) {
      if ((params.signal === "terminate" || params.signal === "kill") && (params.target === undefined || params.confirm !== true)) {
        throw new Error(`phux_signal ${params.signal} requires an explicit target and confirm:true; selectors may affect multiple processes or panes`);
      }
      const target = await resolveActionTarget(params.target, store, signal);
      await cli.signal(target, params.signal, execution(params, signal));
      return toolResult(`Sent ${params.signal} to ${target}.`, {
        operation: "signal", summary: `${params.signal} ${target}`, target,
      });
    },
    renderCall: (args, theme) => callText(theme, `signal ${args.target ?? "selected target"} ${args.signal}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_tag",
    label: "phux tag",
    description: "List, add, or remove canonical phux tags. group:name expands to bounded, ownership-validated per-pane CLI calls.",
    promptSnippet: "Organize shared terminal panes with tags",
    parameters: PhuxTagParams,
    async execute(_id, params, signal) {
      if (params.action === "ls" && params.tags !== undefined) throw new Error("phux_tag ls does not accept tags");
      if (params.action !== "ls" && params.tags === undefined) throw new Error(`phux_tag ${params.action} requires tags`);
      const targets = await resolveActionTargets(params.target, store, signal);
      const rows = [];
      for (const target of targets) rows.push(...await cli.tag(params.action, target, params.tags ?? [], execution(params, signal)));
      const output = boundedResult(`tag ${params.action} targets=${String(targets.length)}`, rows.map((row) => `${row.terminal}\t${row.tagsText}`).join("\n"));
      return toolResult(output.text, {
        operation: "tag", summary: `tag ${params.action} on ${String(targets.length)} target(s)`, count: targets.length,
        modelOutputTruncated: output.truncated,
      });
    },
    renderCall: (args, theme) => callText(theme, `tag ${args.action} ${args.target}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_ask",
    label: "phux ask",
    description: "Report a bounded human-attention ask event for one pane through phux. Pane attention is visible in phux_panes and phux_watch_events.",
    promptSnippet: "Report that an agent needs human attention",
    parameters: PhuxAskParams,
    async execute(_id, params, signal) {
      const target = await resolveActionTarget(params.target, store, signal);
      const asked = await cli.ask(target, params.question, {
        ...(params.id === undefined ? {} : { id: params.id }),
        ...(params.suggestions === undefined ? {} : { suggestions: params.suggestions }),
        ...(params.elapsed_seconds === undefined ? {} : { elapsedSeconds: params.elapsed_seconds }),
        ...execution(params, signal),
      });
      return toolResult(`Reported ask ${JSON.stringify(asked.id)} for ${asked.terminal}.`, {
        operation: "ask", summary: `attention requested on ${asked.terminal}`, target: asked.terminal,
      });
    },
    renderCall: (args, theme) => callText(theme, `ask ${args.target ?? "selected target"}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_watch_events",
    label: "phux watch events",
    description: "Collect events for a required bounded duration, then terminate phux watch. Returns at most max_events and never leaves an indefinite subprocess.",
    promptSnippet: "Wait briefly for bounded shared-terminal events",
    parameters: PhuxWatchParams,
    async execute(_id, params, signal) {
      const target = await resolveActionTarget(params.target, store, signal);
      const collection = await cli.watch({ target, durationMs: params.duration_ms, maxEvents: params.max_events ?? 50, ...(signal === undefined ? {} : { signal }) });
      const output = boundedResult(
        `watch target=${target} events=${String(collection.events.length)} ended=${String(collection.ended)} collection_truncated=${String(collection.truncated)}`,
        collection.events.map((event) => JSON.stringify(event)).join("\n"),
      );
      return toolResult(output.text, {
        operation: "watch", summary: `collected ${String(collection.events.length)} event(s) on ${target}`, target,
        count: collection.events.length, modelOutputTruncated: output.truncated || collection.truncated,
      });
    },
    renderCall: (args, theme) => callText(theme, `watch ${args.target ?? "selected target"} for ${String(args.duration_ms)} ms`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_rendered_snapshot",
    label: "phux rendered snapshot",
    description: "Capture the client's composited multi-pane frame at bounded dimensions. This canonical CLI mode attaches headlessly and may resize its client view.",
    promptSnippet: "Inspect the composited shared-terminal client view",
    parameters: PhuxRenderedSnapshotParams,
    async execute(_id, params, signal) {
      const frame = await cli.renderedSnapshot({
        ...(params.session === undefined ? {} : { session: params.session }),
        cols: params.cols ?? 80, rows: params.rows ?? 24,
        ...execution(params, signal),
      });
      const lines = Array.from({ length: frame.rows }, (_, row) => frame.cells
        .slice(row * frame.cols, (row + 1) * frame.cols)
        .map((cell) => sanitizeRenderText(cell.grapheme)).join(""));
      const output = boundedResult(`rendered session=${params.session ?? "last"} size=${String(frame.cols)}x${String(frame.rows)}`, lines.join("\n"));
      return toolResult(output.text, {
        operation: "rendered_snapshot", summary: `rendered ${String(frame.cols)}x${String(frame.rows)}`, rows: frame.rows, cols: frame.cols,
        modelOutputTruncated: output.truncated,
      });
    },
    renderCall: (args, theme) => callText(theme, `rendered snapshot ${args.session ?? "last session"}`),
    renderResult: renderSummary,
  });

  pi.registerTool({
    name: "phux_targets",
    label: "phux targets",
    description: "List or mutate branch-local named aliases and groups. Definitions persist in Pi session history with canonical pane ownership; alias:name and group:name never silently retarget reused ids.",
    promptSnippet: "Manage branch-local named terminal targets",
    parameters: PhuxTargetsParams,
    async execute(_id, params, signal) {
      if (params.action === "set_alias" || params.action === "set_group") {
        const snapshot = await store.refresh(signal);
        if (snapshot.availability === "unavailable") throw new Error(snapshot.reason ?? "phux pane inventory unavailable");
      }
      switch (params.action) {
        case "set_alias": {
          const targets = resolveTargets(params.target, store);
          if (targets.length !== 1) throw new Error("an alias must resolve to exactly one pane");
          const selection = store.selectionFor(targets[0] ?? "");
          if (selection === null) throw new Error("target is not present in the current pane inventory");
          store.bindAlias(params.name, selection);
          break;
        }
        case "remove_alias": store.removeAlias(params.name); break;
        case "set_group": {
          const selectors = params.targets.flatMap((target) => resolveTargets(target, store));
          const selections = selectors.map((selector) => store.selectionFor(selector));
          if (selections.some((selection) => selection === null)) throw new Error("every group target must be present in the current pane inventory");
          store.bindGroup(params.name, selections as PhuxTargetSelection[]);
          break;
        }
        case "remove_group": store.removeGroup(params.name); break;
        case "list": break;
      }
      const aliases = Object.entries(store.named.aliases).map(([name, selection]) => `alias:${name}\t${selection.display}`);
      const groups = Object.entries(store.named.groups).map(([name, selections]) => `group:${name}\t${selections.map((selection) => selection.selector).join(",")}`);
      const output = boundedResult(`aliases=${String(aliases.length)} groups=${String(groups.length)}`, [...aliases, ...groups].join("\n") || "No named phux targets.");
      return toolResult(output.text, {
        operation: "targets", summary: `${String(aliases.length)} alias(es), ${String(groups.length)} group(s)`,
        count: aliases.length + groups.length, modelOutputTruncated: output.truncated,
      });
    },
    renderCall: (args, theme) => callText(theme, `targets ${args.action}`),
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
  if (explicit?.startsWith("group:") === true) {
    throw new Error("A target group cannot be used for a single-pane operation.");
  }
  if (explicit?.startsWith("alias:") === true) return store.resolveAlias(explicit.slice("alias:".length)).selector;
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

export function resolveTargets(explicit: string | undefined, store: PhuxTargetStore): readonly string[] {
  if (explicit?.startsWith("group:") === true) {
    return store.resolveGroup(explicit.slice("group:".length)).map((selection) => selection.selector);
  }
  return [resolveTarget(explicit, store)];
}

async function resolveActionTarget(
  explicit: string | undefined,
  store: PhuxTargetStore,
  signal: AbortSignal | undefined,
): Promise<string> {
  if (explicit?.startsWith("alias:") === true) await refreshNamedTargets(store, signal);
  return resolveTarget(explicit, store);
}

async function resolveActionTargets(
  explicit: string | undefined,
  store: PhuxTargetStore,
  signal: AbortSignal | undefined,
): Promise<readonly string[]> {
  if (explicit === undefined) throw new Error("an explicit target is required");
  if (explicit.startsWith("alias:") || explicit.startsWith("group:")) await refreshNamedTargets(store, signal);
  return resolveTargets(explicit, store);
}

async function refreshNamedTargets(store: PhuxTargetStore, signal: AbortSignal | undefined): Promise<void> {
  const snapshot = await store.refresh(signal);
  if (snapshot.availability === "unavailable") {
    throw new Error(snapshot.reason ?? "phux pane inventory unavailable; named target resolution failed closed");
  }
}

async function tryBindSpawnedAlias(
  store: PhuxTargetStore,
  alias: string,
  target: string,
  signal: AbortSignal | undefined,
): Promise<string | null> {
  const snapshot = await store.refresh(signal);
  if (snapshot.availability === "unavailable") {
    return snapshot.reason ?? "could not validate spawned pane ownership";
  }
  const selection = store.selectionFor(target);
  if (selection === null) return `spawned pane ${target} was not present in the canonical pane inventory`;
  store.bindAlias(alias, selection);
  return null;
}

function controlResult(operation: "kill", targets: readonly string[]): AgentToolResult<PhuxToolDetails> {
  return toolResult(`Killed ${String(targets.length)} target(s): ${targets.join(", ")}.`, {
    operation, summary: `killed ${String(targets.length)} target(s)`, count: targets.length,
    ...(targets.length === 1 ? { target: targets[0] } : {}),
  });
}

function screenResult(
  operation: "snapshot" | "wait",
  target: string,
  screen: ScreenState,
  outcome?: string,
): AgentToolResult<PhuxToolDetails> {
  const terminal = [...screen.scrollback, ...screen.lines].join("\n");
  const header = `${operation}${outcome === undefined ? "" : ` ${outcome}`} target=${target} pane=@${String(screen.pane)} size=${String(screen.cols)}x${String(screen.rows)}`;
  const output = boundedResult(header, terminal);
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

function operationTimeout(value: number | undefined): number | undefined {
  if (value === undefined) return undefined;
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new RangeError("timeout_seconds must be a positive integer; omit it to wait indefinitely");
  }
  return value;
}

/** Bound only terminal body text so the result header and truncation notices remain visible. */
export function boundedResult(
  header: string,
  body: string,
  phuxTruncated = false,
): { readonly text: string; readonly truncated: boolean } {
  const reservedNotices = [MODEL_TRUNCATION_NOTICE, ...(phuxTruncated ? [PHUX_TRUNCATION_NOTICE] : [])];
  const separators = 1 + reservedNotices.length;
  const reservedBytes = Buffer.byteLength(header) +
    reservedNotices.reduce((total, notice) => total + Buffer.byteLength(notice), 0) + separators;
  const bodyResult = truncateTail(body, {
    maxBytes: Math.max(0, MAX_MODEL_BYTES - reservedBytes),
    maxLines: Math.max(0, MAX_MODEL_LINES - 1 - reservedNotices.length),
  });
  const notices = [
    ...(bodyResult.truncated ? [MODEL_TRUNCATION_NOTICE] : []),
    ...(phuxTruncated ? [PHUX_TRUNCATION_NOTICE] : []),
  ];
  return {
    text: [header, ...(bodyResult.content.length === 0 ? [] : [bodyResult.content]), ...notices].join("\n"),
    truncated: bodyResult.truncated,
  };
}

function toolResult(text: string, details: PhuxToolDetails): AgentToolResult<PhuxToolDetails> {
  return { content: [{ type: "text", text }], details };
}

export function sanitizeRenderText(text: string): string {
  return text
    .replace(/\x1B\][^\x07]*(?:\x07|\x1B\\)/g, "")
    .replace(/(?:\x1B\[|\x9B)[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\x1B[@-_]/g, "")
    .replace(/[\x00-\x1F\x7F-\x9F]/g, " ");
}

function callText(theme: Theme, text: string): Text {
  const safe = sanitizeRenderText(`phux ${text}`);
  return new Text(theme.fg("toolTitle", theme.bold(safe)), 0, 0);
}

function renderSummary(
  result: AgentToolResult<PhuxToolDetails>,
  _options: unknown,
  theme: Theme,
): Text {
  const details = result.details;
  if (details === undefined) return new Text(theme.fg("error", "phux tool failed"), 0, 0);
  const suffix = details.modelOutputTruncated ? " (model output truncated)" : "";
  const safe = sanitizeRenderText(`${details.summary}${suffix}`);
  return new Text(theme.fg(details.exitCode === undefined || details.exitCode === 0 ? "success" : "warning", safe), 0, 0);
}

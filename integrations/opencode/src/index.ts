import type { Plugin, PluginInput, PluginOptions } from "@opencode-ai/plugin";

import { PhuxCli, type PhuxCliOptions } from "../../pi/src/adapter.js";
import { handleLifecycleEvent, OpenCodeLifecycle } from "./lifecycle.js";
import { createPhuxTools } from "./tools.js";

export { PhuxCli } from "../../pi/src/adapter.js";
export type {
  AgentTargetOptions,
  CreateOptions,
  ExecutionOptions,
  PhuxCliOptions,
  PhuxProbe,
  RunOptions,
  SnapshotOptions,
  WaitOptions,
  WaitOutcome,
} from "../../pi/src/adapter.js";
export {
  boundedResult,
  createPhuxTools,
  DEFAULT_SHORT_TIMEOUT_MS,
  MAX_MODEL_BYTES,
  MAX_MODEL_LINES,
  resolveTarget,
} from "./tools.js";
export type { PhuxToolMetadata, PhuxToolRuntime } from "./tools.js";
export { handleLifecycleEvent, OpenCodeLifecycle } from "./lifecycle.js";
export type {
  OpenCodeLifecycleAdapter,
  OpenCodeLifecycleOptions,
  OpenCodeLifecycleState,
} from "./lifecycle.js";

/** Plugin settings plus injectable seams for library and contract tests. */
export interface PhuxOpenCodeOptions {
  readonly executable?: string;
  readonly socket?: string;
  readonly lifecycleTimeoutMs?: number;
  readonly cli?: PhuxCli;
  readonly env?: NodeJS.ProcessEnv;
  readonly onLifecycleError?: (error: unknown) => void;
}

/**
 * Public OpenCode plugin entrypoint. Each invocation owns one selected target;
 * phux_create updates it without changing phux's global focus.
 */
export const PhuxPlugin: Plugin = async (
  _input: PluginInput,
  rawOptions?: PluginOptions,
) => {
  const options = (rawOptions ?? {}) as PhuxOpenCodeOptions;
  const environment = options.env ?? process.env;
  const environmentTarget = readEnvironmentTarget(environment.PHUX_TARGET);
  const cli = options.cli ?? new PhuxCli(cliOptions(options, environment));
  let selectedTarget: string | undefined;
  const currentTarget = (): string | undefined => selectedTarget ?? environmentTarget;
  const lifecycle = new OpenCodeLifecycle({
    cli,
    target: currentTarget,
    ...(options.lifecycleTimeoutMs === undefined ? {} : { timeoutMs: options.lifecycleTimeoutMs }),
    ...(options.onLifecycleError === undefined ? {} : { onError: options.onLifecycleError }),
  });

  const tools = createPhuxTools({
    cli,
    ...(environmentTarget === undefined ? {} : { environmentTarget }),
    getSelectedTarget: () => selectedTarget,
    selectTarget: (target) => {
      selectedTarget = target;
    },
    targetSelected: (context) => {
      void lifecycle.targetSelected(context.sessionID);
    },
  });

  return {
    tool: tools,
    event: async ({ event }) => handleLifecycleEvent(lifecycle, event),
    dispose: async () => lifecycle.dispose(),
  };
};

const pluginContract: Plugin = PhuxPlugin;
void pluginContract;

export default PhuxPlugin;

function cliOptions(options: PhuxOpenCodeOptions, environment: NodeJS.ProcessEnv): PhuxCliOptions {
  return {
    ...(options.executable === undefined ? {} : { executable: options.executable }),
    ...(options.socket === undefined ? {} : { socket: options.socket }),
    env: environment,
  };
}

function readEnvironmentTarget(value: string | undefined): string | undefined {
  if (value === undefined || value.trim().length === 0) return undefined;
  if (value.length > 512) throw new RangeError("PHUX_TARGET must be at most 512 characters");
  return value;
}

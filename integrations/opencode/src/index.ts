import type { Plugin } from "@opencode-ai/plugin";

// This source import is intentionally bundled at build time. The packed plugin
// therefore shares Pi's host-independent CLI contract without a runtime link to
// the sibling integration.
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

/**
 * Minimal public OpenCode plugin entrypoint.
 *
 * This scaffold deliberately registers no hooks or tools and does not execute
 * `phux`. Later integration work can construct `PhuxCli` inside public hooks.
 */
export const PhuxPlugin = async (
  _input: unknown,
  _options?: Record<string, unknown>,
): Promise<Record<string, never>> => ({});

// Compile-time evidence that the entrypoint implements the public SDK contract
// while keeping the packed runtime free of an SDK import.
const pluginContract: Plugin = PhuxPlugin;
void pluginContract;

export default PhuxPlugin;

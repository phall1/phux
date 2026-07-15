import { PhuxError } from "./errors.js";
import {
  DEFAULT_MAX_OUTPUT_BYTES,
  nodeProcessRunner,
  type ProcessResult,
  type ProcessRunner,
  type RunRequest,
} from "./runner.js";
import {
  parseRunResult,
  parseScreenState,
  parseSessionList,
  SchemaValidationError,
  type RunResult,
  type ScreenState,
  type SessionList,
} from "./schemas.js";

export interface PhuxCliOptions {
  readonly executable?: string;
  readonly socket?: string;
  readonly cwd?: string;
  readonly env?: NodeJS.ProcessEnv;
  readonly runner?: ProcessRunner;
  readonly maxStdoutBytes?: number;
  readonly maxStderrBytes?: number;
}

export interface ExecutionOptions {
  /** Abort this local subprocess invocation. */
  readonly signal?: AbortSignal;
  /** Kill this local subprocess if it has not exited within this many ms. */
  readonly timeoutMs?: number;
}

export interface SnapshotOptions extends ExecutionOptions {
  readonly target?: string;
  /** true or zero means all retained history; a positive number bounds it. */
  readonly scrollback?: boolean | number;
  readonly cells?: boolean;
}

export interface WaitOptions extends ExecutionOptions {
  readonly target?: string;
  readonly until?: string;
  readonly idleMs?: number;
  /** phux's own wait deadline, in seconds (distinct from local timeoutMs). */
  readonly phuxTimeoutSeconds?: number;
}

export type WaitOutcome =
  | { readonly outcome: "satisfied"; readonly screen: ScreenState }
  | { readonly outcome: "timed_out"; readonly screen: ScreenState };

export interface RunOptions extends ExecutionOptions {
  /** phux's own sentinel deadline, in seconds (distinct from local timeoutMs). */
  readonly phuxTimeoutSeconds?: number;
}

export interface PhuxProbe {
  readonly available: boolean;
  readonly version?: string;
  readonly rawVersion?: string;
  readonly reason?: string;
}

const VERSION_PATTERN = /^phux\s+v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)$/;

export class PhuxCli {
  readonly executable: string;
  readonly socket: string | undefined;
  private readonly cwd: string | undefined;
  private readonly env: NodeJS.ProcessEnv | undefined;
  private readonly runner: ProcessRunner;
  private readonly maxStdoutBytes: number;
  private readonly maxStderrBytes: number;

  constructor(options: PhuxCliOptions = {}) {
    this.executable = options.executable ?? "phux";
    this.socket = options.socket;
    this.cwd = options.cwd;
    this.env = options.env;
    this.runner = options.runner ?? nodeProcessRunner;
    this.maxStdoutBytes = options.maxStdoutBytes ?? DEFAULT_MAX_OUTPUT_BYTES;
    this.maxStderrBytes = options.maxStderrBytes ?? DEFAULT_MAX_OUTPUT_BYTES;
    requireNonNegativeInteger(this.maxStdoutBytes, "maxStdoutBytes");
    requireNonNegativeInteger(this.maxStderrBytes, "maxStderrBytes");
  }

  async probe(options: ExecutionOptions = {}): Promise<PhuxProbe> {
    try {
      const result = await this.execute(["--version"], options);
      if (result.termination !== "completed") this.throwTermination(result, [this.executable, "--version"]);
      if (result.exitCode !== 0) {
        return { available: false, reason: failureMessage(result) };
      }
      const rawVersion = result.stdout.trim();
      const match = VERSION_PATTERN.exec(rawVersion);
      if (match === null || match[1] === undefined) {
        return {
          available: false,
          rawVersion,
          reason: `unexpected version output; expected "phux X.Y.Z", got ${JSON.stringify(rawVersion)}`,
        };
      }
      return { available: true, version: match[1], rawVersion };
    } catch (error) {
      if (error instanceof PhuxError &&
          (error.code === "aborted" || error.code === "timeout" || error.code === "output_limit")) {
        throw error;
      }
      return {
        available: false,
        reason: isMissingExecutable(error)
          ? `phux executable ${JSON.stringify(this.executable)} was not found; install phux or configure its absolute path`
          : error instanceof Error ? error.message : String(error),
      };
    }
  }

  async ls(options: ExecutionOptions = {}): Promise<SessionList> {
    const args = this.withSocket(["ls", "--json"]);
    return this.jsonCommand("ls", args, options, parseSessionList);
  }

  async snapshot(options: SnapshotOptions = {}): Promise<ScreenState> {
    const args = ["snapshot", "--json"];
    if (options.scrollback === true) args.push("--scrollback");
    else if (typeof options.scrollback === "number") {
      requireNonNegativeInteger(options.scrollback, "scrollback");
      args.push("--scrollback", String(options.scrollback));
    }
    if (options.cells === true) args.push("--cells");
    this.pushSocket(args);
    if (options.target !== undefined) args.push(options.target);
    return this.jsonCommand("snapshot", args, options, parseScreenState);
  }

  async wait(options: WaitOptions = {}): Promise<WaitOutcome> {
    const args = ["wait", "--json"];
    if (options.until !== undefined) args.push("--until", options.until);
    if (options.idleMs !== undefined) {
      requireNonNegativeInteger(options.idleMs, "idleMs");
      args.push("--idle", String(options.idleMs));
    }
    if (options.phuxTimeoutSeconds !== undefined) {
      requireNonNegativeInteger(options.phuxTimeoutSeconds, "phuxTimeoutSeconds");
      args.push("--timeout", String(options.phuxTimeoutSeconds));
    }
    this.pushSocket(args);
    if (options.target !== undefined) args.push(options.target);
    const result = await this.completed("wait", args, options, true);
    if (result.exitCode !== 0 && result.exitCode !== 124) {
      throw commandFailed("wait", this.executable, args, result);
    }
    const screen = parseJson("wait", this.executable, result.stdout, args, parseScreenState);
    return { outcome: result.exitCode === 124 ? "timed_out" : "satisfied", screen };
  }

  async run(target: string, command: readonly string[], options: RunOptions = {}): Promise<RunResult> {
    if (command.length === 0) throw new TypeError("command must contain at least one argv item");
    const args = ["run", "--json"];
    if (options.phuxTimeoutSeconds !== undefined) {
      requireNonNegativeInteger(options.phuxTimeoutSeconds, "phuxTimeoutSeconds");
      args.push("--timeout", String(options.phuxTimeoutSeconds));
    }
    this.pushSocket(args);
    args.push(target, ...command);

    const result = await this.completed("run", args, options, true);
    if (result.exitCode !== 0 && result.stdout.trim().length === 0) {
      throw commandFailed("run", this.executable, args, result);
    }
    const parsed = parseJson("run", this.executable, result.stdout, args, parseRunResult);
    const expectedExit = parsed.exit_code >= 0 && parsed.exit_code <= 255 ? parsed.exit_code : 255;
    if (result.exitCode !== expectedExit) {
      throw invalidResponse(
        "run",
        this.executable,
        args,
        `$.exit_code (${parsed.exit_code}) does not match process exit ${String(result.exitCode)}`,
      );
    }
    return parsed;
  }

  async sendKeys(target: string, keys: readonly string[], options: ExecutionOptions = {}): Promise<void> {
    if (keys.length === 0) throw new TypeError("keys must contain at least one item");
    const args = ["send-keys"];
    this.pushSocket(args);
    args.push(target, ...keys);
    await this.completed("send-keys", args, options, false);
  }

  private async jsonCommand<T>(
    verb: string,
    args: string[],
    options: ExecutionOptions,
    parser: (value: unknown) => T,
  ): Promise<T> {
    const result = await this.completed(verb, args, options, false);
    return parseJson(verb, this.executable, result.stdout, args, parser);
  }

  private async completed(
    verb: string,
    args: string[],
    options: ExecutionOptions,
    allowNonzero: boolean,
  ): Promise<ProcessResult> {
    let result: ProcessResult;
    try {
      result = await this.execute(args, options);
    } catch (cause) {
      const message = isMissingExecutable(cause)
        ? `phux executable ${JSON.stringify(this.executable)} was not found; install phux or configure its absolute path`
        : `could not start phux executable ${JSON.stringify(this.executable)}: ${errorText(cause)}`;
      throw new PhuxError("unavailable", message, {
        argv: [this.executable, ...args],
        cause,
      });
    }
    this.throwTermination(result, [this.executable, ...args]);
    if (!allowNonzero && result.exitCode !== 0) {
      throw commandFailed(verb, this.executable, args, result);
    }
    return result;
  }

  private execute(args: string[], options: ExecutionOptions): Promise<ProcessResult> {
    const request: RunRequest = {
      executable: this.executable,
      args,
      ...(this.cwd === undefined ? {} : { cwd: this.cwd }),
      ...(this.env === undefined ? {} : { env: this.env }),
      ...(options.signal === undefined ? {} : { signal: options.signal }),
      ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
      maxStdoutBytes: this.maxStdoutBytes,
      maxStderrBytes: this.maxStderrBytes,
    };
    return this.runner(request);
  }

  private throwTermination(result: ProcessResult, argv: readonly string[]): void {
    if (result.termination === "aborted") {
      throw new PhuxError("aborted", "phux command was aborted", { argv, stderr: result.stderr });
    }
    if (result.termination === "timed_out") {
      throw new PhuxError("timeout", "phux command exceeded its local subprocess timeout", {
        argv,
        stderr: result.stderr,
      });
    }
    if (result.termination === "output_limit") {
      const limit = result.outputLimit === "stdout" ? this.maxStdoutBytes : this.maxStderrBytes;
      throw new PhuxError(
        "output_limit",
        `phux command exceeded the ${String(limit)}-byte ${result.outputLimit} capture limit`,
        { argv, stderr: result.stderr },
      );
    }
  }

  private withSocket(args: string[]): string[] {
    this.pushSocket(args);
    return args;
  }

  private pushSocket(args: string[]): void {
    if (this.socket !== undefined) args.push("--socket", this.socket);
  }
}

function parseJson<T>(
  verb: string,
  executable: string,
  stdout: string,
  args: string[],
  parser: (value: unknown) => T,
): T {
  let value: unknown;
  try {
    value = JSON.parse(stdout);
  } catch (cause) {
    throw new PhuxError("malformed_json", `phux ${verb} returned malformed JSON: ${errorText(cause)}`, {
      argv: [executable, ...args],
      cause,
    });
  }
  try {
    return parser(value);
  } catch (cause) {
    if (cause instanceof SchemaValidationError) {
      throw invalidResponse(verb, executable, args, cause.message, cause);
    }
    throw cause;
  }
}

function invalidResponse(
  verb: string,
  executable: string,
  args: string[],
  detail: string,
  cause?: unknown,
): PhuxError {
  return new PhuxError(
    "invalid_response",
    `phux ${verb} JSON does not match its documented CLI shape: ${detail}`,
    { argv: [executable, ...args], cause },
  );
}

function commandFailed(
  verb: string,
  executable: string,
  args: string[],
  result: ProcessResult,
): PhuxError {
  return new PhuxError(
    "command_failed",
    `phux ${verb} failed with exit code ${String(result.exitCode)}${diagnosticSuffix(result.stderr)}`,
    {
      argv: [executable, ...args],
      exitCode: result.exitCode,
      stderr: result.stderr,
    },
  );
}

function diagnosticSuffix(stderr: string): string {
  const detail = stderr.trim();
  return detail.length === 0 ? "" : `: ${detail}`;
}

function failureMessage(result: ProcessResult): string {
  return `phux --version exited ${String(result.exitCode)}${diagnosticSuffix(result.stderr)}`;
}

function isMissingExecutable(error: unknown): boolean {
  return error instanceof Error && "code" in error && error.code === "ENOENT";
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function requireNonNegativeInteger(value: number, name: string): void {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new RangeError(`${name} must be a non-negative safe integer`);
  }
}

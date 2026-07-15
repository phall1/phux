import { spawn } from "node:child_process";

export interface RunRequest {
  readonly executable: string;
  readonly args: readonly string[];
  readonly cwd?: string;
  readonly env?: NodeJS.ProcessEnv;
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
}

export type ProcessTermination = "completed" | "aborted" | "timed_out";

export interface ProcessResult {
  readonly termination: ProcessTermination;
  readonly exitCode: number | null;
  readonly stdout: string;
  readonly stderr: string;
}

export type ProcessRunner = (request: RunRequest) => Promise<ProcessResult>;

/** Execute an argv vector directly. No shell is created or consulted. */
export const nodeProcessRunner: ProcessRunner = (request) =>
  new Promise((resolve, reject) => {
    if (request.signal?.aborted) {
      resolve({ termination: "aborted", exitCode: null, stdout: "", stderr: "" });
      return;
    }
    if (request.timeoutMs !== undefined &&
        (!Number.isFinite(request.timeoutMs) || request.timeoutMs < 0)) {
      reject(new RangeError("timeoutMs must be a non-negative finite number"));
      return;
    }

    const child = spawn(request.executable, [...request.args], {
      cwd: request.cwd,
      env: request.env,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let termination: ProcessTermination = "completed";
    let timeout: NodeJS.Timeout | undefined;
    let forceKill: NodeJS.Timeout | undefined;

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => { stdout += chunk; });
    child.stderr.on("data", (chunk: string) => { stderr += chunk; });

    const stop = (reason: ProcessTermination): void => {
      if (termination !== "completed") return;
      termination = reason;
      child.kill("SIGTERM");
      forceKill = setTimeout(() => child.kill("SIGKILL"), 1_000);
      forceKill.unref();
    };
    const onAbort = (): void => stop("aborted");
    request.signal?.addEventListener("abort", onAbort, { once: true });
    if (request.timeoutMs !== undefined) {
      timeout = setTimeout(() => stop("timed_out"), request.timeoutMs);
      timeout.unref();
    }

    child.once("error", (error) => {
      if (timeout !== undefined) clearTimeout(timeout);
      if (forceKill !== undefined) clearTimeout(forceKill);
      request.signal?.removeEventListener("abort", onAbort);
      reject(error);
    });
    child.once("close", (exitCode) => {
      if (timeout !== undefined) clearTimeout(timeout);
      if (forceKill !== undefined) clearTimeout(forceKill);
      request.signal?.removeEventListener("abort", onAbort);
      resolve({ termination, exitCode, stdout, stderr });
    });
  });

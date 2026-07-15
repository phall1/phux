import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, spawnSync } from "node:child_process";

if (process.env.PHUX_PI_REAL_SMOKE !== "1") {
  process.stdout.write("real smoke skipped; set PHUX_PI_REAL_SMOKE=1 to opt in\n");
  process.exit(0);
}

const phux = process.env.PHUX ?? "phux";
const temp = await mkdtemp(join(tmpdir(), "phux-pi-real-smoke-"));
const socket = join(temp, "runtime", "phux.sock");
const session = "pi-package-smoke";
const env = { ...process.env };
for (const name of [
  "PHUX_WS_ADDR", "PHUX_QUIC_ADDR", "PHUX_WT_ADDR",
  "PHUX_WS_TLS_CERT", "PHUX_WS_TLS_KEY", "PHUX_WS_TOKENS",
]) {
  delete env[name];
}
Object.assign(env, {
  PHUX_SOCKET: socket,
  XDG_CACHE_HOME: join(temp, "cache"),
  XDG_CONFIG_HOME: join(temp, "config"),
  XDG_DATA_HOME: join(temp, "data"),
  XDG_RUNTIME_DIR: join(temp, "runtime"),
  XDG_STATE_HOME: join(temp, "state"),
});
let server;
let serverStderr = "";
let cleanupPromise;
let terminating = false;
const signalHandlers = new Map();
for (const signal of ["SIGINT", "SIGTERM"]) {
  const handler = () => { void handleTerminationSignal(signal); };
  signalHandlers.set(signal, handler);
  process.once(signal, handler);
}

try {
  const version = run(phux, ["--version"], env).stdout.trim();
  const match = /^phux\s+v?(\d+)\.(\d+)\.(\d+)/.exec(version);
  assert.ok(match, `unexpected phux version output: ${JSON.stringify(version)}`);
  const [major, minor, patch] = match.slice(1, 4).map(Number);
  const compatible = major > 0 ||
    (major === 0 && (minor > 1 || (minor === 1 && patch >= 0 && !version.includes("-"))));
  assert.ok(compatible, `real smoke requires phux >= 0.1.0, got ${version}`);

  server = spawn(phux, ["server", "--socket", socket, "--session", "pi-smoke-bootstrap"], {
    env,
    stdio: ["ignore", "ignore", "pipe"],
  });
  server.stderr.setEncoding("utf8");
  server.stderr.on("data", (chunk) => { serverStderr = `${serverStderr}${chunk}`.slice(-16_384); });

  await waitForServer(phux, socket, env, server);
  const created = JSON.parse(run(phux, [
    "new", "--json", "-s", session, "--socket", socket,
  ], env).stdout);
  assert.equal(created.session, session);
  const target = `@${String(created.terminal_id)}`;

  const marker = "PHUX_PI_SMOKE_OK";
  const commandResult = run(phux, [
    "run", "--json", "--timeout", "10", "--socket", socket,
    target, `printf '${marker}\\n'`,
  ], env, true);
  const command = JSON.parse(commandResult.stdout);
  assert.equal(command.exit_code, 0);
  assert.match(command.output, new RegExp(marker));

  const snapshot = JSON.parse(run(phux, [
    "snapshot", "--json", "--scrollback", "20", "--socket", socket, target,
  ], env).stdout);
  assert.equal(snapshot.pane, created.terminal_id);
  assert.match([...snapshot.scrollback, ...snapshot.lines].join("\n"), new RegExp(marker));

  const attachArgv = ["phux", "attach", "--socket", socket, session];
  process.stdout.write(`created ${session} ${target}; run exit=${String(command.exit_code)}; snapshot=${String(snapshot.cols)}x${String(snapshot.rows)}\n`);
  process.stdout.write(`human attach argv: ${JSON.stringify(attachArgv)}\n`);
} finally {
  await cleanup();
  if (!terminating) removeSignalHandlers();
}

async function cleanup() {
  cleanupPromise ??= cleanupOnce();
  return cleanupPromise;
}

async function cleanupOnce() {
  const child = server;
  try {
    if (child !== undefined && !hasExited(child)) {
      spawnSync(phux, ["kill", "--socket", socket, session], {
        env,
        encoding: "utf8",
        timeout: 5_000,
      });
      child.kill("SIGTERM");
      await Promise.race([onceExit(child), delay(3_000)]);
      if (!hasExited(child)) {
        child.kill("SIGKILL");
        await Promise.race([onceExit(child), delay(3_000)]);
      }
      if (!hasExited(child)) throw new Error("private phux server did not terminate");
    }
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}

async function handleTerminationSignal(signal) {
  if (terminating) return;
  terminating = true;
  try {
    await cleanup();
  } catch (error) {
    process.stderr.write(`real smoke cleanup failed after ${signal}: ${error instanceof Error ? error.message : String(error)}\n`);
    removeSignalHandlers();
    process.exit(1);
  }
  removeSignalHandlers();
  process.kill(process.pid, signal);
}

function removeSignalHandlers() {
  for (const [signal, handler] of signalHandlers) {
    process.removeListener(signal, handler);
  }
  signalHandlers.clear();
}

async function waitForServer(executable, socketPath, childEnv, child) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (hasExited(child)) {
      throw new Error(`phux server exited early (code=${String(child.exitCode)}, signal=${String(child.signalCode)}): ${serverStderr}`);
    }
    const result = spawnSync(executable, ["ls", "--json", "--socket", socketPath], {
      env: childEnv,
      encoding: "utf8",
      timeout: 2_000,
    });
    if (result.status === 0) return;
    await delay(25);
  }
  throw new Error(`phux server did not become ready: ${serverStderr}`);
}

function run(command, args, childEnv, allowChildExit = false) {
  const result = spawnSync(command, args, { env: childEnv, encoding: "utf8", timeout: 30_000 });
  if (result.error !== undefined) throw result.error;
  if ((!allowChildExit && result.status !== 0) || (allowChildExit && result.stdout.trim().length === 0)) {
    throw new Error(`${command} ${args.join(" ")} failed (${String(result.status)}): ${result.stderr}`);
  }
  return result;
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function hasExited(child) {
  return child.exitCode !== null || child.signalCode !== null;
}

function onceExit(child) {
  if (hasExited(child)) return Promise.resolve();
  return new Promise((resolve) => child.once("exit", resolve));
}

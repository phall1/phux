import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const packageRoot = fileURLToPath(new URL("../", import.meta.url));
const entry = join(packageRoot, "dist", "index.js");
const opencode = process.env.OPENCODE_BIN ?? "opencode";
const temporaryRoot = await mkdtemp(join(tmpdir(), "phux-opencode-runtime-"));
const configDirectory = join(temporaryRoot, "config", "opencode");
const projectDirectory = join(temporaryRoot, "project");
const pluginUrl = pathToFileURL(entry).href;

try {
  await mkdir(configDirectory, { recursive: true });
  await mkdir(projectDirectory, { recursive: true });
  await writeFile(join(configDirectory, "opencode.json"), `${JSON.stringify({ plugin: [pluginUrl] }, null, 2)}\n`);

  const output = execFileSync(opencode, ["debug", "config"], {
    cwd: projectDirectory,
    encoding: "utf8",
    timeout: 30_000,
    env: {
      ...process.env,
      HOME: temporaryRoot,
      XDG_CONFIG_HOME: join(temporaryRoot, "config"),
      XDG_DATA_HOME: join(temporaryRoot, "data"),
      XDG_CACHE_HOME: join(temporaryRoot, "cache"),
      XDG_STATE_HOME: join(temporaryRoot, "state"),
      OPENCODE_DISABLE_AUTOUPDATE: "1",
    },
  });

  const resolved = JSON.parse(output);
  assert.deepEqual(resolved.plugin, [pluginUrl]);
  process.stdout.write(`OpenCode loaded ${pluginUrl} in an isolated config runtime.\n`);
} finally {
  await rm(temporaryRoot, { recursive: true, force: true });
}

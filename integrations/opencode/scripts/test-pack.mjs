import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const packageRoot = new URL("../", import.meta.url);
const temporaryRoot = await mkdtemp(join(tmpdir(), "phux-opencode-pack-"));

try {
  const packed = JSON.parse(execFileSync(
    "npm",
    ["pack", "--json", "--ignore-scripts", "--pack-destination", temporaryRoot],
    { cwd: packageRoot, encoding: "utf8" },
  ));
  assert.equal(packed.length, 1);
  const manifest = packed[0];
  const names = manifest.files.map((file) => file.path).sort();
  assert.deepEqual(names, [
    "README.md",
    "dist/index.d.ts",
    "dist/index.js",
    "dist/index.js.map",
    "package.json",
  ]);

  const tarball = join(temporaryRoot, manifest.filename);
  const consumerRoot = join(temporaryRoot, "consumer");
  execFileSync(
    "npm",
    ["install", "--ignore-scripts", "--omit=dev", "--prefix", consumerRoot, tarball],
    { stdio: "pipe" },
  );

  const installedEntry = join(consumerRoot, "node_modules", "@phux", "opencode", "dist", "index.js");
  const bundledSource = await readFile(installedEntry, "utf8");
  assert.doesNotMatch(bundledSource, /\.\.\/\.\.\/pi|from\s+["']@phux\/pi|@opencode-ai\/plugin/);
  assert.match(bundledSource, /child_process/);

  const plugin = await import(pathToFileURL(installedEntry).href);
  assert.equal(typeof plugin.default, "function");
  assert.equal(typeof plugin.PhuxCli, "function");
  assert.deepEqual(await plugin.default({}), {});
} finally {
  await rm(temporaryRoot, { recursive: true, force: true });
}

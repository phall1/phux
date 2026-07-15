import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const packageDir = new URL("..", import.meta.url).pathname.replace(/\/$/, "");
const temp = await mkdtemp(join(tmpdir(), "phux-pi-pack-"));

try {
  const packed = run("npm", ["pack", "--json", "--ignore-scripts", "--pack-destination", temp], packageDir);
  const report = JSON.parse(packed.stdout);
  assert.equal(report.length, 1, "npm pack must produce exactly one archive");
  const entry = report[0];
  assert.equal(entry.name, "@phux/pi");

  const files = new Set(entry.files.map((file) => file.path));
  for (const required of [
    "README.md",
    "package.json",
    "extensions/index.ts",
    "dist/extensions/index.js",
    "dist/src/index.js",
    "dist/src/index.d.ts",
  ]) {
    assert.ok(files.has(required), `packed archive is missing ${required}`);
  }
  for (const path of files) {
    assert.ok(!path.startsWith("test/"), `tests must not ship in the archive: ${path}`);
    assert.ok(!path.startsWith("scripts/"), `validation scripts must not ship in the archive: ${path}`);
    assert.ok(!path.startsWith("node_modules/"), `node_modules must not ship in the archive: ${path}`);
  }

  const archive = join(temp, entry.filename);
  run("tar", ["-xzf", archive, "-C", temp], packageDir);
  const metadata = JSON.parse(await readFile(join(temp, "package", "package.json"), "utf8"));
  assert.equal(metadata.private, true, "the in-tree package must not be publishable accidentally");
  assert.deepEqual(metadata.pi?.extensions, ["./extensions/index.ts"]);
  assert.equal(metadata.exports?.["."]?.import, "./dist/src/index.js");

  process.stdout.write(`pack check passed: ${entry.filename} (${String(entry.files.length)} files)\n`);
} finally {
  await rm(temp, { recursive: true, force: true });
}

function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, encoding: "utf8", env: process.env });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed (${String(result.status)}):\n${result.stderr}`);
  }
  return result;
}

import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { zipSync } from "fflate";

import { checkPluginVersions } from "./check-plugin-versions.mjs";

function archive(dir, name, manifest) {
  const path = join(dir, name);
  writeFileSync(path, zipSync({
    "manifest.json": Buffer.from(JSON.stringify(manifest)),
    "plugin.js": Buffer.from("export {}"),
  }));
  return path;
}

test("accepts real .s2sp archives whose manifest versions exactly match", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2script-plugin-versions-"));
  const paths = [
    archive(dir, "one.s2sp", { id: "one", version: "2.3.4" }),
    archive(dir, "two.s2sp", { id: "two", version: "2.3.4" }),
  ];

  assert.deepEqual(checkPluginVersions("2.3.4", paths), [
    { path: paths[0], id: "one", version: "2.3.4" },
    { path: paths[1], id: "two", version: "2.3.4" },
  ]);
});

test("rejects mismatches, including prerelease differences", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2script-plugin-versions-"));
  const stable = archive(dir, "stable.s2sp", { id: "stable", version: "2.3.4" });
  const prerelease = archive(dir, "prerelease.s2sp", { id: "prerelease", version: "2.3.4-rc.1" });

  assert.throws(
    () => checkPluginVersions("2.3.4", [stable, prerelease]),
    /prerelease\.s2sp.*version 2\.3\.4-rc\.1.*expected 2\.3\.4/,
  );
  assert.throws(
    () => checkPluginVersions("2.3.4-rc.1", [stable]),
    /stable\.s2sp.*version 2\.3\.4.*expected 2\.3\.4-rc\.1/,
  );
});

test("rejects corrupt archives and invalid manifests", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2script-plugin-versions-"));
  const corrupt = join(dir, "corrupt.s2sp");
  writeFileSync(corrupt, "not a zip");
  const missing = join(dir, "missing.s2sp");
  writeFileSync(missing, zipSync({ "plugin.js": Buffer.from("export {}") }));
  const invalid = archive(dir, "invalid.s2sp", { id: "invalid", version: "banana" });

  assert.throws(() => checkPluginVersions("2.3.4", [corrupt]), /corrupt\.s2sp.*cannot read archive/);
  assert.throws(() => checkPluginVersions("2.3.4", [missing]), /missing\.s2sp.*has no manifest\.json/);
  assert.throws(() => checkPluginVersions("2.3.4", [invalid]), /invalid\.s2sp.*invalid manifest version/);
});

test("requires a concrete expected semver and at least one archive", () => {
  assert.throws(() => checkPluginVersions("2.x", ["unused.s2sp"]), /invalid expected version/);
  assert.throws(() => checkPluginVersions("2.3.4", []), /at least one \.s2sp/);
});

test("CLI accepts expected semver followed by archive paths", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2script-plugin-versions-"));
  const path = archive(dir, "plugin.s2sp", { id: "plugin", version: "1.2.3-rc.2" });
  const script = fileURLToPath(new URL("./check-plugin-versions.mjs", import.meta.url));

  const result = spawnSync(process.execPath, [script, "1.2.3-rc.2", path], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /match 1\.2\.3-rc\.2 \(1 archives\)/);
});

test("matches build metadata exactly", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2script-plugin-versions-"));
  const path = archive(dir, "metadata.s2sp", {version:"1.2.3+build.7"});
  assert.equal(checkPluginVersions("1.2.3+build.7", [path])[0].version,"1.2.3+build.7");
  assert.throws(()=>checkPluginVersions("1.2.3+build.8",[path]), /expected/);
});

/**
 * `s2s version` in a bundle workspace: the root (which changesets never versions) is bumped by the
 * largest bump among its released members, and left alone when no member is released.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/version-bundle.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { loadWorkspace } from "../src/workspace/workspace.ts";
import { plannedBundleBumps } from "../src/version/bundle-bump.ts";
import { versionWorkspace } from "../src/version/version.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = join(here, "fixtures", "ws-bundle");

function findNodeModules(from) {
  let dir = from;
  for (;;) {
    if (existsSync(join(dir, "node_modules", "@changesets", "config"))) return join(dir, "node_modules");
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}
const nodeModules = findNodeModules(here);

function scratch(t, changesets) {
  const dir = mkdtempSync(join(tmpdir(), "s2s-version-bundle-"));
  cpSync(fixture, dir, { recursive: true });
  if (nodeModules !== null) symlinkSync(nodeModules, join(dir, "node_modules"), "dir");
  for (const [id, body] of Object.entries(changesets)) writeFileSync(join(dir, ".changeset", `${id}.md`), body);
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

const release = (name, type, newVersion = "9.9.9") => ({ name, type, oldVersion: "0.0.0", newVersion, changesets: [] });

test("bundle bump: follows the largest member bump", () => {
  const ws = loadWorkspace(fixture);
  assert.deepEqual(
    plannedBundleBumps(ws, [release("@fixture/jb-core", "patch", "1.2.1"), release("@fixture/jb-perks", "minor", "0.4.0")]),
    [{
      name: "@fixture/jb",
      from: "1.0.0",
      to: "1.1.0",
      type: "minor",
      because: ["@fixture/jb-core@1.2.1", "@fixture/jb-perks@0.4.0"],
      path: join(fixture, "package.json"),
    }],
  );
  assert.equal(plannedBundleBumps(ws, [release("@fixture/jb-core", "major")])[0].to, "2.0.0");
});

test("bundle bump: ignores private plugins, none-type releases and non-bundle workspaces", () => {
  const ws = loadWorkspace(fixture);
  assert.deepEqual(plannedBundleBumps(ws, [release("@fixture/jb-devtools", "major")]), []);
  assert.deepEqual(plannedBundleBumps(ws, [release("@fixture/jb-core", "none")]), []);
  const plain = { ...ws, pkg: { ...ws.pkg, s2script: { workspace: ws.pkg.s2script.workspace } } };
  assert.deepEqual(plannedBundleBumps(plain, [release("@fixture/jb-core", "major")]), []);
});

test("s2s version: a member changeset bumps the member and the bundle root on disk", { skip: nodeModules === null }, async (t) => {
  const dir = scratch(t, {
    "perks-feature": `---\n"@fixture/jb-perks": minor\n---\n\nA feature.\n`,
  });
  const result = await versionWorkspace(dir);
  assert.equal(result.bundleBumps[0]?.to, "1.1.0");
  const root = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  assert.equal(root.version, "1.1.0");
  assert.equal(root.s2script.kind, "bundle", "the rest of the root is preserved");
  const perks = JSON.parse(readFileSync(join(dir, "plugins", "perks", "package.json"), "utf8"));
  assert.equal(perks.version, "0.4.0");
  assert.ok(result.touchedFiles.includes(join(dir, "package.json")));
});

test("s2s version: no changesets leaves the bundle root alone", { skip: nodeModules === null }, async (t) => {
  const dir = scratch(t, {});
  const result = await versionWorkspace(dir);
  assert.deepEqual(result.bundleBumps, []);
  assert.equal(JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).version, "1.0.0");
});

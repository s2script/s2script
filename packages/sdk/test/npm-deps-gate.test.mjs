/**
 * TDD test: the runtime-dependencies gate. Plugins run in bare V8, so an npm package
 * that touches Node APIs builds green and dies on a live server — catch it here.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/npm-deps-gate.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, mkdirSync, symlinkSync, writeFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertNoNpmRuntimeDeps } from "../src/npm-deps-gate.ts";

function dirWithNodeModules(entries) {
  const dir = mkdtempSync(join(tmpdir(), "s2npmgate-"));
  const nm = join(dir, "node_modules");
  mkdirSync(nm, { recursive: true });
  for (const [name, mode] of Object.entries(entries)) {
    const target = join(nm, ...name.split("/"));
    mkdirSync(join(target, ".."), { recursive: true });
    if (mode === "symlink") {
      const real = join(dir, "packages", name.replace(/[@/]/g, "_"));
      mkdirSync(real, { recursive: true });
      symlinkSync(real, target, "dir");
    } else {
      mkdirSync(target, { recursive: true });
      writeFileSync(join(target, "package.json"), "{}");
    }
  }
  return dir;
}

test("@s2script/* runtime deps are always allowed (builtins, external at bundle time)", () => {
  const dir = dirWithNodeModules({ "@s2script/sdk": "real" });
  assert.doesNotThrow(() => assertNoNpmRuntimeDeps({ dependencies: { "@s2script/sdk": "^0.11.0" } }, dir));
});

test("a workspace-linked dep is allowed — it is local source the author wrote", () => {
  const dir = dirWithNodeModules({ "@mono/core": "symlink" });
  assert.doesNotThrow(() => assertNoNpmRuntimeDeps({ dependencies: { "@mono/core": "*" } }, dir));
});

test("a file:/workspace: range is allowed without touching node_modules", () => {
  const dir = dirWithNodeModules({});
  assert.doesNotThrow(() =>
    assertNoNpmRuntimeDeps({ dependencies: { a: "file:../a", b: "workspace:*" } }, dir),
  );
});

test("a plain registry dep is refused, naming the dep and the fix", () => {
  const dir = dirWithNodeModules({ mysql2: "real" });
  assert.throws(
    () => assertNoNpmRuntimeDeps({ dependencies: { mysql2: "^3.0.0" } }, dir),
    (e) => /mysql2/.test(e.message) && /bare V8/.test(e.message) && /s2script\.libraries/.test(e.message),
  );
});

test("no dependencies block at all is fine", () => {
  const dir = dirWithNodeModules({});
  assert.doesNotThrow(() => assertNoNpmRuntimeDeps({}, dir));
});

// --- Fix round 1 (real-npm-install regression) ---
//
// Every hand-placed fixture above puts node_modules directly under the plugin's own directory —
// that's the shape npm produces ONLY when the plugin's own package.json is itself the workspace
// root. It is NOT the shape this repo actually uses: examples/monorepo, and every plugins/*
// package, sit as NESTED members under a workspace root ABOVE them, and a real `npm install`
// hoists a sibling's symlink to that ROOT's node_modules — the plugin directory being built gets
// no node_modules of its own at all. A fixture built from this file's own assumptions can't catch
// that gap (it's the exact gap fix round 1 found), so this one runs an ACTUAL `npm install`
// against the real topology instead of asserting anything about what npm does from memory.
function npmInstallNestedSiblings() {
  const root = mkdtempSync(join(tmpdir(), "s2npmgate-real-"));
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify({ name: "root", private: true, version: "0.0.0", workspaces: ["plugin", "packages/*"] }),
  );
  const pluginDir = join(root, "plugin");
  mkdirSync(pluginDir, { recursive: true });
  writeFileSync(
    join(pluginDir, "package.json"),
    JSON.stringify({ name: "probe-plugin", version: "0.0.0", dependencies: { "@probe/dep": "*" } }),
  );
  const depDir = join(root, "packages", "dep");
  mkdirSync(depDir, { recursive: true });
  writeFileSync(join(depDir, "package.json"), JSON.stringify({ name: "@probe/dep", version: "0.0.0" }));
  // --offline: every dependency here is a local workspace member, so this never needs the
  // registry — it just makes that guarantee explicit instead of hoping the sandbox has network.
  execFileSync("npm", ["install", "--offline", "--no-audit", "--no-fund"], { cwd: root, stdio: "pipe" });
  return { root, pluginDir };
}

test(
  "REAL npm install, root workspace ABOVE nested plugin+dep siblings (the topology every " +
    "plugin/example in this repo actually uses) — the sibling is allowed",
  () => {
    const { pluginDir } = npmInstallNestedSiblings();
    // Confirm the premise, not just the conclusion: npm did NOT create a node_modules inside the
    // plugin's own directory, so a check confined to `pluginDir/node_modules` would find nothing
    // and wrongly refuse this dependency.
    assert.throws(() => readdirSync(join(pluginDir, "node_modules")));
    assert.doesNotThrow(() => assertNoNpmRuntimeDeps({ dependencies: { "@probe/dep": "*" } }, pluginDir));
  },
);

test("a registry dep hoisted to an ANCESTOR node_modules (not the plugin's own) is still refused", () => {
  // Simulates the ancestor-hoisted case for a genuine registry install: npm always materializes
  // a registry package as a real directory (never a symlink), regardless of hoisting depth — so
  // this is built by hand rather than via a real install, which would need network access for an
  // actual outside package.
  const root = mkdtempSync(join(tmpdir(), "s2npmgate-realdep-"));
  const pluginDir = join(root, "nested", "plugin");
  mkdirSync(pluginDir, { recursive: true });
  const pkgDir = join(root, "node_modules", "mysql2");
  mkdirSync(pkgDir, { recursive: true });
  writeFileSync(join(pkgDir, "package.json"), "{}");
  assert.throws(
    () => assertNoNpmRuntimeDeps({ dependencies: { mysql2: "^3.0.0" } }, pluginDir),
    (e) => /mysql2/.test(e.message) && /bare V8/.test(e.message),
  );
});

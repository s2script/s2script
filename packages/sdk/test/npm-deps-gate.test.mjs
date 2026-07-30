/**
 * TDD test: the runtime-dependencies gate. Plugins run in bare V8, so an npm package
 * that touches Node APIs builds green and dies on a live server — catch it here.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/npm-deps-gate.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
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

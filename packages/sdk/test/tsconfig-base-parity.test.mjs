import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { sharedCompilerOptionsJson } from "../src/tsconfig-shared.ts";

const here = dirname(fileURLToPath(import.meta.url));

test("tsconfig.base.json carries EVERY shared compiler option (editor == gate)", () => {
  const base = JSON.parse(
    readFileSync(join(here, "..", "..", "..", "tsconfig.base.json"), "utf8"),
  );
  for (const [key, want] of Object.entries(sharedCompilerOptionsJson)) {
    assert.deepEqual(
      base.compilerOptions[key],
      want,
      `tsconfig.base.json compilerOptions.${key} drifted from tsconfig-shared.ts`,
    );
  }
});

test("sdk exports map serves every root .d.ts subpath (no editor-only 404s)", () => {
  const pkg = JSON.parse(readFileSync(join(here, "..", "package.json"), "utf8"));
  assert.ok(pkg.exports["./plugin"], "exports must include ./plugin — the L1 entry surface");
  assert.equal(pkg.exports["./plugin"].types, "./plugin.d.ts");
});

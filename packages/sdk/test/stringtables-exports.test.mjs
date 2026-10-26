// packages/sdk/test/stringtables-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./stringtables", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./stringtables"], "expected a ./stringtables export subpath");
});

test("stringtables.d.ts declares StringTables and the StringTable handle", () => {
  const dts = readFileSync(join(root, "stringtables.d.ts"), "utf8");
  assert.match(dts, /export interface StringTable \{/);
  assert.match(dts, /export declare const StringTables/);
  for (const m of ["findString(", "getString(", "getData(", "setData(", "addString(", "find(", "all("])
    assert.ok(dts.includes(m), `expected ${m}`);
});

test("the root barrel re-exports stringtables", () => {
  assert.match(readFileSync(join(root, "index.d.ts"), "utf8"), /export \* from "\.\/stringtables";/);
});

// packages/sdk/test/hud-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./hud", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./hud"], "expected a ./hud export subpath");
});

test("hud.d.ts declares Hud and its methods", () => {
  const dts = readFileSync(join(root, "hud.d.ts"), "utf8");
  assert.match(dts, /export declare const Hud: \{/);
  for (const m of ["hide(", "show(", "set(", "reset(", "resetAll(", "hidden(", "hiddenByAll(", "isHidden("])
    assert.ok(dts.includes(m), `expected ${m}`);
});

test("the root barrel re-exports hud", () => {
  assert.match(readFileSync(join(root, "index.d.ts"), "utf8"), /export \* from "\.\/hud";/);
});

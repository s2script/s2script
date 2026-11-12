// packages/sdk/test/addons-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./addons", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./addons"], "expected a ./addons export subpath");
});

test("addons.d.ts declares Addons and its methods", () => {
  const dts = readFileSync(join(root, "addons.d.ts"), "utf8");
  assert.match(dts, /export declare const Addons: \{/);
  for (const m of ["status(", "mount(", "unmount(", "mounted(", "setExtraAddons(", "setClientAddons(",
                   "clientAddons(", "configure(", "clearClientCache(", "currentMap(", "owner("])
    assert.ok(dts.includes(m), `expected ${m}`);
});

test("the root barrel re-exports addons", () => {
  assert.match(readFileSync(join(root, "index.d.ts"), "utf8"), /export \* from "\.\/addons";/);
});

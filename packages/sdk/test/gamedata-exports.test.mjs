// packages/sdk/test/gamedata-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./unsafe", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./unsafe"], "expected an ./unsafe export subpath");
});

test("unsafe.d.ts declares an augmentable EngineCalls and Engine", () => {
  const dts = readFileSync(join(root, "unsafe.d.ts"), "utf8");
  assert.match(dts, /export interface EngineCalls/);
  assert.match(dts, /export declare const Engine/);
  assert.match(dts, /status\(/);
});

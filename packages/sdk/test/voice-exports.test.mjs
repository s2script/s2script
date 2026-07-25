// packages/sdk/test/voice-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./voice", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./voice"], "expected a ./voice export subpath");
});

test("voice.d.ts declares Voice and VoiceStats", () => {
  const dts = readFileSync(join(root, "voice.d.ts"), "utf8");
  assert.match(dts, /export interface VoiceStats/);
  assert.match(dts, /export declare const Voice/);
  assert.match(dts, /setAudibleTo\(/);
  assert.match(dts, /rewrites/);
});

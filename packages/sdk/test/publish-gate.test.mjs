/**
 * TDD test: publishes ⇒ types gate.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/publish-gate.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertPublishesTypes, hasPublishes } from "../src/publish-gate.ts";

function dirWith(files) {
  const d = mkdtempSync(join(tmpdir(), "s2gate-"));
  for (const [name, body] of Object.entries(files)) writeFileSync(join(d, name), body);
  return d;
}

test("hasPublishes: recognises self, a non-empty map, and nothing", () => {
  assert.equal(hasPublishes("self"), true);
  assert.equal(hasPublishes({ "@x/y": "^1.0.0" }), true);
  assert.equal(hasPublishes({}), false);
  assert.equal(hasPublishes(undefined), false);
  assert.equal(hasPublishes(null), false);
  assert.equal(hasPublishes(""), false);
});

test("a plugin that publishes nothing passes with no contract", () => {
  const d = dirWith({});
  const r = assertPublishesTypes({ s2script: {} }, d);
  assert.equal(r.ok, true);
  assert.equal(r.typesPath, null);
});

test("publishes with a valid contract resolves an absolute types path", () => {
  const d = dirWith({ "api.d.ts": "export declare function z(): void;\n" });
  const r = assertPublishesTypes({ types: "api.d.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, true);
  assert.equal(r.typesPath, join(d, "api.d.ts"));
});

test("publishes without a types field is a named error", () => {
  const d = dirWith({});
  const r = assertPublishesTypes({ s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /"types" is missing/);
});

test("publishes pointing at a non-.d.ts is a named error", () => {
  const d = dirWith({ "api.ts": "export function z() {}\n" });
  const r = assertPublishesTypes({ types: "api.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /must be a \.d\.ts file/);
});

test("publishes pointing at a missing file is a named error", () => {
  const d = dirWith({});
  const r = assertPublishesTypes({ types: "api.d.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /types file not found/);
});

test("publishes pointing at an empty file is a named error", () => {
  const d = dirWith({ "api.d.ts": "" });
  const r = assertPublishesTypes({ types: "api.d.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /empty/);
});

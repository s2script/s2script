import { test } from "node:test";
import assert from "node:assert/strict";
import { stripJsonComments } from "../src/gamedata/jsonc.ts";

test("strips trailing line comments (the style gamedata/core/*.jsonc uses)", () => {
  const src = '{\n  "a": 1, // the sig\n  "b": 2\n}';
  assert.deepEqual(JSON.parse(stripJsonComments(src)), { a: 1, b: 2 });
});

test("strips full-line and block comments", () => {
  const src = '{\n  // leading\n  "a": 1,\n  /* block\n     spanning */ "b": 2\n}';
  assert.deepEqual(JSON.parse(stripJsonComments(src)), { a: 1, b: 2 });
});

test("does NOT strip // or /* inside string literals", () => {
  const src = '{ "url": "http://x/y", "pat": "55 48 /* not a comment */ 89" }';
  const out = JSON.parse(stripJsonComments(src));
  assert.equal(out.url, "http://x/y");
  assert.equal(out.pat, "55 48 /* not a comment */ 89");
});

test("honours escapes so a trailing backslash does not swallow the string", () => {
  const src = '{ "a": "back\\\\slash", "b": 2 } // tail';
  assert.deepEqual(JSON.parse(stripJsonComments(src)), { a: "back\\slash", b: 2 });
});

test("preserves line count so JSON.parse positions still line up", () => {
  const src = '{\n// one\n/* two\nthree */\n"a": 1\n}';
  assert.equal(stripJsonComments(src).split("\n").length, src.split("\n").length);
});

test("does not accept trailing commas (nlohmann rejects them too)", () => {
  assert.throws(() => JSON.parse(stripJsonComments('{ "a": 1, }')));
});

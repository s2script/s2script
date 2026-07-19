import { test } from "node:test";
import assert from "node:assert";
import { validateConfigBlock } from "../src/config-validate.ts";

test("valid config block → no errors", () => {
  assert.deepEqual(validateConfigBlock({
    greeting: { type: "string", default: "hi" },
    n: { type: "int", default: 3 }, f: { type: "float", default: 1.5 }, b: { type: "bool", default: true },
  }), []);
});
test("default not matching type → an error naming the key", () => {
  const errs = validateConfigBlock({ n: { type: "int", default: "oops" } });
  assert.equal(errs.length, 1);
  assert.match(errs[0], /n.*int/);
});
test("int rejects a non-integer default", () => {
  assert.equal(validateConfigBlock({ n: { type: "int", default: 1.5 } }).length, 1);
});
test("unknown type → an error (a string type key still classifies as a decl)", () => {
  const errs = validateConfigBlock({ x: { type: "date", default: 0 } });
  assert.equal(errs.length, 1);
  assert.match(errs[0], /unknown type/);
});

test("a section recurses and validates nested decls", () => {
  assert.deepEqual(validateConfigBlock({
    top: { type: "bool", default: true },
    sect: { inner: { type: "int", default: 5 }, deeper: { leaf: { type: "string", default: "x" } } },
  }), []);
  // A bad nested decl is reported with its dotted path.
  const errs = validateConfigBlock({ sect: { inner: { type: "int", default: "oops" } } });
  assert.equal(errs.length, 1);
  assert.match(errs[0], /sect\.inner.*int/);
});

test("enum + min together → error (mutually exclusive)", () => {
  const errs = validateConfigBlock({ n: { type: "int", default: 1, enum: [1, 2], min: 0 } });
  assert.ok(errs.some((e) => /mutually exclusive/.test(e)));
});

test("dotted key → error", () => {
  const errs = validateConfigBlock({ "a.b": { type: "string", default: "x" } });
  assert.equal(errs.length, 1);
  assert.match(errs[0], /must not contain '\.'/);
});

test("default outside enum → error", () => {
  const errs = validateConfigBlock({ mode: { type: "string", default: "insane", enum: ["easy", "hard"] } });
  assert.ok(errs.some((e) => /not one of enum/.test(e)));
});

test("valid enum + default in set → no errors", () => {
  assert.deepEqual(validateConfigBlock({ mode: { type: "string", default: "easy", enum: ["easy", "hard"] } }), []);
});

test("min/max: default within range ok, outside → error", () => {
  assert.deepEqual(validateConfigBlock({ n: { type: "int", default: 5, min: 0, max: 10 } }), []);
  const errs = validateConfigBlock({ n: { type: "int", default: 99, min: 0, max: 10 } });
  assert.ok(errs.some((e) => /above max/.test(e)));
});

test("enum on float → error (string|int only)", () => {
  const errs = validateConfigBlock({ f: { type: "float", default: 1.0, enum: [1.0, 2.0] } });
  assert.ok(errs.some((e) => /string\|int only/.test(e)));
});

test("sensitive must be a boolean", () => {
  assert.deepEqual(validateConfigBlock({ pw: { type: "string", default: "", sensitive: true } }), []);
  const errs = validateConfigBlock({ pw: { type: "string", default: "", sensitive: "yes" } });
  assert.ok(errs.some((e) => /'sensitive' must be a boolean/.test(e)));
});

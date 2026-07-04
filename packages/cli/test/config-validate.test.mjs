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
test("unknown type → an error", () => {
  assert.equal(validateConfigBlock({ x: { type: "date", default: 0 } }).length, 1);
});

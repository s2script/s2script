import { test } from "node:test";
import assert from "node:assert";
import { parseFlag, hasFlag, positionals } from "../src/cli/args.ts";

test("parseFlag: --k v and --k=v forms; missing -> undefined", () => {
  assert.equal(parseFlag(["--game", "cs2"], "--game"), "cs2");
  assert.equal(parseFlag(["--game=cs2"], "--game"), "cs2");
  assert.equal(parseFlag(["build"], "--game"), undefined);
  assert.equal(parseFlag(["--game", "--next"], "--game"), undefined, "next flag is not the value");
});

test("hasFlag", () => {
  assert.equal(hasFlag(["--ci"], "--ci"), true);
  assert.equal(hasFlag(["deploy"], "--ci"), false);
});

test("positionals: skips flags and the value after a value-flag", () => {
  assert.deepEqual(positionals(["deploy", "dir", "--registry", "u"], ["--registry"]), ["deploy", "dir"]);
  assert.deepEqual(positionals(["a", "--registry=u", "b"], ["--registry"]), ["a", "b"]);
  assert.deepEqual(positionals(["-y", "x"], []), ["x"]);
});

/**
 * Bundle workspaces: root validation, the member pins, and the deploy manifest.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/bundle.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";

import {
  bundleManifest,
  bundleProblems,
  isBundleWorkspace,
  readBundle,
  sameMembers,
} from "../src/workspace/bundle.ts";
import { preflightProblems } from "../src/workspace/preflight.ts";

function plugin(name, version = "1.0.0", extra = {}) {
  const bare = name.split("/").pop();
  return {
    name,
    version,
    dir: `/ws/plugins/${bare}`,
    relDir: `plugins/${bare}`,
    pkg: { name, version, main: "src/plugin.ts" },
    entry: "src/plugin.ts",
    private: false,
    ...extra,
  };
}

function ws(rootExtra = {}, plugins = [plugin("@edge/jb-core"), plugin("@edge/jb-perks")]) {
  return {
    root: "/ws",
    pkg: {
      name: "@edge/jb",
      version: "1.0.0",
      workspaces: ["plugins/*"],
      ...rootExtra,
      s2script: { workspace: { plugins: ["plugins/*"] }, kind: "bundle", ...(rootExtra.s2script ?? {}) },
    },
    plugins,
    libs: [],
  };
}

test("bundle: a plain workspace is not a bundle and has no bundle problems", () => {
  const plain = ws();
  delete plain.pkg.s2script.kind;
  assert.equal(isBundleWorkspace(plain), false);
  assert.deepEqual(bundleProblems(plain), []);
  assert.equal(readBundle(plain), null);
});

test("bundle: a coherent bundle root passes and reads its members", () => {
  const w = ws({ s2script: { bundle: { optional: ["@edge/jb-perks"] } } });
  assert.deepEqual(bundleProblems(w), []);
  const b = readBundle(w);
  assert.equal(b.name, "@edge/jb");
  assert.deepEqual(
    b.members.map((m) => m.name),
    ["@edge/jb-core", "@edge/jb-perks"],
  );
  assert.deepEqual([...b.optional], ["@edge/jb-perks"]);
});

test("bundle: private plugins are built but never members", () => {
  const w = ws({}, [plugin("@edge/jb-core"), plugin("@edge/jb-dev", "0.0.1", { private: true })]);
  assert.deepEqual(
    readBundle(w).members.map((m) => m.name),
    ["@edge/jb-core"],
  );
});

test("bundle: the manifest pins every member at its local version, flagging optional ones", () => {
  const w = ws({ s2script: { bundle: { optional: ["@edge/jb-perks"] } } }, [
    plugin("@edge/jb-core", "1.2.0"),
    plugin("@edge/jb-perks", "0.4.1"),
  ]);
  assert.deepEqual(bundleManifest(readBundle(w)), {
    id: "@edge/jb",
    version: "1.0.0",
    kind: "bundle",
    members: {
      "@edge/jb-core": { version: "1.2.0" },
      "@edge/jb-perks": { version: "0.4.1", optional: true },
    },
  });
});

for (const [label, root, plugins, pattern] of [
  ["a private root", { private: true }, undefined, /"private": true/],
  ["a root with main", { main: "index.ts" }, undefined, /remove "main"/],
  ["a root with no version", { version: undefined }, undefined, /needs a "version"/],
  ["no non-private members", {}, [plugin("@edge/jb-core", "1.0.0", { private: true })], /at least one non-private/],
  ["a member in another scope", {}, [plugin("@edge/jb-core"), plugin("@other/x")], /must share a scope/],
  ["an unscoped member of a scoped bundle", {}, [plugin("jb-core")], /must share a scope/],
  ["optional naming a non-member", { s2script: { bundle: { optional: ["@edge/nope"] } } }, undefined, /not a non-private/],
  ["optional that is not an array", { s2script: { bundle: { optional: "@edge/jb-core" } } }, undefined, /must be an array/],
  [
    "only optional members",
    { s2script: { bundle: { optional: ["@edge/jb-core", "@edge/jb-perks"] } } },
    undefined,
    /at least one required member/,
  ],
]) {
  test(`bundle: refuses ${label}`, () => {
    const problems = bundleProblems(ws(root, plugins));
    assert.ok(
      problems.some((p) => pattern.test(p)),
      `expected a problem matching ${pattern}, got:\n${problems.join("\n")}`,
    );
  });
}

test("bundle: preflight reports bundle problems alongside the rest", () => {
  const problems = preflightProblems(ws({ private: true }));
  assert.ok(problems.some((p) => /"private": true/.test(p)));
});

test("bundle: sameMembers ignores key order but not versions or the optional flag", () => {
  const a = { "@e/a": { version: "1.0.0" }, "@e/b": { version: "1.0.0", optional: true } };
  assert.ok(sameMembers(a, { "@e/b": { version: "1.0.0", optional: true }, "@e/a": { version: "1.0.0" } }));
  assert.ok(!sameMembers(a, { "@e/a": { version: "1.0.1" }, "@e/b": { version: "1.0.0", optional: true } }));
  assert.ok(!sameMembers(a, { "@e/a": { version: "1.0.0" }, "@e/b": { version: "1.0.0" } }));
  assert.ok(!sameMembers(a, { "@e/a": { version: "1.0.0" } }));
});

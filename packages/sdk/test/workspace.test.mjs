/**
 * TDD test: the workspace model (design spec 2026-07-27 §4) — glob matching, discovery walk-up,
 * the three §4.3 rules, the topological order ported from loader.rs, and the §5.2 range gate.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/workspace.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { matchGlob, matchSegment, splitSegments, hasMagic } from "../src/workspace/glob.ts";
import { findWorkspaceRoot, loadWorkspace, expandDirGlob } from "../src/workspace/workspace.ts";
import {
  topoOrder,
  orderPlugins,
  graphNodes,
  checkSiblingRanges,
  formatRangeViolations,
  satisfiesRange,
} from "../src/workspace/graph.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "fixtures");

// ---------------------------------------------------------------------------
// glob.ts
// ---------------------------------------------------------------------------

test("glob: `*` matches exactly one segment and never crosses a separator", () => {
  assert.ok(matchGlob("plugins/*", "plugins/basechat"));
  assert.ok(!matchGlob("plugins/*", "plugins/disabled/nextmap"), "`*` must not cross `/`");
  assert.ok(!matchGlob("plugins/*", "plugins"), "`*` needs a segment to match");
  assert.ok(matchGlob("plugins/disabled/*", "plugins/disabled/nextmap"));
});

test("glob: `*` composes with literals inside a segment", () => {
  assert.ok(matchGlob("plugins/sh*", "plugins/shop"));
  assert.ok(!matchGlob("plugins/sh*", "plugins/ranks"));
  assert.ok(matchGlob("@me/*", "@me/shop"), "a package name is matched as segments too");
  assert.ok(!matchGlob("@me/*", "@you/shop"));
});

test("glob: `**` matches zero or more segments", () => {
  assert.ok(matchGlob("plugins/**", "plugins"), "zero segments");
  assert.ok(matchGlob("plugins/**", "plugins/a"));
  assert.ok(matchGlob("plugins/**", "plugins/a/b/c"));
  assert.ok(matchGlob("**/dist", "a/b/dist"));
  assert.ok(matchGlob("**/dist", "dist"));
  assert.ok(!matchGlob("**/dist", "a/b/src"));
});

test("glob: leading ./ and trailing / are noise, not structure", () => {
  assert.deepEqual(splitSegments("./plugins/a/"), ["plugins", "a"]);
  assert.ok(matchGlob("./plugins/*", "plugins/a/"));
});

test("glob: regex metacharacters in a pattern are literals", () => {
  assert.ok(matchSegment("a.b", "a.b"));
  assert.ok(!matchSegment("a.b", "axb"), "`.` must not act as a regex wildcard");
  assert.ok(!hasMagic("plugins/a"));
  assert.ok(hasMagic("plugins/*"));
});

// ---------------------------------------------------------------------------
// workspace.ts — discovery
// ---------------------------------------------------------------------------

test("expandDirGlob: expands against directories on disk, sorted, node_modules excluded", () => {
  const root = join(fixtures, "ws-basic");
  assert.deepEqual(expandDirGlob(root, "plugins/*"), [
    "plugins/consumer",
    "plugins/disabled",
    "plugins/producer",
  ]);
  assert.deepEqual(expandDirGlob(root, "packages/*"), ["packages/shared"]);
  assert.deepEqual(expandDirGlob(root, "nope/*"), [], "a missing prefix matches nothing, it is not an error");
});

test("findWorkspaceRoot: walks UP from a nested directory to the marker", () => {
  const root = join(fixtures, "ws-basic");
  assert.equal(findWorkspaceRoot(root), root, "the root finds itself");
  assert.equal(findWorkspaceRoot(join(root, "plugins", "consumer", "src")), root);
  assert.equal(findWorkspaceRoot(join(root, "packages", "shared")), root);
});

test("findWorkspaceRoot: null outside any workspace — the backward-compatibility hinge", () => {
  // A temp dir, deliberately NOT inside the repo: once this repo dogfoods §9 its own root carries
  // the marker, and a fixture-relative assertion would then be asserting the wrong thing.
  const dir = mkdtempSync(join(tmpdir(), "s2ws-"));
  mkdirSync(join(dir, "nested", "deep"), { recursive: true });
  assert.equal(findWorkspaceRoot(join(dir, "nested", "deep")), null);
});

// ---------------------------------------------------------------------------
// workspace.ts — the three §4.3 rules
// ---------------------------------------------------------------------------

test("loadWorkspace: plugins and libs are split by the s2script.workspace.plugins globs", () => {
  const ws = loadWorkspace(join(fixtures, "ws-basic"));
  assert.deepEqual(
    ws.plugins.map((p) => p.name),
    ["@fixture/consumer", "@fixture/producer"],
    "plugins are ordered by directory (dependency order is graph.ts's job)",
  );
  assert.deepEqual(ws.libs.map((l) => l.name), ["@fixture/shared"]);
  assert.equal(ws.plugins[0].relDir, "plugins/consumer");
  assert.equal(ws.plugins[1].entry, "src/plugin.ts");
  assert.equal(ws.plugins[1].private, true, "npm's own `private` carries through — built, never published");
  assert.equal(ws.plugins[0].private, false);
});

test("§4.3 rule 1: a glob match with no package.json is skipped SILENTLY", () => {
  const ws = loadWorkspace(join(fixtures, "ws-basic"));
  // plugins/disabled/ matches `plugins/*` and is just a directory — it must not appear, and it
  // must not raise. This is the existing bash guard `[ -f "$d/package.json" ] || continue`.
  assert.ok(!ws.plugins.some((p) => p.relDir === "plugins/disabled"));
  assert.ok(!ws.libs.some((l) => l.relDir === "plugins/disabled"));
});

test("§4.3 rule 2: a plugin package.json with no entry point is a HARD ERROR", () => {
  assert.throws(
    () => loadWorkspace(join(fixtures, "ws-no-entry")),
    (e) => {
      assert.match(e.message, /has 1 problem:/);
      assert.match(e.message, /plugins\/broken: matches s2script\.workspace\.plugins but has no entry point/);
      assert.match(e.message, /set s2script\.main or main/);
      assert.ok(!/plugins\/ok/.test(e.message), "the healthy sibling is not implicated");
      return true;
    },
  );
});

test("§4.3 rule 3: a plugin dir not covered by npm workspaces is a HARD ERROR, all at once", () => {
  assert.throws(
    () => loadWorkspace(join(fixtures, "ws-uncovered")),
    (e) => {
      // Aggregation: BOTH orphans are named, not just the first (spec §11).
      assert.match(e.message, /has 2 problems:/);
      assert.match(e.message, /plugins\/orphan-a: is a plugin but is not covered by npm "workspaces"/);
      assert.match(e.message, /plugins\/orphan-b: is a plugin but is not covered by npm "workspaces"/);
      // The WHY is in the message: the failure mode is a silent degradation, not a diagnostic.
      assert.match(e.message, /silently degrades to `any`/);
      return true;
    },
  );
});

test("loadWorkspace: a directory without the marker is a named refusal", () => {
  assert.throws(
    () => loadWorkspace(join(fixtures, "hello")),
    /is not a workspace root — its package\.json has no s2script\.workspace block/,
  );
});

// ---------------------------------------------------------------------------
// graph.ts — the port of loader.rs::topo_order
// ---------------------------------------------------------------------------

test("topoOrder: a producer comes before its hard-dep consumer", () => {
  const order = topoOrder([
    { id: "b", dependsOn: ["@a/if"], publishes: [] },
    { id: "a", dependsOn: [], publishes: ["@a/if"] },
  ]);
  assert.deepEqual(order, ["a", "b"]);
});

test("topoOrder: independent plugins fall in lexicographic order (stable tie-break)", () => {
  const order = topoOrder([
    { id: "zulu", dependsOn: [], publishes: [] },
    { id: "alpha", dependsOn: [], publishes: [] },
    { id: "mike", dependsOn: [], publishes: [] },
  ]);
  assert.deepEqual(order, ["alpha", "mike", "zulu"]);
});

test("topoOrder: the ready set stays lexicographic as edges are relaxed", () => {
  // a unblocks both m and c; whichever is emitted first must be the lexicographically smaller.
  const order = topoOrder([
    { id: "m", dependsOn: ["@a/if"], publishes: [] },
    { id: "c", dependsOn: ["@a/if"], publishes: [] },
    { id: "a", dependsOn: [], publishes: ["@a/if"] },
  ]);
  assert.deepEqual(order, ["a", "c", "m"]);
});

test("topoOrder: an optional dependency imposes NO edge (it is never in dependsOn)", () => {
  // graphNodes reads pluginDependencies only, so an optional sibling cannot reorder the batch:
  // z would sort last, and does, even though a "depends" on it optionally.
  const nodes = graphNodes([
    {
      name: "a",
      version: "1.0.0",
      dir: "",
      relDir: "",
      entry: "src/plugin.ts",
      private: false,
      pkg: { s2script: { optionalPluginDependencies: { "@z/if": "^1.0.0" } } },
    },
    {
      name: "z",
      version: "1.0.0",
      dir: "",
      relDir: "",
      entry: "src/plugin.ts",
      private: false,
      pkg: { s2script: { publishes: { "@z/if": "1.0.0" } } },
    },
  ]);
  assert.deepEqual(nodes[0].dependsOn, [], "optionalPluginDependencies must not become an edge");
  assert.deepEqual(topoOrder(nodes), ["a", "z"]);
});

test("topoOrder: a plugin depending on its OWN published interface imposes no edge", () => {
  const order = topoOrder([
    { id: "z", dependsOn: ["@z/if"], publishes: ["@z/if"] },
    { id: "a", dependsOn: [], publishes: [] },
  ]);
  assert.deepEqual(order, ["a", "z"], "a self-edge would make z unreachable and trip the cycle path");
});

test("topoOrder: a dep with no in-batch producer imposes no edge (registry dependency)", () => {
  const order = topoOrder([
    { id: "b", dependsOn: ["@s2script/zones"], publishes: [] },
    { id: "a", dependsOn: [], publishes: [] },
  ]);
  assert.deepEqual(order, ["a", "b"]);
});

test("topo_cycle_falls_back_to_name_order (port of loader.rs) — WARNS, never throws", () => {
  // The exact batch from core/src/loader.rs::tests::topo_cycle_falls_back_to_name_order.
  const warnings = [];
  const order = topoOrder(
    [
      { id: "a", dependsOn: ["@b/if"], publishes: ["@a/if"] },
      { id: "b", dependsOn: ["@a/if"], publishes: ["@b/if"] },
    ],
    (m) => warnings.push(m),
  );
  assert.deepEqual(order, ["a", "b"]);
  assert.equal(warnings.length, 1, "exactly one WARN for the whole cycle");
  assert.match(warnings[0], /plugin dependency cycle \(a, b\)/);
  assert.match(warnings[0], /InterfaceUnavailable/, "the message must say what actually happens at runtime");
});

test("topoOrder: a cycle still emits every id exactly once, acyclic part first", () => {
  const order = topoOrder(
    [
      { id: "y", dependsOn: ["@z/if"], publishes: ["@y/if"] },
      { id: "z", dependsOn: ["@y/if"], publishes: ["@z/if"] },
      { id: "a", dependsOn: [], publishes: [] },
    ],
    () => {},
  );
  assert.deepEqual(order, ["a", "y", "z"]);
  assert.equal(new Set(order).size, 3, "no id may be dropped or duplicated");
});

test("orderPlugins: the fixture workspace orders the producer before its consumer", () => {
  const ws = loadWorkspace(join(fixtures, "ws-basic"));
  assert.deepEqual(
    orderPlugins(ws.plugins).map((p) => p.name),
    ["@fixture/producer", "@fixture/consumer"],
    "directory order is consumer-first; dependency order must flip it",
  );
});

// ---------------------------------------------------------------------------
// graph.ts — the §5.2 sibling range gate
// ---------------------------------------------------------------------------

test("satisfiesRange: real semver, and a malformed range is false rather than a throw", () => {
  assert.ok(satisfiesRange("2.0.1", "^2.0.0"));
  assert.ok(!satisfiesRange("3.0.0", "^2.0.0"));
  assert.ok(!satisfiesRange("1.0.0", "garbage"), "fail closed, and report it as a violation");
});

test("§5.2: a matching sibling range is clean", () => {
  const ws = loadWorkspace(join(fixtures, "ws-basic"));
  assert.deepEqual(checkSiblingRanges(ws.plugins), []);
});

test("§5.2: every violating range is reported at once, not the first", () => {
  const ws = loadWorkspace(join(fixtures, "ws-ranges"));
  const violations = checkSiblingRanges(ws.plugins);
  assert.equal(violations.length, 2);
  assert.deepEqual(
    violations.map((v) => `${v.consumer} ${v.iface} ${v.range} ${v.version}`).sort(),
    ["@me/ranks @me/shop ^2.0.0 3.0.0", "@me/warmup @me/shop ^1.0.0 3.0.0"],
  );
  const report = formatRangeViolations(violations);
  assert.match(report, /^2 dependency ranges do not match this workspace:/);
  assert.match(report, /@me\/ranks\s+pluginDependencies\["@me\/shop"\] = "\^2\.0\.0"\s+but @me\/shop is 3\.0\.0/);
  assert.match(report, /@me\/warmup\s+pluginDependencies\["@me\/shop"\] = "\^1\.0\.0"\s+but @me\/shop is 3\.0\.0/);
});

test("§5.2: an optional sibling range is NOT gated (spec names pluginDependencies only)", () => {
  // @me/warmup optionally depends on @me/ranks ^9.0.0 while the workspace ships 0.4.0. An optional
  // dep resolves to null when unsatisfied — by design — so it is not a broken-workspace claim.
  const ws = loadWorkspace(join(fixtures, "ws-ranges"));
  assert.ok(!checkSiblingRanges(ws.plugins).some((v) => v.iface === "@me/ranks"));
});

test("§5.2: a dependency no sibling publishes is left alone (registry path unchanged)", () => {
  const ws = loadWorkspace(join(fixtures, "ws-basic"));
  const plugins = ws.plugins.map((p) =>
    p.name !== "@fixture/consumer"
      ? p
      : { ...p, pkg: { ...p.pkg, s2script: { pluginDependencies: { "@s2script/zones": "^99.0.0" } } } },
  );
  assert.deepEqual(checkSiblingRanges(plugins), []);
});

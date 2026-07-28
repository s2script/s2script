/**
 * TDD test: the workspace model (design spec 2026-07-27 §4) — glob matching, discovery walk-up,
 * the three §4.3 rules, the topological order ported from loader.rs, and the §5.2 range gate.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/workspace.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
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

test("findWorkspaceRoot: an unparseable package.json the author does not own is walked PAST", () => {
  // §4.2 makes "returns null" THE backward-compatibility hinge, and the walk-up crosses
  // directories the plugin author does not control. A stray comma in an ancestor's package.json
  // used to hard-fail `s2s build ./standalone-plugin` — for a plugin in no workspace at all.
  const dir = mkdtempSync(join(tmpdir(), "s2ws-"));
  writeFileSync(join(dir, "package.json"), '{ "name": "junk", }\n');
  const pluginDir = join(dir, "standalone-plugin");
  mkdirSync(pluginDir);
  writeFileSync(
    join(pluginDir, "package.json"),
    JSON.stringify({ name: "@me/standalone", version: "1.0.0", main: "src/plugin.ts" }),
  );
  assert.equal(findWorkspaceRoot(pluginDir), null, "no workspace anywhere ⇒ single-plugin mode");
});

test("findWorkspaceRoot: an unparseable package.json that CARRIES the marker is still fatal", () => {
  // The one file whose parse failure must not be swallowed: degrading the real workspace root to
  // single-plugin mode over a stray comma is the silent failure this design exists to remove.
  const dir = mkdtempSync(join(tmpdir(), "s2ws-"));
  writeFileSync(
    join(dir, "package.json"),
    '{ "workspaces": ["plugins/*"], "s2script": { "workspace": { "plugins": ["plugins/*"] } },, }\n',
  );
  assert.throws(() => findWorkspaceRoot(join(dir, "plugins", "p")), /cannot read .*package\.json/);
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

test("§4.3 rule 3: an uncovered plugin dir WARNS and still builds", () => {
  // This was a hard error, because sibling resolution rode npm's `node_modules` symlink and an
  // uncovered plugin therefore degraded silently to an `any` stub. `typecheck.ts` no longer uses
  // the symlink at all — it maps each interface straight at the producer's contract — so the
  // failure mode that justified failing the build no longer exists, and keeping the error would
  // reject workspaces that now work. The diagnostic stays, because an uncovered directory gets no
  // install or editor resolution for its OTHER dependencies, which is nearly always an oversight.
  const warnings = [];
  const realWarn = console.warn;
  console.warn = (...a) => warnings.push(a.join(" "));
  let ws;
  try {
    ws = loadWorkspace(join(fixtures, "ws-uncovered")); // must NOT throw now
  } finally {
    console.warn = realWarn;
  }

  // Stronger than the old assertion: the plugins are not merely tolerated, they are BUILT.
  assert.deepEqual(ws.plugins.map((p) => p.relDir).sort(), ["plugins/orphan-a", "plugins/orphan-b"]);
  // Both orphans are still named — aggregation survives the demotion (§11).
  const warned = warnings.join("\n");
  assert.match(warned, /plugins\/orphan-a: is a plugin but is not listed/);
  assert.match(warned, /plugins\/orphan-b: is a plugin but is not listed/);
  // And the message now names BOTH manifests, so a pnpm user is not told to edit a field they
  // deliberately do not have.
  assert.match(warned, /pnpm-workspace\.yaml/);
});

/** A throwaway workspace root with `plugins/<dir>/package.json` written from `members`. */
function tempWorkspace(members) {
  const root = mkdtempSync(join(tmpdir(), "s2ws-"));
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify({
      name: "@fixture/tmp-ws",
      private: true,
      version: "0.0.0",
      workspaces: ["plugins/*"],
      s2script: { workspace: { plugins: ["plugins/*"] } },
    }),
  );
  for (const [dir, pkg] of Object.entries(members)) {
    mkdirSync(join(root, "plugins", dir, "src"), { recursive: true });
    writeFileSync(join(root, "plugins", dir, "package.json"), JSON.stringify(pkg, null, 2));
  }
  return root;
}

test("§4.3: two plugins sharing a `name` are a HARD ERROR, never a silent collapse", () => {
  // The verified failure mode: `topoOrder` keys indegree by name, so a two-plugin workspace loaded
  // as ONE plugin, `buildWorkspace` built one, the command exited 0, and the only trace was a
  // nonsense "plugin dependency cycle ()" pointing at the wrong problem. Rule 2 chose a hard error
  // over exactly this class of silence.
  const root = tempWorkspace({
    a: { name: "@me/dup", version: "1.0.0", main: "src/plugin.ts" },
    b: { name: "@me/dup", version: "2.0.0", main: "src/plugin.ts" },
  });
  assert.throws(
    () => loadWorkspace(root),
    (e) => {
      assert.match(e.message, /has 1 problem:/);
      assert.match(e.message, /plugins\/b: duplicate plugin name "@me\/dup" — already declared by plugins\/a/);
      assert.match(e.message, /id on the wire/);
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

test("satisfiesRange: the common cases, and a malformed range is false rather than a throw", () => {
  assert.ok(satisfiesRange("2.0.1", "^2.0.0"));
  assert.ok(!satisfiesRange("3.0.0", "^2.0.0"));
  assert.ok(!satisfiesRange("1.0.0", "garbage"), "fail closed, and report it as a violation");
});

test("satisfiesRange AGREES with interfaces.rs::version_satisfies (decision #11)", () => {
  // The engine is MAJOR-ONLY, and a full-semver gate disagreed with it in both directions. The
  // dangerous direction is a range the SDK waves through and the engine then rejects at every
  // single call — `>=1.0.0` against 3.0.0 is `satisfies() === true` but `1 != 3` in the engine.
  assert.ok(!satisfiesRange("3.0.0", ">=1.0.0"), "leading_major(range) is 1, so the engine refuses");
  assert.ok(!satisfiesRange("2.0.0", "1.x || 2.x"), "the engine reads the LEADING major only");
  // And the other direction: a gate stricter than the runtime fails builds the engine loads
  // happily, which is just as wrong (spec §5.3.0's closing paragraph).
  assert.ok(satisfiesRange("1.1.0", "^1.2.0"), "the engine loads this, so preflight must not fail it");
  assert.ok(satisfiesRange("2.0.1", "~2.9.9"), "any comparator, same major");
  assert.ok(satisfiesRange("7.2.1", "*"), "`*` is the engine's one wildcard");
  assert.ok(!satisfiesRange("1.0.0", "^x"), "no leading major on either side ⇒ false");
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

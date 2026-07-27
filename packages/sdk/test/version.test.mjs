/**
 * `s2s version` (design spec 2026-07-27 §7): the in-memory mirror that teaches changesets the
 * s2script interface graph.
 *
 * The load-bearing assertions:
 *   - a changeset on a producer CASCADES a bump to its dependent, which changesets alone cannot do
 *     because it cannot see `s2script.pluginDependencies`;
 *   - that dependent's `pluginDependencies` range is rewritten to track the new version;
 *   - and NOTHING FAKE IS EVER WRITTEN TO DISK — no mirrored dependency reaches any package.json.
 *     That last one is the central invariant of the design, so it is asserted over every
 *     package.json in the workspace rather than just the one we expect to be affected.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/version.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { loadWorkspace } from "../src/workspace/workspace.ts";
import { mirrorPackages, realPackages, siblingEdges } from "../src/version/mirror.ts";
import { rewriteSiblingRanges } from "../src/version/ranges.ts";
import { loadChangesets } from "../src/version/changesets.ts";
import { resolveVersionRoot, versionWorkspace } from "../src/version/version.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "fixtures");

/** The nearest ancestor carrying an installed `@changesets/*` — the SDK does not vendor them. */
function findNodeModules(from) {
  let dir = from;
  for (;;) {
    if (existsSync(join(dir, "node_modules", "@changesets", "config"))) return join(dir, "node_modules");
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

const nodeModules = findNodeModules(here);

/**
 * A throwaway copy of a fixture workspace — `s2s version` WRITES, so no test may run against the
 * committed fixture. `node_modules` is symlinked rather than copied because `loadChangesets`
 * deliberately resolves changesets from the workspace root (§3.6), and a temp dir under /tmp has
 * no ancestor that carries it.
 */
function scratchWorkspace(name, t) {
  const dir = mkdtempSync(join(tmpdir(), "s2s-version-"));
  cpSync(join(fixtures, name), dir, { recursive: true });
  if (nodeModules !== null) symlinkSync(nodeModules, join(dir, "node_modules"), "dir");
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

/** Every package.json in the workspace (root + members), by path. */
function allPackageJsons(root) {
  const ws = loadWorkspace(root);
  const out = new Map([[join(root, "package.json"), readJson(join(root, "package.json"))]]);
  for (const m of [...ws.plugins, ...ws.libs]) {
    const path = join(m.dir, "package.json");
    out.set(path, readJson(path));
  }
  return out;
}

// ---------------------------------------------------------------------------
// mirror.ts — step 2
// ---------------------------------------------------------------------------

test("mirror: a sibling pluginDependency becomes a dependency edge changesets understands", () => {
  const ws = loadWorkspace(join(fixtures, "ws-version"));

  assert.deepStrictEqual(siblingEdges(ws), [
    { consumer: "@fixture/vconsumer", producer: "@fixture/vproducer", iface: "@fixture/vproducer", range: "^2.0.0" },
  ]);

  const mirror = mirrorPackages(ws);
  const consumer = mirror.packages.find((p) => p.packageJson.name === "@fixture/vconsumer");
  assert.deepStrictEqual(consumer.packageJson.dependencies, { "@fixture/vproducer": "^2.0.0" });
});

test("mirror: the injection never touches the real package objects — on disk or in memory", () => {
  const ws = loadWorkspace(join(fixtures, "ws-version"));
  const real = realPackages(ws);
  mirrorPackages(ws);

  const realConsumer = real.packages.find((p) => p.packageJson.name === "@fixture/vconsumer");
  assert.strictEqual(realConsumer.packageJson.dependencies, undefined, "the real object must stay clean");
  const onDisk = readJson(join(fixtures, "ws-version", "plugins", "consumer", "package.json"));
  assert.strictEqual(onDisk.dependencies, undefined);
  assert.strictEqual(onDisk.devDependencies, undefined);
});

test("mirror: an interface version authored independently of the package version is NOT an edge", (t) => {
  // `publishes` decouples interface name AND version from the package's, and changesets never
  // versions a hand-authored interface version — cascading a package bump into that consumer's
  // range would fabricate a dependency the workspace does not actually have.
  const dir = scratchWorkspace("ws-version", t);
  const producerPath = join(dir, "plugins", "producer", "package.json");
  const producer = readJson(producerPath);
  producer.s2script.publishes = { "@fixture/legacy-iface": "1.2.0" };
  writeFileSync(producerPath, JSON.stringify(producer, null, 2) + "\n");

  const consumerPath = join(dir, "plugins", "consumer", "package.json");
  const consumer = readJson(consumerPath);
  consumer.s2script.pluginDependencies = { "@fixture/legacy-iface": "^1.2.0" };
  writeFileSync(consumerPath, JSON.stringify(consumer, null, 2) + "\n");

  assert.deepStrictEqual(siblingEdges(loadWorkspace(dir)), []);
});

// ---------------------------------------------------------------------------
// ranges.ts — step 5
// ---------------------------------------------------------------------------

test("ranges: a bump that leaves the range is rewritten even below the config threshold", (t) => {
  const dir = scratchWorkspace("ws-version", t);
  const ws = loadWorkspace(dir);
  const releases = [
    { name: "@fixture/vproducer", type: "patch", oldVersion: "2.0.1", newVersion: "3.0.0", changesets: [] },
  ];
  // "minor" is the strictest setting; leaving the declared range still forces the rewrite, exactly
  // as apply-release-plan does for a real dependency field.
  const result = rewriteSiblingRanges(ws, releases, { updateInternalDependencies: "minor", ignore: [] });

  assert.deepStrictEqual(result.rewrites, [
    {
      consumer: "@fixture/vconsumer",
      producer: "@fixture/vproducer",
      iface: "@fixture/vproducer",
      from: "^2.0.0",
      to: "^3.0.0",
    },
  ]);
  const consumer = readJson(join(dir, "plugins", "consumer", "package.json"));
  assert.strictEqual(consumer.s2script.pluginDependencies["@fixture/vproducer"], "^3.0.0");
});

test("ranges: updateInternalDependencies governs an in-range bump", (t) => {
  const dir = scratchWorkspace("ws-version", t);
  const ws = loadWorkspace(dir);
  const releases = [
    { name: "@fixture/vproducer", type: "patch", oldVersion: "2.0.1", newVersion: "2.0.2", changesets: [] },
  ];

  // "minor" threshold, patch bump, still inside ^2.0.0 → left alone.
  assert.deepStrictEqual(
    rewriteSiblingRanges(ws, releases, { updateInternalDependencies: "minor", ignore: [] }).rewrites,
    [],
  );
  assert.strictEqual(
    readJson(join(dir, "plugins", "consumer", "package.json")).s2script.pluginDependencies["@fixture/vproducer"],
    "^2.0.0",
  );

  // Same bump under the "patch" threshold → rewritten.
  const rewritten = rewriteSiblingRanges(ws, releases, { updateInternalDependencies: "patch", ignore: [] });
  assert.deepStrictEqual(rewritten.rewrites.map((r) => r.to), ["^2.0.2"]);
});

test("ranges: a range that cannot be rebuilt is reported, never guessed at", (t) => {
  const dir = scratchWorkspace("ws-version", t);
  const consumerPath = join(dir, "plugins", "consumer", "package.json");
  const consumer = readJson(consumerPath);
  consumer.s2script.pluginDependencies["@fixture/vproducer"] = ">=2.0.0 <3.0.0";
  writeFileSync(consumerPath, JSON.stringify(consumer, null, 2) + "\n");

  const releases = [
    { name: "@fixture/vproducer", type: "major", oldVersion: "2.0.1", newVersion: "3.0.0", changesets: [] },
  ];
  const result = rewriteSiblingRanges(loadWorkspace(dir), releases, {
    updateInternalDependencies: "patch",
    ignore: [],
  });

  assert.deepStrictEqual(result.rewrites, []);
  assert.strictEqual(result.skips.length, 1);
  assert.match(result.skips[0].reason, /simple <operator><version>/);
  assert.strictEqual(
    readJson(consumerPath).s2script.pluginDependencies["@fixture/vproducer"],
    ">=2.0.0 <3.0.0",
    "the authored range must survive untouched",
  );
});

// ---------------------------------------------------------------------------
// changesets.ts — the coupling gate
// ---------------------------------------------------------------------------

test("changesets: the workspace's own @changesets/* load, version-gated", async (t) => {
  if (nodeModules === null) return t.skip("no installed @changesets/* to resolve");
  const dir = scratchWorkspace("ws-version", t);
  const cs = await loadChangesets(dir);

  for (const fn of ["assembleReleasePlan", "applyReleasePlan", "readConfig", "readChangesets", "readPreState"]) {
    assert.strictEqual(typeof cs[fn], "function", `${fn} must resolve through the CJS interop`);
  }
  // The gate reports what it accepted: a wrong plan should be traceable to a version, not guessed at.
  assert.ok(cs.versions["@changesets/assemble-release-plan"], "the accepted versions are reported");
});

test("changesets: a missing @changesets/* fails with a NAMED reason", async (t) => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-version-bare-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "bare", private: true }) + "\n");

  await assert.rejects(
    () => loadChangesets(dir),
    (e) => /not installed: @changesets\//.test(e.message) && /INTERNALS/.test(e.message),
  );
});

// ---------------------------------------------------------------------------
// version.ts — the whole five-step flow
// ---------------------------------------------------------------------------

test("version: refuses by name outside a workspace", (t) => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-version-none-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  assert.throws(() => resolveVersionRoot(dir), /workspace command/);
});

test("version: a workspace with no .changeset/ fails with a remedy, not a bare ENOENT", async (t) => {
  const dir = scratchWorkspace("ws-basic", t);
  await assert.rejects(() => versionWorkspace(dir), /changeset init/);
});

test("version: a changeset on a producer cascades a bump to its dependent AND rewrites its range", async (t) => {
  if (nodeModules === null) return t.skip("no installed @changesets/* to resolve");
  const dir = scratchWorkspace("ws-version", t);

  const result = await versionWorkspace(dir);

  assert.strictEqual(result.changesetCount, 1);

  const producer = readJson(join(dir, "plugins", "producer", "package.json"));
  const consumer = readJson(join(dir, "plugins", "consumer", "package.json"));

  assert.strictEqual(producer.version, "3.0.0", "the changeset's own major bump");
  assert.notStrictEqual(
    consumer.version,
    "1.3.0",
    "the dependent MUST be bumped — changesets cannot see s2script.pluginDependencies, so this is " +
      "exactly what the in-memory mirror exists to produce",
  );
  assert.strictEqual(consumer.version, "1.3.1");
  assert.strictEqual(
    consumer.s2script.pluginDependencies["@fixture/vproducer"],
    "^3.0.0",
    "step 5 must track the producer's new version",
  );

  // A lib nobody released is left exactly as authored.
  assert.strictEqual(readJson(join(dir, "packages", "shared", "package.json")).version, "1.0.0");
  // The applied changeset is consumed, as `changeset version` does.
  assert.ok(!existsSync(join(dir, ".changeset", "heavy-pandas-shout.md")));
});

test("version: NO mirrored dependency ever reaches a package.json on disk", async (t) => {
  if (nodeModules === null) return t.skip("no installed @changesets/* to resolve");
  const dir = scratchWorkspace("ws-version", t);

  await versionWorkspace(dir);

  // The central invariant of §7: the mirror is how changesets is shown the s2script graph, and it
  // must remain invisible. A regression here would silently rewrite the authoring format.
  for (const [path, pkg] of allPackageJsons(dir)) {
    for (const field of ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"]) {
      const deps = pkg[field];
      if (deps === undefined) continue;
      assert.ok(
        !Object.hasOwn(deps, "@fixture/vproducer"),
        `${path} gained a mirrored ${field} entry for @fixture/vproducer`,
      );
    }
  }
  // The consumer had no npm dependency fields at all; it must still have none.
  const consumer = readJson(join(dir, "plugins", "consumer", "package.json"));
  assert.strictEqual(consumer.dependencies, undefined);
  assert.strictEqual(consumer.devDependencies, undefined);
});

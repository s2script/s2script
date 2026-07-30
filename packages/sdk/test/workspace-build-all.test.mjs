/**
 * `s2s build` in workspace mode (design spec 2026-07-27 §5.1, §5.4): `--filter` selection,
 * `--stamp-version`, and the collect-all build.
 *
 * The two load-bearing behaviours here are the ones a reasonable-looking implementation gets
 * wrong, so each is written to fail in that case:
 *   - a `--filter` that matches nothing must ERROR, never quietly build zero plugins,
 *   - a failing plugin must NOT stop its siblings, and must still make the command exit non-zero.
 *
 * The last test drives the real CLI in a subprocess, because the exit code is the part of
 * collect-all that no in-process assertion can prove.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/workspace-build-all.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { cpSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import {
  buildWorkspace,
  collectFilters,
  formatBuildFailures,
  selectLibraries,
  selectPlugins,
  stampWorkspaceVersion,
} from "../src/workspace/build-all.ts";
import { loadWorkspace } from "../src/workspace/workspace.ts";
import { preflightProblems } from "../src/workspace/preflight.ts";
import { openZip } from "./zip.mjs";

const execFileAsync = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = join(here, "..");
const packagesDir = join(here, "..", "..");
const fx = (n) => join(here, "fixtures", n);

/** A throwaway copy of a fixture workspace — the stamp tests REWRITE package.json files. */
function copyFixture(name) {
  const dir = join(mkdtempSync(join(tmpdir(), "s2s-ws-")), name);
  cpSync(fx(name), dir, { recursive: true });
  return dir;
}

const readPkg = (dir) => JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));

// ---------------------------------------------------------------------------
// §5.4 — --filter
// ---------------------------------------------------------------------------

test("--filter is repeatable in both the spaced and the `=` form", () => {
  assert.deepEqual(collectFilters(["--filter", "@me/shop", "--filter=@me/ranks", "--other"]), [
    "@me/shop",
    "@me/ranks",
  ]);
  assert.deepEqual(collectFilters(["build", "."]), []);
});

test("--filter with no pattern is a named refusal, not a silently ignored flag", () => {
  assert.throws(() => collectFilters(["--filter"]), /--filter requires a pattern/);
  assert.throws(() => collectFilters(["--filter", "--ci"]), /--filter requires a pattern/);
  assert.throws(() => collectFilters(["--filter="]), /--filter= requires a pattern/);
});

test("--filter matches a PACKAGE NAME, exactly or by scope wildcard", () => {
  const ws = loadWorkspace(fx("ws-ranges"));
  assert.deepEqual(selectPlugins(ws, ["@me/shop"]).map((p) => p.name), ["@me/shop"]);
  assert.deepEqual(
    selectPlugins(ws, ["@me/*"]).map((p) => p.name),
    ["@me/ranks", "@me/shop", "@me/warmup"],
  );
  // Repeatable: the union, deduped, still in workspace order.
  assert.deepEqual(
    selectPlugins(ws, ["@me/warmup", "@me/shop", "@me/shop"]).map((p) => p.name),
    ["@me/shop", "@me/warmup"],
  );
});

test("no --filter selects every plugin", () => {
  const ws = loadWorkspace(fx("ws-ranges"));
  assert.equal(selectPlugins(ws, []).length, ws.plugins.length);
});

test("a `/` pattern matching no package name is retried as a PATH GLOB from the root", () => {
  const ws = loadWorkspace(fx("ws-ranges"));
  assert.deepEqual(selectPlugins(ws, ["plugins/sh*"]).map((p) => p.name), ["@me/shop"]);
  assert.deepEqual(selectPlugins(ws, ["plugins/shop"]).map((p) => p.name), ["@me/shop"]);
  assert.deepEqual(
    selectPlugins(ws, ["plugins/*"]).map((p) => p.name),
    ["@me/ranks", "@me/shop", "@me/warmup"],
  );
});

test("the NAME form wins: a pattern that matches a name is never reinterpreted as a path", () => {
  // `@me/*` matches three names. If the path retry ran anyway it would match nothing extra here,
  // so prove precedence the only way that can differ: a name match must not require a directory.
  const ws = loadWorkspace(fx("ws-ranges"));
  const byName = selectPlugins(ws, ["@me/shop"]);
  assert.deepEqual(byName.map((p) => p.relDir), ["plugins/shop"]);
  // And a bare (slash-less) pattern is NEVER retried as a path, so it cannot match a directory.
  assert.throws(() => selectPlugins(ws, ["shop"]), /--filter matched no plugin/);
});

test("a --filter matching nothing is an ERROR, not an empty build", () => {
  const ws = loadWorkspace(fx("ws-ranges"));
  assert.throws(
    () => selectPlugins(ws, ["@me/shpo"]),
    (e) => {
      assert.match(e.message, /--filter matched no plugin: "@me\/shpo"/);
      // The message has to be actionable: name what the workspace actually holds.
      assert.match(e.message, /@me\/shop \(plugins\/shop\)/);
      return true;
    },
  );
  assert.throws(() => selectPlugins(ws, ["plugins/nope*"]), /--filter matched no plugin/);
});

test("every unmatched --filter is reported at once, not just the first", () => {
  const ws = loadWorkspace(fx("ws-ranges"));
  assert.throws(
    () => selectPlugins(ws, ["@me/nope", "@me/shop", "plugins/gone*"]),
    /matched no plugin: "@me\/nope", "plugins\/gone\*"/,
  );
});

// ---------------------------------------------------------------------------
// §5.4 — --stamp-version
// ---------------------------------------------------------------------------

test("--stamp-version rewrites every plugin version AND the sibling ranges it would break", () => {
  const root = copyFixture("ws-ranges");
  const stamp = stampWorkspaceVersion(loadWorkspace(root), "1.5.0");

  assert.deepEqual(stamp.stamped.sort(), ["@me/ranks", "@me/shop", "@me/warmup"]);
  assert.equal(readPkg(join(root, "plugins", "shop")).version, "1.5.0");
  assert.equal(readPkg(join(root, "plugins", "ranks")).version, "1.5.0");

  // "^2.0.0" is false the moment @me/shop is stamped to 1.5.0, so it moves with it — the
  // comparator the author wrote is preserved.
  assert.equal(readPkg(join(root, "plugins", "ranks")).s2script.pluginDependencies["@me/shop"], "^1.5.0");
  const warmup = readPkg(join(root, "plugins", "warmup"));
  // Minimal churn: 1.5.0 still satisfies "^1.0.0", so that range is left exactly as authored.
  assert.equal(warmup.s2script.pluginDependencies["@me/shop"], "^1.0.0");
  // @me/ranks publishes no interface at all, so this optional dep is a REGISTRY dependency as far
  // as the workspace is concerned — the same rule §5.2 gates by, and it is not touched.
  assert.equal(warmup.s2script.optionalPluginDependencies["@me/ranks"], "^9.0.0");

  // The invariant the whole rewrite exists for: a stamped workspace preflights clean, where
  // before the stamp this very fixture reported two §5.2 violations.
  assert.equal(preflightProblems(loadWorkspace(fx("ws-ranges"))).length, 1);
  assert.deepEqual(preflightProblems(loadWorkspace(root)), []);
});

test("--stamp-version rewrites an OPTIONAL sibling range too", () => {
  // An optional dep resolves to `Interface | null` and is still range-checked at call time, so a
  // range left pointing at a version the workspace no longer ships would silently null out a
  // producer sitting right there on disk.
  const root = copyFixture("ws-basic");
  stampWorkspaceVersion(loadWorkspace(root), "1.5.0");
  const consumer = readPkg(join(root, "plugins", "consumer")).s2script;
  assert.equal(consumer.pluginDependencies["@fixture/producer"], "^1.5.0");
  assert.equal(consumer.optionalPluginDependencies["@fixture/producer"], "^1.5.0");
});

test("--stamp-version rewrites a comparator range the ENGINE rejects (decision #11)", () => {
  // ">=1.0.0" reads as satisfied under full semver for any later version, so the rewrite was
  // skipped and `--stamp-version 3.0.0` left the range in place — while
  // `interfaces.rs::version_satisfies` computes leading_major 1 != 3 and throws on every call.
  // The stamp's whole invariant is "the §5.2 gate passes afterwards", and it did not.
  const root = copyFixture("ws-ranges");
  const ranksPkg = join(root, "plugins", "ranks", "package.json");
  const ranks = JSON.parse(readFileSync(ranksPkg, "utf8"));
  ranks.s2script.pluginDependencies["@me/shop"] = ">=1.0.0";
  writeFileSync(ranksPkg, `${JSON.stringify(ranks, null, 2)}\n`);

  const stamp = stampWorkspaceVersion(loadWorkspace(root), "3.0.0");
  assert.equal(readPkg(join(root, "plugins", "ranks")).s2script.pluginDependencies["@me/shop"], ">=3.0.0");
  assert.ok(
    stamp.rewrites.some((r) => r.consumer === "@me/ranks" && r.from === ">=1.0.0" && r.to === ">=3.0.0"),
    `the rewrite must be reported, got ${JSON.stringify(stamp.rewrites)}`,
  );
  assert.deepEqual(preflightProblems(loadWorkspace(root)), [], "a stamped workspace preflights clean");
});

test("--stamp-version refuses a range: a manifest carries a version, never a range", () => {
  const ws = loadWorkspace(fx("ws-ranges"));
  assert.throws(() => stampWorkspaceVersion(ws, "^1.5.0"), /needs a concrete version/);
  // A release tag's leading `v` is stripped, exactly as build-base-plugins.sh always did.
  const root = copyFixture("ws-ranges");
  assert.equal(stampWorkspaceVersion(loadWorkspace(root), "v1.5.0").version, "1.5.0");
});

// ---------------------------------------------------------------------------
// §5.1 — collect-all
// ---------------------------------------------------------------------------

test("one broken plugin does not hide the others: every plugin is attempted", async () => {
  const ws = loadWorkspace(fx("ws-buildall"));
  const result = await buildWorkspace({ workspace: ws, packagesDir });

  // All three attempted, in dependency (here: lexicographic) order — NOT stopped at the failure.
  assert.deepEqual(result.outcomes.map((o) => o.name), [
    "@fixture/alpha",
    "@fixture/broken",
    "@fixture/zulu",
  ]);
  assert.deepEqual(result.built.map((o) => o.name), ["@fixture/alpha", "@fixture/zulu"]);
  assert.deepEqual(result.failed.map((o) => o.name), ["@fixture/broken"]);

  // The artifacts of the plugins that DID build are on disk, at the unchanged location.
  for (const o of result.built) {
    assert.ok(existsSync(o.outPath), `${o.name} produced ${o.outPath}`);
    assert.match(o.outPath, /[\\/]dist[\\/]_fixture_\w+\.s2sp$/);
  }
  // The one that failed produced no path and carries its own named reason.
  assert.equal(result.failed[0].outPath, undefined);
  assert.match(result.failed[0].error, /invalid s2script\.config/);

  const summary = formatBuildFailures(result.failed, result.outcomes.length);
  assert.match(summary, /1 of 3 plugins failed to build:/);
  assert.match(summary, /@fixture\/broken \(plugins\/broken\)/);
  assert.match(summary, /invalid s2script\.config/);
});

test("--filter narrows what is built", async () => {
  const ws = loadWorkspace(fx("ws-buildall"));
  const result = await buildWorkspace({ workspace: ws, packagesDir, filters: ["@fixture/zulu"] });
  assert.deepEqual(result.outcomes.map((o) => o.name), ["@fixture/zulu"]);
  assert.equal(result.failed.length, 0);
});

test("`s2s build` at a workspace root exits NON-ZERO when any plugin failed", async () => {
  let err;
  try {
    await execFileAsync(
      process.execPath,
      ["--experimental-strip-types", "--no-warnings", "src/cli.ts", "build", fx("ws-buildall")],
      { cwd: sdkRoot, env: { ...process.env, CI: "1" } },
    );
  } catch (e) {
    err = e;
  }
  assert.ok(err, "the command must not exit 0 with a failed plugin");
  assert.equal(err.code, 1);
  // stdout stays machine-readable: nothing but the artifact paths of what actually built.
  const paths = err.stdout.split("\n").filter((l) => l.trim() !== "");
  assert.equal(paths.length, 2, `expected 2 artifact paths, got ${JSON.stringify(paths)}`);
  for (const p of paths) assert.ok(existsSync(p), `${p} exists`);
  // …and the failure is named on stderr, not swallowed.
  assert.match(err.stderr, /1 of 3 plugins failed to build:/);
  assert.match(err.stderr, /@fixture\/broken/);
});

// ---------------------------------------------------------------------------
// Task 8 revision: a workspace-declared library (`s2script.kind === "library"`) is a plain
// `ws.libs` member, not a `ws.plugins` one — `selectPlugins`/`orderPlugins` never see it, so
// without this it would never be built at all, even though `s2s add`/`s2s deploy` need a real
// `.s2lib` to vendor/publish.
// ---------------------------------------------------------------------------

test("a workspace-declared library builds to its own .s2lib alongside its plugin consumer", async () => {
  const ws = loadWorkspace(fx("ws-library"));
  assert.deepEqual(ws.plugins.map((p) => p.name), ["@fixture/uses-lib"]);
  assert.deepEqual(ws.libs.map((m) => m.name), ["@fixture/greetlib"]);

  const result = await buildWorkspace({ workspace: ws, packagesDir });
  assert.deepEqual(result.failed, []);

  // The library is attempted FIRST — log ordering only (see build-all.ts's doc comment): a
  // workspace sibling resolves from SOURCE, not from this artifact, so there is no dependency
  // edge between the two builds either way.
  assert.deepEqual(result.outcomes.map((o) => o.name), ["@fixture/greetlib", "@fixture/uses-lib"]);

  const libOut = result.built.find((o) => o.name === "@fixture/greetlib").outPath;
  assert.match(libOut, /\.s2lib$/);
  const libZip = openZip(libOut);
  assert.deepEqual(
    libZip.getEntries().map((e) => e.entryName).sort(),
    ["index.d.ts", "index.js", "manifest.json"],
  );
  assert.equal(JSON.parse(libZip.readAsText("manifest.json")).kind, "library");

  const pluginOut = result.built.find((o) => o.name === "@fixture/uses-lib").outPath;
  assert.match(pluginOut, /\.s2sp$/);
  // The plugin's own build really did inline the sibling's source (Task 6), proving the pair is
  // the intended consumer relationship and not two builds that merely happened not to fail.
  assert.match(openZip(pluginOut).readAsText("plugin.js"), /hello, /);
});

// ---------------------------------------------------------------------------
// Fix round 1, finding #2: the brief's OWN stated reason for swapping the `ordered` loop's
// `buildPlugin` call to `buildPackage` is a `kind: "library"` member matched by the
// `s2script.workspace.plugins` globs — it lands in `ws.plugins` (loadWorkspace does not filter by
// kind), so `orderPlugins`/`selectPlugins` treat it as an ordinary plugin. Without the swap,
// `buildPlugin` throws its own "is s2script.kind=library" refusal for it. The `ws-library` fixture
// above puts its library under `libs/*` instead, which exercises the SEPARATE
// `declaredLibraries()` path and proves nothing about this swap — hence a dedicated fixture.
// ---------------------------------------------------------------------------

test("a kind:\"library\" member matched by the PLUGINS glob builds to .s2lib, not .s2sp", async () => {
  const ws = loadWorkspace(fx("ws-library-in-plugins-glob"));
  // The library really did land in ws.plugins (not ws.libs) — the precondition the swap fixes.
  assert.deepEqual(ws.plugins.map((p) => p.name), ["@fixture/libinplugins"]);
  assert.deepEqual(ws.libs, []);

  const result = await buildWorkspace({ workspace: ws, packagesDir });
  assert.deepEqual(result.failed, []);
  const out = result.built.find((o) => o.name === "@fixture/libinplugins").outPath;
  assert.match(out, /\.s2lib$/);
  const zip = openZip(out);
  assert.equal(JSON.parse(zip.readAsText("manifest.json")).kind, "library");
});

// ---------------------------------------------------------------------------
// Fix round 1, finding #3 (coordinator's ruling): `--filter` selects across libraries too. A
// filtered PLUGIN build never needs its sibling library built (a consumer resolves it from
// SOURCE, Task 6) — building a library is only for publishing it — so there is no correctness
// argument for forcing an unrelated library's build into a filtered run, and a real CI argument
// against it: an unrelated broken library must not fail a run meant to isolate one target.
//
// `ws-library-filter` holds one plugin (`@fixture/keep`), one healthy library
// (`@fixture/goodlib`), and one library that is DELIBERATELY BROKEN (`@fixture/brokenlib`, no
// `types` — buildLibrary's own fail-fast) so "does not build" and "does not fail on" are two
// different things to prove.
// ---------------------------------------------------------------------------

test("--filter naming only a PLUGIN does not build, and does not fail on, an unrelated library", async () => {
  const ws = loadWorkspace(fx("ws-library-filter"));
  const result = await buildWorkspace({ workspace: ws, packagesDir, filters: ["@fixture/keep"] });
  assert.deepEqual(result.outcomes.map((o) => o.name), ["@fixture/keep"]);
  assert.deepEqual(result.failed, [], "the broken library was never attempted, so it cannot fail the run");
});

test("--filter naming a LIBRARY builds just it, no plugins", async () => {
  const ws = loadWorkspace(fx("ws-library-filter"));
  const result = await buildWorkspace({ workspace: ws, packagesDir, filters: ["@fixture/goodlib"] });
  assert.deepEqual(result.outcomes.map((o) => o.name), ["@fixture/goodlib"]);
  assert.deepEqual(result.failed, []);
  assert.match(result.built[0].outPath, /\.s2lib$/);
});

test("--filter matching NEITHER a plugin NOR a library is still an ERROR", () => {
  const ws = loadWorkspace(fx("ws-library-filter"));
  assert.throws(() => selectPlugins(ws, ["@fixture/nonexistent"]), /--filter matched no plugin/);
});

test("selectLibraries mirrors selectPlugins's own matching rules", () => {
  const ws = loadWorkspace(fx("ws-library-filter"));
  assert.deepEqual(selectLibraries(ws, []).map((m) => m.name).sort(), ["@fixture/brokenlib", "@fixture/goodlib"]);
  assert.deepEqual(selectLibraries(ws, ["@fixture/goodlib"]).map((m) => m.name), ["@fixture/goodlib"]);
  assert.deepEqual(selectLibraries(ws, ["@fixture/keep"]).map((m) => m.name), [], "a plugin-only filter selects no libraries");
});

test("unfiltered, ws-library-filter really DOES fail on the broken library — proving the two filtered tests above are not vacuously passing", async () => {
  const ws = loadWorkspace(fx("ws-library-filter"));
  const result = await buildWorkspace({ workspace: ws, packagesDir });
  assert.deepEqual(result.failed.map((o) => o.name), ["@fixture/brokenlib"]);
  assert.match(result.failed[0].error, /types/);
});

// ---------------------------------------------------------------------------
// Fix round (final review), finding #10: `declaredLibraries` (build-all.ts) now routes its kind
// check through `packageKind`, which THROWS on an unknown kind (design D6: kind is explicit, never
// inferred) instead of a bare `=== "library"` comparison that silently read a typo as "not a
// library" — built by nothing, named by nothing, forever.
// ---------------------------------------------------------------------------

test("a libs/* member with a typo'd kind is a NAMED failure, not silently skipped", () => {
  const dir = copyFixture("ws-library-filter");
  const goodlibPkgPath = join(dir, "libs", "goodlib", "package.json");
  const pkg = JSON.parse(readFileSync(goodlibPkgPath, "utf8"));
  pkg.s2script.kind = "libary"; // typo
  writeFileSync(goodlibPkgPath, JSON.stringify(pkg));
  const ws = loadWorkspace(dir);
  assert.throws(() => selectLibraries(ws, []), /unknown s2script\.kind "libary"/);
});

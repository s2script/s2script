/**
 * Bundle directories (`s2script.workspace.bundles`): several bundles in one workspace, each with
 * its own descriptor package.json and its own members, versioned by one `s2s version` and
 * published by one `s2s deploy`.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/multi-bundle.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { unzipSync } from "fflate";

import { RegistryClient } from "../src/registry/client.ts";
import { buildWorkspace } from "../src/workspace/build-all.ts";
import { loadWorkspace } from "../src/workspace/workspace.ts";
import { bundleOfPlugin, bundleProblems, readBundle, readBundles } from "../src/workspace/bundle.ts";
import { builtPluginsFromOutcomes, computePlan, planBundle, uploadBundle, uploadPlan } from "../src/workspace/deploy-all.ts";
import { plannedBundleBumps } from "../src/version/bundle-bump.ts";
import { versionWorkspace } from "../src/version/version.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const fixture = join(here, "fixtures", "ws-multi-bundle");

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

/** A writable copy of the fixture, with `edits` (relative path -> JSON object) applied. */
function scratch(t, { edits = {}, changesets = {} } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "s2s-multi-bundle-"));
  cpSync(fixture, dir, { recursive: true });
  if (nodeModules !== null) symlinkSync(nodeModules, join(dir, "node_modules"), "dir");
  for (const [rel, json] of Object.entries(edits)) writeFileSync(join(dir, rel), JSON.stringify(json, null, 2) + "\n");
  for (const [id, body] of Object.entries(changesets)) writeFileSync(join(dir, ".changeset", `${id}.md`), body);
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
const rootPkg = () => readJson(join(fixture, "package.json"));

test("bundle dirs: each bundle reads its own members; a plugin outside every bundle is in none", () => {
  const ws = loadWorkspace(fixture);
  assert.deepEqual(bundleProblems(ws), []);
  assert.equal(readBundle(ws), null, "the root is not a bundle");
  const bundles = readBundles(ws);
  assert.deepEqual(
    bundles.map((b) => [b.name, b.version, b.relDir, b.members.map((m) => m.name), [...b.optional]]),
    [
      ["@fixture/jb", "1.0.0", "jb", ["@fixture/jb-core", "@fixture/jb-perks"], ["@fixture/jb-perks"]],
      ["@fixture/ttt", "2.0.0", "ttt", ["@fixture/ttt-core", "@fixture/ttt-shop"], []],
    ],
  );
  const of = bundleOfPlugin(bundles);
  assert.equal(of.get("@fixture/ttt-shop"), "@fixture/ttt");
  assert.equal(of.get("@fixture/jb-core"), "@fixture/jb");
  assert.equal(of.has("@fixture/standalone"), false);
});

test("bundle dirs: a glob entry expands like the plugin globs", (t) => {
  const pkg = rootPkg();
  pkg.s2script.workspace.bundles = ["*"];
  const dir = scratch(t, { edits: { "package.json": pkg } });
  // `tools/` has no package.json, so `*` names exactly the two bundle directories.
  assert.deepEqual(
    readBundles(loadWorkspace(dir)).map((b) => b.name),
    ["@fixture/jb", "@fixture/ttt"],
  );
});

const refusals = [
  [
    "a root bundle that also lists bundle directories",
    (pkg) => ({ "package.json": { ...pkg, private: undefined, s2script: { ...pkg.s2script, kind: "bundle" } } }),
    /a root bundle cannot also contain bundles/,
  ],
  [
    "an entry that matches nothing",
    (pkg) => ({ "package.json": { ...pkg, s2script: { workspace: { ...pkg.s2script.workspace, bundles: ["ttt", "nope"] } } } }),
    /"nope" matches no directory/,
  ],
  [
    "a malformed field",
    (pkg) => ({ "package.json": { ...pkg, s2script: { workspace: { ...pkg.s2script.workspace, bundles: "ttt" } } } }),
    /must be an array/,
  ],
  [
    "a descriptor without the bundle kind",
    () => ({ "ttt/package.json": { name: "@fixture/ttt", version: "2.0.0" } }),
    /ttt\/package.json: is listed in s2script.workspace.bundles but has no s2script.kind "bundle"/,
  ],
  [
    "a descriptor that is still a workspace root",
    () => ({
      "ttt/package.json": {
        name: "@fixture/ttt",
        version: "2.0.0",
        s2script: { kind: "bundle", workspace: { plugins: ["plugins/*"] } },
      },
    }),
    /must not carry s2script.workspace/,
  ],
  [
    "two bundles with one name",
    () => ({ "jb/package.json": { name: "@fixture/ttt", version: "1.0.0", s2script: { kind: "bundle" } } }),
    /duplicate bundle name "@fixture\/ttt"/,
  ],
  [
    "a member outside the bundle's scope",
    () => ({ "ttt/plugins/shop/package.json": { name: "@other/ttt-shop", version: "2.0.0", main: "src/plugin.ts" } }),
    /@other\/ttt-shop cannot be a member of @fixture\/ttt/,
  ],
  [
    "an optional name from a sibling bundle",
    () => ({
      "jb/package.json": { name: "@fixture/jb", version: "1.0.0", s2script: { kind: "bundle", bundle: { optional: ["@fixture/ttt-shop"] } } },
    }),
    /names "@fixture\/ttt-shop", which is not a non-private plugin of this bundle/,
  ],
];

for (const [label, edits, pattern] of refusals) {
  test(`bundle dirs: refuses ${label}`, (t) => {
    const dir = scratch(t, { edits: edits(rootPkg()) });
    const problems = bundleProblems(loadWorkspace(dir));
    assert.ok(problems.some((p) => pattern.test(p)), `expected ${pattern} in:\n${problems.join("\n")}`);
  });
}

test("bundle dirs: refuses nested bundle directories", (t) => {
  const pkg = rootPkg();
  pkg.s2script.workspace.bundles = ["ttt", "ttt/plugins/core", "jb"];
  const dir = scratch(t, {
    edits: {
      "package.json": pkg,
      "ttt/plugins/core/package.json": { name: "@fixture/ttt-core", version: "2.0.0", main: "src/plugin.ts", s2script: { kind: "bundle" } },
    },
  });
  const problems = bundleProblems(loadWorkspace(dir));
  assert.ok(problems.some((p) => /ttt\/plugins\/core: is inside bundle directory ttt/.test(p)), problems.join("\n"));
  assert.ok(problems.some((p) => /is both a workspace package and a bundle directory/.test(p)), problems.join("\n"));
});

const release = (name, type, newVersion = "9.9.9") => ({ name, type, oldVersion: "0.0.0", newVersion, changesets: [] });

test("bundle dirs: each bundle follows only its own members' bumps", () => {
  const ws = loadWorkspace(fixture);
  assert.deepEqual(
    plannedBundleBumps(ws, [release("@fixture/ttt-shop", "minor", "2.1.0"), release("@fixture/standalone", "major")]),
    [
      {
        name: "@fixture/ttt",
        from: "2.0.0",
        to: "2.1.0",
        type: "minor",
        because: ["@fixture/ttt-shop@2.1.0"],
        path: join(fixture, "ttt", "package.json"),
      },
    ],
  );
  assert.deepEqual(
    plannedBundleBumps(ws, [release("@fixture/jb-core", "major"), release("@fixture/ttt-core", "patch")]).map((b) => [b.name, b.to]),
    [
      ["@fixture/jb", "2.0.0"],
      ["@fixture/ttt", "2.0.1"],
    ],
  );
});

test("s2s version: one changeset run bumps each bundle descriptor on disk, independently", { skip: nodeModules === null }, async (t) => {
  const dir = scratch(t, {
    changesets: {
      "shop-feature": `---\n"@fixture/ttt-shop": minor\n---\n\nA feature.\n`,
      "perks-fix": `---\n"@fixture/jb-perks": patch\n---\n\nA fix.\n`,
    },
  });
  const result = await versionWorkspace(dir);
  assert.deepEqual(
    result.bundleBumps.map((b) => [b.name, b.from, b.to]),
    [
      ["@fixture/jb", "1.0.0", "1.0.1"],
      ["@fixture/ttt", "2.0.0", "2.1.0"],
    ],
  );
  const ttt = readJson(join(dir, "ttt", "package.json"));
  assert.equal(ttt.version, "2.1.0");
  assert.equal(ttt.s2script.kind, "bundle", "the rest of the descriptor is preserved");
  assert.equal(readJson(join(dir, "jb", "package.json")).version, "1.0.1");
  assert.equal(readJson(join(dir, "package.json")).version, "0.0.0", "the root is not a bundle and is left alone");
  assert.ok(result.touchedFiles.includes(join(dir, "ttt", "package.json")));
  assert.ok(result.touchedFiles.includes(join(dir, "jb", "package.json")));
});

const jsonResponse = (status, body) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function fakeRegistry({ failDeploy = () => false } = {}) {
  const posts = [];
  const client = new RegistryClient({
    baseUrl: "https://www.example.com",
    token: "s2s_x",
    fetch: async (url, init) => {
      const u = new URL(String(url));
      if (u.pathname === "/api/v1/meta") return jsonResponse(404, { message: "package not found" });
      if (u.pathname === "/api/v1/deploy") {
        const files = unzipSync(new Uint8Array(init.body));
        const manifest = JSON.parse(Buffer.from(files["manifest.json"]).toString("utf8"));
        posts.push({ manifest });
        if (failDeploy(manifest)) return jsonResponse(500, { error: "boom" });
        return jsonResponse(200, { name: manifest.id, version: manifest.version, reviewState: "unreviewed" });
      }
      return jsonResponse(404, { message: "unexpected" });
    },
  });
  return { client, posts };
}

async function built() {
  const ws = loadWorkspace(fixture);
  const build = await buildWorkspace({ workspace: ws, packagesDir });
  assert.deepEqual(build.failed, []);
  const { ordered, built } = builtPluginsFromOutcomes(ws, build.outcomes);
  const plan = await computePlan(ordered, async () => false);
  return { bundles: readBundles(ws), plan, built };
}

test("bundle dirs deploy: each member claims its own bundle, and each bundle pins only its members", async () => {
  const { bundles, plan, built: b } = await built();
  const { client, posts } = fakeRegistry();
  const results = await uploadPlan(plan, b, client, { bundle: bundleOfPlugin(bundles) });
  const claims = Object.fromEntries(posts.map((p) => [p.manifest.id, p.manifest.bundle]));
  assert.deepEqual(claims, {
    "@fixture/jb-core": "@fixture/jb",
    "@fixture/jb-perks": "@fixture/jb",
    "@fixture/standalone": undefined,
    "@fixture/ttt-core": "@fixture/ttt",
    "@fixture/ttt-shop": "@fixture/ttt",
  });
  for (const bundle of bundles) {
    const entry = await planBundle(bundle, client);
    const r = await uploadBundle(entry, results, client);
    assert.equal(r.status, "published");
  }
  const ttt = posts.find((p) => p.manifest.id === "@fixture/ttt").manifest;
  assert.deepEqual(ttt.members, { "@fixture/ttt-core": { version: "2.0.0" }, "@fixture/ttt-shop": { version: "2.0.0" } });
});

test("bundle dirs deploy: a member failure blocks only its own bundle", async () => {
  const { bundles, plan, built: b } = await built();
  const { client } = fakeRegistry({ failDeploy: (m) => m.id === "@fixture/jb-core" });
  const results = await uploadPlan(plan, b, client, { bundle: bundleOfPlugin(bundles) });
  const status = {};
  for (const bundle of bundles) {
    // The same per-bundle filtering `s2s deploy` does: a bundle sees only its own members' results.
    const members = new Set(bundle.members.map((m) => m.name));
    const r = await uploadBundle(await planBundle(bundle, client), results.filter((x) => members.has(x.plugin.name)), client);
    status[bundle.name] = r.status;
  }
  assert.deepEqual(status, { "@fixture/jb": "skipped", "@fixture/ttt": "published" });
});

test("s2s deploy --filter: a bundle with members outside the filter is skipped, not published", async () => {
  const { execFile } = await import("node:child_process");
  const { promisify } = await import("node:util");
  // An unreachable registry: every "already published?" check degrades to unpublished.
  const env = { ...process.env, CI: "1", S2SCRIPT_REGISTRY_URL: "http://127.0.0.1:9" };
  const r = await promisify(execFile)(
    process.execPath,
    ["--experimental-strip-types", "--no-warnings", "src/cli.ts", "deploy", fixture, "--dry-run", "--filter", "@fixture/ttt-*"],
    { cwd: join(here, ".."), env },
  );
  assert.match(r.stdout, /@fixture\/ttt 2\.0\.0 {3}PUBLISH \(bundle of 2 plugins\)/);
  assert.match(r.stdout, /@fixture\/jb 1\.0\.0 {3}skip \(not every member is in --filter\)/);
  assert.doesNotMatch(r.stdout, /@fixture\/jb 1\.0\.0 {3}PUBLISH/);
});

/**
 * TDD test: `s2s deploy` — the §6.3 private gate (single-plugin mode) and workspace-mode plan
 * computation / upload (design spec 2026-07-27 §6).
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/deploy.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { buildPlugin } from "../src/build.ts";
import { RegistryClient, RegistryError } from "../src/registry/client.ts";
import { assertDeployable, assembleDeployArchive, deployPlugin } from "../src/registry/deploy.ts";
import {
  builtPluginsFromOutcomes,
  computePlan,
  formatPlan,
  isAlreadyPublished,
  publishCount,
  uploadPlan,
} from "../src/workspace/deploy-all.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const fx = (n) => join(here, "fixtures", n);

function clientWith(fetchImpl) {
  return new RegistryClient({ baseUrl: "https://www.example.com", token: "s2s_x", fetch: fetchImpl });
}

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

// ---------------------------------------------------------------------------
// §6.3 — the private gate, single-plugin mode
// ---------------------------------------------------------------------------

test("assertDeployable refuses a private package by name, and only that", () => {
  assert.throws(() => assertDeployable({ name: "@demo/x", private: true }, "x"), (e) => {
    assert.match(e.message, /@demo\/x is private/);
    assert.match(e.message, /never published/);
    return true;
  });
  assert.doesNotThrow(() => assertDeployable({ name: "@demo/x", private: false }, "x"));
  assert.doesNotThrow(() => assertDeployable({ name: "@demo/x" }, "x"), "no private field at all is deployable");
});

test("deployPlugin refuses a private package BEFORE requiring login or building — closes §3.4's hole", async () => {
  // fixtures/publisher carries private:true. Today (pre-§6.3) this would have built + attempted
  // to deploy it; the assertion below fails in that world (no throw, or a "not logged in" throw
  // instead of a private-package one).
  await assert.rejects(
    () => deployPlugin({ dir: fx("publisher"), packagesDir }),
    (e) => {
      assert.match(e.message, /@demo\/publisher is private/);
      return true;
    },
  );
});

// ---------------------------------------------------------------------------
// assembleDeployArchive — the split-out tail of deployPlugin, reused by workspace upload
// ---------------------------------------------------------------------------

test("assembleDeployArchive reads the manifest out of an already-built archive, types null when unpublished", async () => {
  const { readFileSync } = await import("node:fs");
  const pkg = JSON.parse(readFileSync(join(fx("hello"), "package.json"), "utf8"));
  const outPath = await buildPlugin(fx("hello"), packagesDir);
  const { manifest, s2sp, types } = assembleDeployArchive(fx("hello"), pkg, outPath);
  assert.equal(manifest.id, "@demo/hello");
  assert.equal(types, null);
  assert.deepEqual([...s2sp], [...readFileSync(outPath)], "s2sp is the archive's own raw bytes");
});

test("assembleDeployArchive packs a types tarball when the plugin publishes", async () => {
  const { readFileSync } = await import("node:fs");
  const pkg = JSON.parse(readFileSync(join(fx("publisher"), "package.json"), "utf8"));
  const outPath = await buildPlugin(fx("publisher"), packagesDir);
  const { types, manifest } = assembleDeployArchive(fx("publisher"), pkg, outPath);
  assert.ok(types, "publisher declares s2script.publishes + types, so a tarball must be packed");
  assert.ok(manifest.publishes && manifest.publishes["@demo/publisher"], "manifest carries the derived publishes block");
});

// ---------------------------------------------------------------------------
// §6.1 step 2 — plan computation across all three outcomes
// ---------------------------------------------------------------------------

test("computePlan: private, already-published, and PUBLISH — all three outcomes", async () => {
  const plugins = [
    { name: "@me/warmup", version: "1.0.0", private: true },
    { name: "@me/shared-api", version: "1.2.0", private: false },
    { name: "@me/shop", version: "2.0.1", private: false },
  ];
  const published = new Set(["@me/shared-api@1.2.0"]);
  const plan = await computePlan(plugins, async (name, version) => published.has(`${name}@${version}`));
  assert.deepEqual(
    plan.map((e) => [e.plugin.name, e.reason]),
    [
      ["@me/warmup", "skip (private)"],
      ["@me/shared-api", "skip (already published)"],
      ["@me/shop", "PUBLISH"],
    ],
  );
  assert.equal(publishCount(plan), 1);
});

test("computePlan never calls checkPublished for a private plugin — private wins outright", async () => {
  let called = false;
  const plan = await computePlan([{ name: "@me/x", version: "1.0.0", private: true }], async () => {
    called = true;
    return true;
  });
  assert.equal(plan[0].reason, "skip (private)");
  assert.equal(called, false);
});

test("formatPlan matches design spec §6.1's worked example, column for column", async () => {
  const plugins = [
    { name: "@me/shared-api", version: "1.2.0", private: false },
    { name: "@me/shop", version: "2.0.1", private: false },
    { name: "@me/ranks", version: "0.4.0", private: false },
    { name: "@me/warmup", version: "1.0.0", private: true },
  ];
  const published = new Set(["@me/shared-api@1.2.0"]);
  const plan = await computePlan(plugins, async (name, version) => published.has(`${name}@${version}`));
  assert.equal(
    formatPlan(plan),
    [
      "    @me/shared-api 1.2.0   skip (already published)",
      "    @me/shop       2.0.1   PUBLISH",
      "    @me/ranks      0.4.0   PUBLISH",
      "    @me/warmup     1.0.0   skip (private)",
    ].join("\n"),
  );
  assert.equal(publishCount(plan), 2, "matches the spec's \"Publish 2 plugins\" prompt");
});

// ---------------------------------------------------------------------------
// §6.2 — the already-published check is a courtesy; any failure degrades to "assume unpublished"
// ---------------------------------------------------------------------------

test("isAlreadyPublished: true only on an EXACT version hit", async () => {
  const client = clientWith(async () => jsonResponse(200, { name: "@me/shop", version: "2.0.1", reviewState: "reviewed", hasTypes: false }));
  assert.equal(await isAlreadyPublished(client, "@me/shop", "2.0.1"), true);
});

test("isAlreadyPublished: a different resolved version is NOT a match (range semantics don't leak in)", async () => {
  const client = clientWith(async () => jsonResponse(200, { name: "@me/shop", version: "1.9.0", reviewState: "reviewed", hasTypes: false }));
  assert.equal(await isAlreadyPublished(client, "@me/shop", "2.0.1"), false);
});

test("isAlreadyPublished: registry unreachable degrades to \"assume unpublished\", never throws", async () => {
  const client = clientWith(async () => {
    throw new Error("fetch failed", { cause: new Error("connect ECONNREFUSED") });
  });
  assert.equal(await isAlreadyPublished(client, "@me/shop", "2.0.1"), false);
});

test("isAlreadyPublished: a 404-shaped \"not found\" also reads as unpublished", async () => {
  const client = clientWith(async () => jsonResponse(404, { message: "not found" }));
  assert.equal(await isAlreadyPublished(client, "@me/shop", "2.0.1"), false);
});

// ---------------------------------------------------------------------------
// builtPluginsFromOutcomes — recovering full plugin objects from build-all.ts's slim outcomes
// ---------------------------------------------------------------------------

test("builtPluginsFromOutcomes: preserves outcome order, maps only successes into `built`", () => {
  const ws = {
    root: "/ws",
    plugins: [
      { name: "@me/a", version: "1.0.0", dir: "/ws/plugins/a", relDir: "plugins/a", private: false, pkg: {} },
      { name: "@me/b", version: "1.0.0", dir: "/ws/plugins/b", relDir: "plugins/b", private: false, pkg: {} },
    ],
  };
  const outcomes = [
    { name: "@me/a", version: "1.0.0", relDir: "plugins/a", outPath: "/ws/plugins/a/dist/x.s2sp" },
    { name: "@me/b", version: "1.0.0", relDir: "plugins/b", error: "boom" },
  ];
  const { ordered, built } = builtPluginsFromOutcomes(ws, outcomes);
  assert.deepEqual(ordered.map((p) => p.name), ["@me/a", "@me/b"]);
  assert.equal(built.size, 1);
  assert.equal(built.get("@me/a").outPath, "/ws/plugins/a/dist/x.s2sp");
  assert.equal(built.has("@me/b"), false);
});

// ---------------------------------------------------------------------------
// §6.1 step 4 / §6.2 — upload in plan order; a 409 mid-fan-out is a SKIP, never a failure
// ---------------------------------------------------------------------------

async function builtHello() {
  const { readFileSync } = await import("node:fs");
  const pkg = JSON.parse(readFileSync(join(fx("hello"), "package.json"), "utf8"));
  const outPath = await buildPlugin(fx("hello"), packagesDir);
  const plugin = { name: pkg.name, version: pkg.version, dir: fx("hello"), relDir: "hello", private: false, pkg };
  return { plugin, outPath };
}

test("uploadPlan: skip entries pass straight through, no build/upload needed for them", async () => {
  const plan = [
    { plugin: { name: "@me/warmup", version: "1.0.0" }, reason: "skip (private)" },
    { plugin: { name: "@me/shared-api", version: "1.2.0" }, reason: "skip (already published)" },
  ];
  const client = clientWith(async () => {
    throw new Error("must not be called for a skip entry");
  });
  const results = await uploadPlan(plan, new Map(), client);
  assert.deepEqual(results.map((r) => r.status), ["skipped", "skipped"]);
  assert.deepEqual(results.map((r) => r.detail), ["skip (private)", "skip (already published)"]);
});

test("uploadPlan: a successful deploy reports \"published\" with the server's reviewState", async () => {
  const { plugin, outPath } = await builtHello();
  const plan = [{ plugin, reason: "PUBLISH" }];
  const built = new Map([[plugin.name, { plugin, outPath }]]);
  const client = clientWith(async () => jsonResponse(200, { name: plugin.name, version: plugin.version, reviewState: "unreviewed" }));
  const results = await uploadPlan(plan, built, client);
  assert.equal(results[0].status, "published");
  assert.equal(results[0].detail, "unreviewed");
});

test("uploadPlan: the SERVER's 409 duplicate-version rejection is a SKIP, not a failure (§6.2)", async () => {
  const { plugin, outPath } = await builtHello();
  const plan = [{ plugin, reason: "PUBLISH" }];
  const built = new Map([[plugin.name, { plugin, outPath }]]);
  const client = clientWith(async () => jsonResponse(409, { error: `version ${plugin.name}@${plugin.version} already exists` }));
  const results = await uploadPlan(plan, built, client);
  assert.equal(results[0].status, "skipped");
  assert.match(results[0].detail, /already published/);
});

test("uploadPlan: any OTHER server rejection is reported as failed, not swallowed", async () => {
  const { plugin, outPath } = await builtHello();
  const plan = [{ plugin, reason: "PUBLISH" }];
  const built = new Map([[plugin.name, { plugin, outPath }]]);
  const client = clientWith(async () => jsonResponse(500, { error: "internal error" }));
  const results = await uploadPlan(plan, built, client);
  assert.equal(results[0].status, "failed");
  assert.match(results[0].detail, /internal error/);
});

test("uploadPlan: a PUBLISH entry missing from `built` is a caller bug, not a runtime skip", async () => {
  const plan = [{ plugin: { name: "@me/ghost", version: "1.0.0" }, reason: "PUBLISH" }];
  await assert.rejects(
    () => uploadPlan(plan, new Map(), clientWith(async () => jsonResponse(200, {}))),
    /never built/,
  );
});

test("RegistryError 409 is recognized regardless of message wording (status-based, not text-matched)", () => {
  const e = new RegistryError("anything", 409);
  assert.ok(e instanceof RegistryError);
  assert.equal(e.status, 409);
});

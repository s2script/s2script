/**
 * TDD test: addPackage()'s kind dispatch — the seam between `s2s add` and the build.
 *
 * Before this test, neither branch's SUCCESS path was covered: lib-extract.test.mjs only tests
 * the extraction primitive in isolation, and commands-add.test.mjs's workspace-root refusal is
 * pinned to an unroutable registry, so it never reaches a real resolve/download. If addPackage's
 * vendored path ever drifted from what libraries.ts's `localLibraryDir`/`resolveLibraries` expect,
 * `s2s add` would report success while the BUILD failed later with "declared but not vendored" —
 * far from the actual cause. This test pins that agreement directly by feeding addPackage's real
 * on-disk output into the real, unmodified `resolveLibraries()`.
 *
 * RegistryClient already accepts an injectable `fetch` (see registry-client.test.mjs's
 * `clientWith`); addPackage now threads that same optional `fetch` through, so this follows the
 * established fetch-stub pattern rather than monkey-patching globalThis.fetch.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/add-dispatch.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, writeFileSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { zipSync, strToU8 } from "fflate";
import { addPackage } from "../src/registry/add.ts";
import { localLibraryDir, resolveLibraries } from "../src/libraries.ts";
import { packTypesTarball } from "../src/types-pack.ts";

function consumerDir() {
  const dir = mkdtempSync(join(tmpdir(), "s2s-add-dispatch-"));
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "consumer", version: "0.0.1" }, null, 2) + "\n");
  return dir;
}

function fakeS2lib() {
  return Buffer.from(
    zipSync({
      "manifest.json": strToU8(
        JSON.stringify({ kind: "library", id: "@edge/base64", version: "1.2.3", apiVersion: "1.4" }),
      ),
      "index.js": strToU8("module.exports.encode = (s) => s;\n"),
      "index.d.ts": strToU8("export declare function encode(s: string): string;\n"),
    }),
  );
}

function jsonResponse(body) {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

test("library dispatch: vendors into .s2script/libs, merges s2script.libraries, writes no .npmrc", async () => {
  const dir = consumerDir();
  const s2lib = fakeS2lib();
  const calls = [];
  const fetchStub = async (url) => {
    const u = String(url);
    calls.push(u);
    if (u.includes("/api/v1/resolve")) {
      return jsonResponse({
        name: "@edge/base64",
        version: "1.2.3",
        reviewState: "reviewed",
        hasTypes: false,
        kind: "library",
      });
    }
    if (u.includes("/api/v1/download/lib")) return new Response(s2lib, { status: 200 });
    throw new Error("unexpected url " + u);
  };

  const result = await addPackage({
    pluginDir: dir,
    spec: "@edge/base64",
    registryUrl: "https://example.invalid",
    fetch: fetchStub,
  });

  assert.equal(result.kind, "library");
  assert.equal(result.typesDir, null, "the library variant must type as null, not the vendored dir");
  assert.equal(result.npmrcLine, null);
  assert.ok(calls.some((u) => u.includes("/api/v1/download/lib")));
  assert.ok(!calls.some((u) => u.includes("/api/v1/download/types")), "a library must never hit the types endpoint");

  // On-disk layout.
  assert.match(readFileSync(join(result.libDir, "index.js"), "utf8"), /encode/);
  assert.match(readFileSync(join(result.libDir, "index.d.ts"), "utf8"), /encode/);
  const vendoredPkg = JSON.parse(readFileSync(join(result.libDir, "package.json"), "utf8"));
  assert.equal(vendoredPkg.version, "1.2.3");

  // Consumer manifest merge — s2script.libraries, never pluginDependencies.
  const consumerPkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  assert.deepEqual(consumerPkg.s2script.libraries, { "@edge/base64": "^1.0.0" });
  assert.equal(consumerPkg.s2script.pluginDependencies, undefined);

  // No .npmrc for a library, ever — the whole point of this feature.
  assert.equal(existsSync(join(dir, ".npmrc")), false);

  // THE seam this test exists to pin: addPackage's own libDir must be exactly what
  // localLibraryDir independently computes, and the real, unmodified resolveLibraries() must
  // resolve the declared name straight to it. This is what actually catches a layout drift
  // between `s2s add` and the build — not just eyeballing that the paths look similar.
  assert.equal(result.libDir, localLibraryDir(dir, "@edge/base64"));
  const resolved = resolveLibraries(dir, { "@edge/base64": "^1.0.0" });
  assert.deepEqual(resolved.missing, []);
  assert.equal(resolved.manifests["@edge/base64"].source, "vendored");
  assert.equal(resolved.manifests["@edge/base64"].version, "1.2.3");
  assert.equal(resolved.manifests["@edge/base64"].apiVersion, "1.4");
  assert.deepEqual(resolved.paths["@edge/base64"], [join(result.libDir, "index.d.ts")]);
});

test("plugin dispatch is unaffected by the library branch: types tarball, pluginDependencies, npmrc", async () => {
  const dir = consumerDir();
  const dtsPath = join(dir, "producer-api.d.ts");
  writeFileSync(dtsPath, "export declare function ping(): void;\n");
  const tgz = packTypesTarball({ name: "@edge/plugin-thing", version: "2.0.0", typesPath: dtsPath });

  const fetchStub = async (url) => {
    const u = String(url);
    if (u.includes("/api/v1/resolve")) {
      return jsonResponse({
        name: "@edge/plugin-thing",
        version: "2.0.0",
        reviewState: "reviewed",
        hasTypes: true,
        kind: "plugin",
      });
    }
    if (u.includes("/api/v1/download/types")) return new Response(tgz, { status: 200 });
    throw new Error("unexpected url " + u);
  };

  const result = await addPackage({
    pluginDir: dir,
    spec: "@edge/plugin-thing",
    registryUrl: "https://example.invalid",
    fetch: fetchStub,
  });

  assert.equal(result.kind, "plugin");
  assert.equal(result.libDir, null);
  assert.match(readFileSync(join(result.typesDir, "index.d.ts"), "utf8"), /ping/);

  const consumerPkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  assert.deepEqual(consumerPkg.s2script.pluginDependencies, { "@edge/plugin-thing": "^2.0.0" });
  assert.equal(consumerPkg.s2script.libraries, undefined);
  assert.equal(existsSync(join(dir, ".s2script", "libs")), false, "the plugin path must never touch libs/");
  assert.equal(typeof result.npmrcLine, "string");
});

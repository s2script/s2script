/**
 * TDD test: library build → .s2lib.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/build-library.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { packageKind, buildLibrary } from "../src/build-library.ts";

function libDir({ kind = "library", extraFiles = {} } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "s2lib-"));
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify({
      name: "@edge/base64",
      version: "1.2.3",
      main: "src/index.ts",
      types: "src/index.d.ts",
      s2script: { kind },
    }, null, 2),
  );
  writeFileSync(join(dir, "src", "index.ts"), `export function encode(s: string): string { return s; }\n`);
  writeFileSync(join(dir, "src", "index.d.ts"), `export declare function encode(s: string): string;\n`);
  for (const [rel, body] of Object.entries(extraFiles)) writeFileSync(join(dir, rel), body);
  return dir;
}

test("packageKind defaults to plugin and honours an explicit library", () => {
  assert.equal(packageKind({}), "plugin");
  assert.equal(packageKind({ s2script: {} }), "plugin");
  assert.equal(packageKind({ s2script: { kind: "library" } }), "library");
  assert.throws(() => packageKind({ s2script: { kind: "lib" } }), /unknown s2script\.kind/);
});

test("buildLibrary emits a .s2lib with exactly manifest.json, index.js and index.d.ts", async () => {
  const dir = libDir();
  const out = await buildLibrary(dir);
  assert.match(out, /_edge_base64\.s2lib$/);
  const entries = unzipSync(new Uint8Array(readFileSync(out)));
  assert.deepEqual(Object.keys(entries).sort(), ["index.d.ts", "index.js", "manifest.json"]);
  const manifest = JSON.parse(Buffer.from(entries["manifest.json"]).toString("utf8"));
  assert.equal(manifest.kind, "library");
  assert.equal(manifest.id, "@edge/base64");
  assert.equal(manifest.version, "1.2.3");
  assert.equal(typeof manifest.apiVersion, "string");
  assert.match(Buffer.from(entries["index.js"]).toString("utf8"), /encode/);
});

test("a library must declare types", async () => {
  const dir = libDir();
  const pkgPath = join(dir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  delete pkg.types;
  writeFileSync(pkgPath, JSON.stringify(pkg));
  await assert.rejects(() => buildLibrary(dir), /types/);
});

test("a library may not declare pluginDependencies", async () => {
  const dir = libDir();
  const pkgPath = join(dir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  pkg.s2script.pluginDependencies = { "@edge/other": "^1.0.0" };
  writeFileSync(pkgPath, JSON.stringify(pkg));
  await assert.rejects(() => buildLibrary(dir), /pluginDependencies/);
});

// Fix round (final review), finding #9: the OTHER half of the same guard (build-library.ts:41)
// had no coverage of its own — deleting `s2.optionalPluginDependencies` from that `if` condition
// left the whole suite green, since the sibling `pluginDependencies` test above never exercises it.
test("a library may not declare optionalPluginDependencies", async () => {
  const dir = libDir();
  const pkgPath = join(dir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  pkg.s2script.optionalPluginDependencies = { "@edge/other": "^1.0.0" };
  writeFileSync(pkgPath, JSON.stringify(pkg));
  await assert.rejects(() => buildLibrary(dir), /optionalPluginDependencies/);
});

// Fix round (final review), finding #9: build-library.ts:47 ("a library may not declare
// publishes") had zero regression coverage — deleting the check left the suite green.
test("a library may not declare publishes", async () => {
  const dir = libDir();
  const pkgPath = join(dir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  pkg.s2script.publishes = "self";
  writeFileSync(pkgPath, JSON.stringify(pkg));
  await assert.rejects(() => buildLibrary(dir), /publishes/);
});

// ---------------------------------------------------------------------------
// Fix round (final review), finding #1: mirrors the `buildPlugin` integration test in
// build.test.mjs — proves the `@s2script/*` reserved-scope refusal is wired into `buildLibrary`
// too, not just present in the shared `assertLibrariesResolved` helper.
// ---------------------------------------------------------------------------

test("buildLibrary refuses an @s2script/*-scoped s2script.libraries entry", async () => {
  const dir = libDir();
  const pkgPath = join(dir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  pkg.s2script.libraries = { "@s2script/base64": "^1.0.0" };
  writeFileSync(pkgPath, JSON.stringify(pkg));
  await assert.rejects(() => buildLibrary(dir), /reserved/);
});

// ---------------------------------------------------------------------------
// Fix round (final review), finding #4: `buildLibrary` never ran the B2 residual-rule lint gate —
// nothing linted library source ANYWHERE (lint.ts's own directory walk skips dot-directories and
// scopes to a CONSUMER's plugin dir, so a vendored or sibling library is out of its range either
// way). `no-floating-promise-in-factory` is used here (not `no-ctx-escape`, which needs a leftover
// `plugin()` factory the public types no longer export) as the simplest pinned-rule violation that
// still typechecks; the point is that ANY of the four now fails a library build.
// ---------------------------------------------------------------------------

test("buildLibrary lints library source — a floating promise in OnPluginStart fails the build", async () => {
  const dir = libDir();
  writeFileSync(
    join(dir, "src", "index.ts"),
    'import { Database } from "@s2script/sdk/db";\n\n' +
      "export async function OnPluginStart(): Promise<void> {\n" +
      '  Database.open("prefs");\n' +
      "}\n",
  );
  await assert.rejects(() => buildLibrary(dir), /lint failed[\s\S]*no-floating-promise-in-factory/);
});

test("@s2script/* imports stay external — the consumer's context resolves them", async () => {
  const dir = libDir();
  writeFileSync(
    join(dir, "src", "index.ts"),
    `import { Chat } from "@s2script/sdk/chat";\nexport function shout(s: string): void { Chat.toAll(s); }\n`,
  );
  const out = await buildLibrary(dir);
  const entries = unzipSync(new Uint8Array(readFileSync(out)));
  const js = Buffer.from(entries["index.js"]).toString("utf8");
  assert.match(js, /@s2script\/sdk\/chat/);
});

// ---------------------------------------------------------------------------
// Fix round 1, finding #1: a library declaring its own `s2script.libraries` must inline the
// dependency's real code in BOTH resolution modes (libraries.ts) — a vendored copy under
// `.s2script/libs/`, and a workspace-sibling library resolved straight from source. The sibling
// mode needs `alias: siblingDirs` wired into `buildLibrary`'s own esbuild call (mirroring
// build.ts's identical wiring for a PLUGIN's library imports): a sibling has no vendored copy for
// `nodePaths` to find unaided, so without the alias this typechecks fine and then fails esbuild
// with "Could not resolve".
// ---------------------------------------------------------------------------

function vendoredLibraryOfLibrary() {
  const dir = mkdtempSync(join(tmpdir(), "s2lib-of-lib-v-"));
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify({
      name: "@edge/shout",
      version: "1.0.0",
      main: "src/index.ts",
      types: "src/index.d.ts",
      s2script: { kind: "library", libraries: { "@edge/vendored-inner": "^1.0.0" } },
    }),
  );
  writeFileSync(
    join(dir, "src", "index.ts"),
    'import { inner } from "@edge/vendored-inner";\n\nexport function shout(): string {\n  return inner();\n}\n',
  );
  writeFileSync(join(dir, "src", "index.d.ts"), "export declare function shout(): string;\n");
  const libDirPath = join(dir, ".s2script", "libs", "@edge", "vendored-inner");
  mkdirSync(libDirPath, { recursive: true });
  writeFileSync(join(libDirPath, "index.js"), 'module.exports = { inner: () => "vendored-inner-resolved" };\n');
  writeFileSync(join(libDirPath, "index.d.ts"), "export declare function inner(): string;\n");
  writeFileSync(
    join(libDirPath, "package.json"),
    JSON.stringify({ name: "@edge/vendored-inner", version: "1.0.0", main: "index.js", types: "index.d.ts" }),
  );
  return dir;
}

function siblingLibraryOfLibrary() {
  const root = mkdtempSync(join(tmpdir(), "s2lib-of-lib-s-"));
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify({
      name: "root-ws",
      private: true,
      workspaces: ["libs/*"],
      s2script: { workspace: { plugins: ["plugins/*"] } },
    }),
  );
  const consumerDir = join(root, "libs", "consumer-lib");
  mkdirSync(join(consumerDir, "src"), { recursive: true });
  writeFileSync(
    join(consumerDir, "package.json"),
    JSON.stringify({
      name: "@edge/shout-sibling",
      version: "1.0.0",
      main: "src/index.ts",
      types: "src/index.d.ts",
      s2script: { kind: "library", libraries: { "@edge/sibling-inner": "^1.0.0" } },
    }),
  );
  writeFileSync(
    join(consumerDir, "src", "index.ts"),
    'import { inner } from "@edge/sibling-inner";\n\nexport function shout(): string {\n  return inner();\n}\n',
  );
  writeFileSync(join(consumerDir, "src", "index.d.ts"), "export declare function shout(): string;\n");

  // Directory name deliberately does NOT match the package name — resolution must be
  // name-matched against the sibling's own package.json, never directory-matched.
  const innerDir = join(root, "libs", "unrelated-dirname");
  mkdirSync(join(innerDir, "src"), { recursive: true });
  writeFileSync(
    join(innerDir, "src", "index.ts"),
    'export function inner(): string {\n  return "sibling-inner-resolved";\n}\n',
  );
  writeFileSync(join(innerDir, "src", "index.d.ts"), "export declare function inner(): string;\n");
  writeFileSync(
    join(innerDir, "package.json"),
    JSON.stringify({
      name: "@edge/sibling-inner",
      version: "1.0.0",
      main: "src/index.ts",
      types: "src/index.d.ts",
      s2script: { kind: "library" },
    }),
  );
  return consumerDir;
}

test("buildLibrary inlines a VENDORED library-of-a-library's real code, not an external require", async () => {
  const dir = vendoredLibraryOfLibrary();
  const out = await buildLibrary(dir);
  const entries = unzipSync(new Uint8Array(readFileSync(out)));
  const js = Buffer.from(entries["index.js"]).toString("utf8");
  assert.match(js, /vendored-inner-resolved/);
  assert.doesNotMatch(js, /require\("@edge\/vendored-inner"\)/);
});

test("buildLibrary inlines a WORKSPACE-SIBLING library-of-a-library's real code, not an external require", async () => {
  const consumerDir = siblingLibraryOfLibrary();
  const out = await buildLibrary(consumerDir);
  const entries = unzipSync(new Uint8Array(readFileSync(out)));
  const js = Buffer.from(entries["index.js"]).toString("utf8");
  assert.match(js, /sibling-inner-resolved/);
  assert.doesNotMatch(js, /require\("@edge\/sibling-inner"\)/);
});

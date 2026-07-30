/**
 * TDD test: the s2script.libraries resolver — a vendored copy under `.s2script/libs/`, then a
 * workspace sibling.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/libraries.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { librariesRoot, localLibraryDir, resolveLibraries, assertLibrariesResolved } from "../src/libraries.ts";
import { loadWorkspace } from "../src/workspace/workspace.ts";
import { buildPlugin } from "../src/build.ts";
import { openZip } from "./zip.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..", ".."); // test/ -> sdk/ -> packages/ -> repo
const packagesDir = join(repoRoot, "packages");

function vendored(name, { version = "1.0.0", apiVersion = "1.x" } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "s2libs-"));
  const libDir = join(dir, ".s2script", "libs", ...name.split("/"));
  mkdirSync(libDir, { recursive: true });
  writeFileSync(join(libDir, "index.js"), "module.exports = {};\n");
  writeFileSync(join(libDir, "index.d.ts"), "export declare function encode(s: string): string;\n");
  writeFileSync(
    join(libDir, "package.json"),
    JSON.stringify({ name, version, main: "index.js", types: "index.d.ts", s2script: { apiVersion } }),
  );
  return dir;
}

test("librariesRoot is the vendored tree the build resolves against", () => {
  assert.equal(librariesRoot("/p"), join("/p", ".s2script", "libs"));
});

test("localLibraryDir finds a vendored library and rejects traversal", () => {
  const dir = vendored("@edge/base64");
  assert.ok(localLibraryDir(dir, "@edge/base64"));
  assert.equal(localLibraryDir(dir, "@edge/missing"), null);
  assert.equal(localLibraryDir(dir, "../../etc"), null);
});

test("resolveLibraries maps each declared library to its .d.ts", () => {
  const dir = vendored("@edge/base64");
  const r = resolveLibraries(dir, { "@edge/base64": "^1.0.0" });
  assert.deepEqual(r.missing, []);
  assert.deepEqual(r.paths["@edge/base64"], [
    join(dir, ".s2script", "libs", "@edge", "base64", "index.d.ts"),
  ]);
  assert.equal(r.manifests["@edge/base64"].version, "1.0.0");
});

test("a declared-but-unvendored library is a hard error naming the fix", () => {
  const dir = vendored("@edge/base64");
  assert.throws(
    () => assertLibrariesResolved(dir, { "@edge/other": "^1.0.0" }, "1.x"),
    /s2s add @edge\/other/,
  );
});

test("a library built against a newer api major is refused", () => {
  const dir = vendored("@edge/base64", { apiVersion: "2.x" });
  assert.throws(
    () => assertLibrariesResolved(dir, { "@edge/base64": "^1.0.0" }, "1.x"),
    /apiVersion/,
  );
});

// ---------------------------------------------------------------------------
// The second resolution source: a workspace sibling. `.s2script/libs/` is right for a library
// pulled from the registry; re-vendoring a sibling's copy after every edit is untenable in a
// monorepo, so a sibling resolves straight to its own `main`/`types` on disk instead.
// ---------------------------------------------------------------------------

/**
 * A workspace root with one consumer plugin (covered by both `workspaces` and
 * `s2script.workspace.plugins`) and one library member under `libs/`. The library's directory
 * name deliberately does NOT match its package name — proving resolution is name-matched against
 * the sibling's own package.json, not directory-matched the way the vendored tree is.
 */
function workspaceWithLibrary(libName, { hasKind = true, asPlugin = false, withBrokenSibling = false } = {}) {
  const root = mkdtempSync(join(tmpdir(), "s2ws-"));
  const consumerDir = join(root, "plugins", "consumer");
  mkdirSync(consumerDir, { recursive: true });
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify({
      name: "root-ws",
      private: true,
      workspaces: ["plugins/*", "libs/*"],
      s2script: { workspace: { plugins: ["plugins/*"] } },
    }),
  );
  writeFileSync(
    join(consumerDir, "package.json"),
    JSON.stringify({ name: "@acme/consumer", version: "1.0.0", main: "index.js" }),
  );

  if (withBrokenSibling) {
    // §4.3 rule 2: a plugin package.json with no entry point is a HARD ERROR under `loadWorkspace`.
    // Present here to prove `resolveLibrarySibling` survives it (it uses `scanWorkspace`, which
    // COLLECTS this as a problem rather than throwing).
    const brokenDir = join(root, "plugins", "broken");
    mkdirSync(brokenDir, { recursive: true });
    writeFileSync(join(brokenDir, "package.json"), JSON.stringify({ name: "@acme/broken", version: "1.0.0" }));
  }

  const memberRel = asPlugin ? join("plugins", "a-lib-plugin") : join("libs", "unrelated-dirname");
  const memberDir = join(root, memberRel);
  mkdirSync(memberDir, { recursive: true });
  writeFileSync(join(memberDir, "main.js"), "module.exports = {};\n");
  writeFileSync(join(memberDir, "main.d.ts"), "export declare function encode(s: string): string;\n");
  const memberPkg = {
    name: libName,
    version: "1.0.0",
    main: "main.js",
    types: "main.d.ts",
    ...(hasKind ? { s2script: { kind: "library" } } : {}),
  };
  writeFileSync(join(memberDir, "package.json"), JSON.stringify(memberPkg));

  return { root, consumerDir, memberDir };
}

test("resolveLibraries finds a workspace sibling when there is no vendored copy", () => {
  const { consumerDir, memberDir } = workspaceWithLibrary("@acme/base64");
  const r = resolveLibraries(consumerDir, { "@acme/base64": "^1.0.0" });
  assert.deepEqual(r.missing, []);
  assert.deepEqual(r.paths["@acme/base64"], [join(memberDir, "main.d.ts")]);
  assert.equal(r.manifests["@acme/base64"].source, "workspace");
  assert.equal(r.siblingDirs["@acme/base64"], memberDir);
});

test("a vendored copy wins over a workspace sibling of the same name", () => {
  const { consumerDir } = workspaceWithLibrary("@acme/base64");
  const vendoredLibDir = join(consumerDir, ".s2script", "libs", "@acme", "base64");
  mkdirSync(vendoredLibDir, { recursive: true });
  writeFileSync(join(vendoredLibDir, "index.js"), "module.exports = {};\n");
  writeFileSync(join(vendoredLibDir, "index.d.ts"), "export declare function decode(s: string): string;\n");
  writeFileSync(
    join(vendoredLibDir, "package.json"),
    JSON.stringify({ name: "@acme/base64", version: "9.9.9", main: "index.js", types: "index.d.ts" }),
  );

  const r = resolveLibraries(consumerDir, { "@acme/base64": "^1.0.0" });
  assert.deepEqual(r.paths["@acme/base64"], [join(vendoredLibDir, "index.d.ts")]);
  assert.equal(r.manifests["@acme/base64"].source, "vendored");
  assert.equal(r.siblingDirs["@acme/base64"], undefined, "no alias needed for a vendored resolution");
});

test("a workspace member without s2script.kind === 'library' does not resolve as one", () => {
  const { consumerDir } = workspaceWithLibrary("@acme/shared-utils", { hasKind: false });
  const r = resolveLibraries(consumerDir, { "@acme/shared-utils": "^1.0.0" });
  assert.deepEqual(r.missing, ["@acme/shared-utils"], "a plain shared-code member is not a library");
});

test("a plugin sibling with a matching name does not resolve as a library", () => {
  const { consumerDir } = workspaceWithLibrary("@acme/decoy", { asPlugin: true });
  const r = resolveLibraries(consumerDir, { "@acme/decoy": "^1.0.0" });
  assert.deepEqual(r.missing, ["@acme/decoy"]);
});

test("assertLibrariesResolved never gates a workspace sibling's apiVersion", () => {
  const { consumerDir } = workspaceWithLibrary("@acme/base64");
  // sdkApiVersion "0.x" (major 0) would refuse ANY vendored entry outright (major(...) defaults to
  // 1 for a missing/blank apiVersion, and 1 > 0) — proving the "workspace" source really does skip
  // the gate rather than merely happening to pass it.
  assert.doesNotThrow(() => assertLibrariesResolved(consumerDir, { "@acme/base64": "^1.0.0" }, "0.x"));
});

test("a workspace sibling outside the workspace's own coverage is not a library sibling", () => {
  // A directory that merely sits BENEATH a workspace root without being one of its covered
  // members (mirrors workspace/siblings.ts's blast-radius guard) must not go sibling-hunting.
  const { root } = workspaceWithLibrary("@acme/base64");
  const uncovered = join(root, "elsewhere", "standalone-plugin");
  mkdirSync(uncovered, { recursive: true });
  const r = resolveLibraries(uncovered, { "@acme/base64": "^1.0.0" });
  assert.deepEqual(r.missing, ["@acme/base64"]);
});

test("a workspace sibling library resolves despite an UNRELATED malformed sibling", () => {
  // `resolveLibrarySibling` must use `scanWorkspace` (collects problems), not `loadWorkspace`
  // (throws on ANY workspace-wide shape problem) — otherwise a typo in some other plugin's
  // package.json would break `s2s build plugins/healthy-plugin` merely for declaring a library.
  // Mirrors workspace/siblings.ts's identical fixture/rationale for sibling CONTRACT resolution.
  const { consumerDir, memberDir } = workspaceWithLibrary("@acme/base64", { withBrokenSibling: true });
  const r = resolveLibraries(consumerDir, { "@acme/base64": "^1.0.0" });
  assert.deepEqual(r.missing, [], "one broken, unrelated plugin must not take a healthy library resolution down");
  assert.deepEqual(r.paths["@acme/base64"], [join(memberDir, "main.d.ts")]);
  // And nothing is swept under the rug: the workspace-mode path still refuses the same tree.
  assert.throws(() => loadWorkspace(join(consumerDir, "..", "..")), /plugins\/broken: .* has no entry point/);
});

// ---------------------------------------------------------------------------
// Integration: the real `buildPlugin()`, not just the resolver in isolation. Proves the
// interaction of `nodePaths` + `alias` + `mainFields` + the typecheck `paths` merge inside the
// actual esbuild.build()/typecheckPlugin() calls — a unit test of libraries.ts alone cannot catch
// a regression in that wiring.
// ---------------------------------------------------------------------------

function vendoredLibraryPlugin() {
  const dir = mkdtempSync(join(tmpdir(), "s2libv-"));
  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify({
      name: "@fixture/lib-vendored-consumer",
      version: "0.1.0",
      main: "src/plugin.ts",
      private: true,
      s2script: { libraries: { "@fixture/vendored-lib": "^1.0.0" } },
    }),
  );
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(
    join(dir, "src", "plugin.ts"),
    'import { plugin } from "@s2script/sdk/plugin";\n' +
      'import { hello } from "@fixture/vendored-lib";\n\n' +
      "export default plugin((ctx) => {\n" +
      '  ctx.commands.register("fixture_libv", (cmd) => { cmd.reply(hello()); });\n' +
      "});\n",
  );
  const libDir = join(dir, ".s2script", "libs", "@fixture", "vendored-lib");
  mkdirSync(libDir, { recursive: true });
  writeFileSync(join(libDir, "index.js"), 'module.exports = { hello: () => "library-vendored-resolved" };\n');
  writeFileSync(join(libDir, "index.d.ts"), "export declare function hello(): string;\n");
  writeFileSync(
    join(libDir, "package.json"),
    JSON.stringify({ name: "@fixture/vendored-lib", version: "1.0.0", main: "index.js", types: "index.d.ts" }),
  );
  return dir;
}

function siblingLibraryWorkspace() {
  const root = mkdtempSync(join(tmpdir(), "s2libsibws-"));
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify({
      name: "root-ws",
      private: true,
      workspaces: ["plugins/*", "libs/*"],
      s2script: { workspace: { plugins: ["plugins/*"] } },
    }),
  );
  const consumerDir = join(root, "plugins", "consumer");
  mkdirSync(join(consumerDir, "src"), { recursive: true });
  writeFileSync(
    join(consumerDir, "package.json"),
    JSON.stringify({
      name: "@fixture/lib-sibling-consumer",
      version: "0.1.0",
      main: "src/plugin.ts",
      private: true,
      s2script: { libraries: { "@fixture/sibling-lib": "^1.0.0" } },
    }),
  );
  writeFileSync(
    join(consumerDir, "src", "plugin.ts"),
    'import { plugin } from "@s2script/sdk/plugin";\n' +
      'import { hello } from "@fixture/sibling-lib";\n\n' +
      "export default plugin((ctx) => {\n" +
      '  ctx.commands.register("fixture_libs", (cmd) => { cmd.reply(hello()); });\n' +
      "});\n",
  );
  // Directory name deliberately does NOT match the package name (same proof as the unit tests
  // above) — resolution must be name-matched, not directory-matched.
  const libDir = join(root, "libs", "unrelated-dirname2");
  mkdirSync(join(libDir, "src"), { recursive: true });
  writeFileSync(
    join(libDir, "src", "index.ts"),
    'export function hello(): string {\n  return "library-sibling-resolved";\n}\n',
  );
  writeFileSync(join(libDir, "src", "index.d.ts"), "export declare function hello(): string;\n");
  writeFileSync(
    join(libDir, "package.json"),
    JSON.stringify({
      name: "@fixture/sibling-lib",
      version: "1.0.0",
      main: "src/index.ts",
      types: "src/index.d.ts",
      s2script: { kind: "library" },
    }),
  );
  return { root, consumerDir };
}

test("build inlines a VENDORED library's real code into plugin.js, not an external require", async () => {
  const dir = vendoredLibraryPlugin();
  const out = await buildPlugin(dir, packagesDir);
  const zip = openZip(out);
  const js = zip.readAsText("plugin.js");
  assert.ok(
    js.includes("library-vendored-resolved"),
    `the vendored library's source must be inlined — got:\n${js.slice(0, 500)}`,
  );
  assert.ok(!js.includes('require("@fixture/vendored-lib")'), "must not be left as an external require");
});

test("build inlines a WORKSPACE-SIBLING library's real code into plugin.js, not an external require", async () => {
  const { consumerDir } = siblingLibraryWorkspace();
  const out = await buildPlugin(consumerDir, packagesDir);
  const zip = openZip(out);
  const js = zip.readAsText("plugin.js");
  assert.ok(
    js.includes("library-sibling-resolved"),
    `the sibling library's source must be inlined — got:\n${js.slice(0, 500)}`,
  );
  assert.ok(!js.includes('require("@fixture/sibling-lib")'), "must not be left as an external require");
});

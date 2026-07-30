/**
 * buildLibrary: a library package -> dist/<sanitized-name>.s2lib.
 *
 * A .s2lib mirrors .s2sp deliberately (one archive concept in the system), but core
 * never loads one: a library is BUILD-TIME code that the consumer's esbuild bundles
 * into its own plugin.js. `apiVersion` is still stamped because a library may import
 * @s2script/sdk/*, and the CONSUMER's build refuses a library from a newer major.
 *
 * The library's own s2script.libraries are INLINED here (design D3), so a published
 * library is one self-contained file and there is no transitive graph anywhere.
 */

import * as esbuild from "esbuild";
import { zipSync } from "fflate";
import { readFileSync, writeFileSync, mkdirSync, existsSync, statSync } from "node:fs";
import { resolve, join } from "node:path";
import { STAMPED_API_VERSION } from "./api-version.ts";
import { assertLibrariesResolved, librariesRoot } from "./libraries.ts";
import { assertNoNpmRuntimeDeps } from "./npm-deps-gate.ts";
import { typecheckPlugin, formatDiagnostics } from "./typecheck/typecheck.ts";
import { lintPlugin } from "./lint/lint.ts";

// Re-exported (not defined here): `resolveLibrarySibling` in libraries.ts needs `packageKind` too,
// and libraries.ts already imports FROM this file's `assertLibrariesResolved`/`librariesRoot`
// above — defining it there and re-exporting it here is what keeps every EXISTING import of
// `packageKind`/`PackageKind` from `./build-library.ts` (build.ts, registry/deploy.ts, tests)
// working with no edits, while avoiding the two modules importing each other.
export type { PackageKind } from "./libraries.ts";
export { packageKind } from "./libraries.ts";

export async function buildLibrary(dir: string, packagesDir?: string): Promise<string> {
  const absDir = resolve(dir);
  const pkgPath = join(absDir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  const s2 = pkg.s2script ?? {};

  // A cross-plugin proxy is a ledgered runtime relationship owned by a LOADED plugin.
  // Build-time code cannot hold one, so declaring it would be a lie the loader never sees.
  if (s2.pluginDependencies || s2.optionalPluginDependencies) {
    throw new Error(
      "a library may not declare pluginDependencies/optionalPluginDependencies — an inter-plugin " +
        "proxy belongs to a loaded plugin's ledger, not to build-time code",
    );
  }
  if (s2.publishes) {
    throw new Error("a library may not declare publishes — publishes is a plugin's runtime contract");
  }

  const typesRel = pkg.types ?? pkg.typings;
  if (typeof typesRel !== "string" || !typesRel.endsWith(".d.ts")) {
    throw new Error('a library must set "types" to a .d.ts — consumers vendor it to typecheck against');
  }
  const typesPath = resolve(absDir, typesRel);
  if (!existsSync(typesPath) || !statSync(typesPath).isFile() || statSync(typesPath).size === 0) {
    throw new Error(`types file is missing or empty: ${typesRel}`);
  }

  assertNoNpmRuntimeDeps(pkg, absDir);
  // The return value matters, not just the fail-fast: `siblingDirs` is the esbuild alias map for
  // a WORKSPACE-sibling library (Task 6) — it has no vendored copy under `.s2script/libs/`, so
  // `nodePaths` below cannot find it unaided. Discarding this (as an earlier version of this file
  // did) typechecks a sibling-of-a-sibling fine and then fails esbuild with "Could not resolve" —
  // caught only by actually building one, which is why build.ts's own `alias:` wiring is mirrored
  // here rather than assumed to be unnecessary for a library.
  const librariesResolved = assertLibrariesResolved(absDir, s2.libraries ?? {}, STAMPED_API_VERSION);

  const tc = typecheckPlugin(absDir, packagesDir !== undefined ? { packagesDir } : undefined);
  if (!tc.ok) {
    throw new Error(`typecheck failed (${tc.diagnostics.length} error(s)):\n${formatDiagnostics(tc.diagnostics)}`);
  }

  // Residual-rule lint gate (B2), the same one `buildPlugin` runs, AFTER tsc — before this a
  // library's own source was linted by NOTHING: `lint/lint.ts`'s own directory walk skips
  // dot-directories (so a vendored `.s2script/libs/` copy is invisible to it, and it's `.js` by
  // then anyway) and scopes its targets to the CONSUMER's plugin dir, so a workspace-sibling
  // library sitting two directories away is out of range either way. Two of the four pinned rules
  // — `no-await-in-raw-view` and `no-bigint-in-interface-payloads` — describe hazards that fail
  // SILENTLY at runtime and apply verbatim to library code, which executes inside the consumer's
  // plugin context once bundled in.
  const lint = await lintPlugin(absDir, tc.program!);
  if (!lint.ok) {
    throw new Error(`lint failed (${lint.errorCount} error(s)):\n${lint.output}`);
  }
  if (lint.output.trim().length > 0) console.warn(lint.output);

  const entryRelative = s2.main ?? pkg.main;
  if (!entryRelative) throw new Error(`buildLibrary: no entry point in ${pkgPath} (set s2script.main or main)`);

  const result = await esbuild.build({
    entryPoints: [join(absDir, entryRelative)],
    bundle: true,
    platform: "neutral",
    format: "cjs",
    // Builtins resolve in the CONSUMER's context at runtime, so they stay external
    // through both bundles. Everything else — including this library's own libraries
    // — is inlined, which is what makes a .s2lib self-contained.
    external: ["@s2script/*"],
    target: "es2020",
    mainFields: ["module", "main"],
    // A vendored library's directory is already named for its package, so `nodePaths` alone
    // resolves it. A WORKSPACE-sibling library (librariesResolved.siblingDirs) is not, hence the
    // alias — same reasoning `build.ts` applies to a plugin's own library imports.
    nodePaths: [librariesRoot(absDir)],
    alias: librariesResolved.siblingDirs,
    write: false,
  });

  const manifest = {
    kind: "library" as const,
    id: pkg.name,
    version: pkg.version,
    apiVersion: STAMPED_API_VERSION,
    main: "index.js",
    types: "index.d.ts",
  };

  const sanitized = String(pkg.name).replace(/[^a-zA-Z0-9._-]/g, "_");
  const outDir = join(absDir, "dist");
  mkdirSync(outDir, { recursive: true });
  const outPath = join(outDir, `${sanitized}.s2lib`);
  writeFileSync(
    outPath,
    zipSync({
      "manifest.json": Buffer.from(JSON.stringify(manifest, null, 2)),
      "index.js": Buffer.from(result.outputFiles[0].text),
      "index.d.ts": readFileSync(typesPath),
    }),
  );
  return outPath;
}

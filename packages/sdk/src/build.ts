/**
 * buildPlugin: reads a plugin directory, bundles the TypeScript entry with
 * esbuild (CJS, @s2script/* external), derives manifest.json, and zips both
 * into <dir>/dist/<sanitized-id>.s2sp.
 *
 * The .s2sp format is consumed by core's read_s2sp (loader.rs):
 *   - zip must contain exactly "manifest.json" + "plugin.js"
 *   - manifest.json must have keys: id, version, apiVersion (serde rename on
 *     api_version field in loader::Manifest), pluginDependencies, publishes
 */

import * as esbuild from "esbuild";
import { zipSync } from "fflate";
import { readFileSync, writeFileSync, mkdirSync, rmSync } from "node:fs";
import { resolve, join, sep, basename } from "node:path";
import { typecheckPlugin, formatDiagnostics } from "./typecheck/typecheck.ts";
import { validateConfigBlock } from "./config-validate.ts";
import { assertPublishesTypes, hasPublishes } from "./publish-gate.ts";
import { derivePublishes, hashContract, expandPublishes } from "./publishes.ts";
import { STAMPED_API_VERSION } from "./api-version.ts";
import { localContractPath } from "./contracts.ts";
import { resolveSiblingContracts, shadowedCopyWarning } from "./workspace/siblings.ts";
import { scanPluginProgram } from "./publish-scan.ts";
import { lintPlugin } from "./lint/lint.ts";
import { validatePluginGamedata } from "./gamedata/validate.ts";
import { generateGamedataTypes } from "./gamedata/gen-types.ts";
import { stripJsonComments } from "./gamedata/jsonc.ts";
import type { PluginGamedata } from "./gamedata/types.ts";

/** Shape of plugin package.json (the fields we care about). */
interface PluginPackageJson {
  name: string;
  version: string;
  main?: string;
  types?: string;
  s2script?: {
    apiVersion?: string;
    main?: string;
    pluginDependencies?: Record<string, string>;
    optionalPluginDependencies?: Record<string, string>;
    publishes?: string | Record<string, string>;
    config?: Record<string, unknown>;
    /** Path (relative to the plugin dir) of this plugin's own gamedata (JSONC). */
    gamedata?: string;
    /** Declared capabilities, e.g. ["engine:calls"]. Auditable; the operator allow-list decides. */
    permissions?: string[];
  };
}

/**
 * Bundle the plugin at `dir`, produce a .s2sp archive, return the output path.
 * @param dir         Path to the plugin directory (absolute or relative to cwd).
 * @param packagesDir Optional path to packages/ or node_modules/@s2script (for @s2script/* .d.ts).
 *                    When omitted, resolved via env / monorepo / plugin node_modules.
 */
export async function buildPlugin(dir: string, packagesDir?: string): Promise<string> {
  const absDir = resolve(dir);

  // --- Read package.json ONCE; every step below reuses this parse. ---
  const pkgPath = join(absDir, "package.json");
  const pkg: PluginPackageJson = JSON.parse(readFileSync(pkgPath, "utf8"));

  const s2 = pkg.s2script ?? {};

  // --- Cheap fail-fast: config block shape (no program needed). ---
  const config = s2.config ?? undefined;
  if (config !== undefined) {
    const cfgErrs = validateConfigBlock(config);
    if (cfgErrs.length) throw new Error(`invalid s2script.config:\n  ${cfgErrs.join("\n  ")}`);
  }

  // --- Cheap fail-fast: the plugin's own gamedata (declared engine calls). Parsed and validated
  //     BEFORE the typecheck gate so a bad descriptor is reported as a gamedata error rather than
  //     as a downstream TS error, and so the generated types exist for the gate below. ---
  const permissions: string[] = Array.isArray(s2.permissions) ? s2.permissions : [];
  let gamedata: PluginGamedata | undefined;
  if (typeof s2.gamedata === "string") {
    const gdPath = resolve(absDir, s2.gamedata);
    // Containment: the gamedata must live inside the plugin directory. `s2script.gamedata` is an
    // author-supplied path, and a build must never read (and then PACK) a file from outside the
    // package — the same class of thing commit 7f45a70 hardened for install.
    const dirPrefix = absDir.endsWith(sep) ? absDir : absDir + sep;
    if (gdPath !== absDir && !gdPath.startsWith(dirPrefix)) {
      throw new Error(
        `s2script.gamedata escapes the plugin directory: ${JSON.stringify(s2.gamedata)} resolves to ${gdPath}`
      );
    }
    // Naming convention: `<plugin-name-without-scope>.gamedata.jsonc`, mirroring the framework's own
    // `gamedata/core.gamedata.jsonc` — a gamedata file is named for whoever owns it. Enforced rather
    // than documented so the convention actually holds: `@me/burn` -> `burn.gamedata.jsonc`.
    const unscoped = pkg.name.includes("/") ? pkg.name.slice(pkg.name.lastIndexOf("/") + 1) : pkg.name;
    const actualBase = basename(gdPath);
    const allowedBases = [`${unscoped}.gamedata.jsonc`, `${unscoped}.gamedata.json`];
    if (!allowedBases.includes(actualBase)) {
      throw new Error(
        `s2script.gamedata must be named '${unscoped}.gamedata.jsonc' (after the plugin name without ` +
          `its scope, like the framework's own core.gamedata.jsonc), but is '${actualBase}'`
      );
    }
    // Full JSONC: trailing `// …` and `/* … */` too, string-aware. gamedata/core.gamedata.jsonc —
    // which authors are told to imitate — uses both freely, so a line-only stripper rejected the
    // house style. See gamedata/jsonc.ts.
    const raw = stripJsonComments(readFileSync(gdPath, "utf8"));
    try {
      gamedata = JSON.parse(raw) as PluginGamedata;
    } catch (e) {
      throw new Error(`invalid gamedata ${gdPath}: ${(e as Error).message}`);
    }
    const gdErrs = validatePluginGamedata(gamedata, { permissions });
    if (gdErrs.length) throw new Error(`invalid gamedata:\n  ${gdErrs.join("\n  ")}`);

    // Layout is data, semantics are code: the EngineCalls types are DERIVED from the gamedata.
    // Written before the gate so a wrong arity/arg type at a call site fails the build (the file
    // is a typecheck root — see typecheck.ts generatedDeclarationFiles).
    const genDir = join(absDir, ".s2script");
    mkdirSync(genDir, { recursive: true });
    writeFileSync(join(genDir, "gamedata.d.ts"), generateGamedataTypes(gamedata));
  } else {
    // No gamedata: DELETE any previously generated types. Leaving them behind is not merely untidy —
    // typecheck.ts loads the file as a root, so a stale copy makes the gate certify Engine.call()s
    // that the emitted .s2sp (which now carries no gamedata.json) provably cannot make. Same hazard
    // when a call is renamed or dropped, which is why this runs unconditionally rather than only
    // when the gamedata key is removed.
    rmSync(join(absDir, ".s2script", "gamedata.d.ts"), { force: true });
  }

  // --- Workspace siblings (design spec 2026-07-27 §5.3): which declared dependencies resolve to a
  //     producer sitting right here on disk. Resolved BEFORE the gate so the two named sibling
  //     errors (§11 — a sibling with no `publishes`, or with `publishes` but no usable `types`)
  //     surface as themselves rather than as a downstream TS2307. Empty outside a workspace, which
  //     is the compatibility hinge: every existing plugin builds exactly as it did. ---
  const declaredDepNames = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
  ];
  const siblingContracts = resolveSiblingContracts(absDir, declaredDepNames);
  for (const dep of siblingContracts.shadowedCopies) console.warn(shadowedCopyWarning(absDir, dep));

  // --- Typecheck gate (Slice 5E.1): full strict against the shipped engine .d.ts. No .s2sp on
  //     error. Runs FIRST now: the program it builds feeds the publishes/use derivation (B1)
  //     and the lint gate (B2). ---
  const tc = typecheckPlugin(absDir, packagesDir !== undefined ? { packagesDir } : undefined);
  if (!tc.ok) {
    throw new Error(`typecheck failed (${tc.diagnostics.length} error(s)):\n${formatDiagnostics(tc.diagnostics)}`);
  }
  const scan = scanPluginProgram(tc.program!, absDir);

  // --- Residual-rule lint gate (B2): the pinned eslint-plugin-s2script rules, in-process,
  //     AFTER tsc (spec §5.3). Errors abort the build — no .s2sp. Warnings pass through.
  const lint = await lintPlugin(absDir, tc.program!);
  if (!lint.ok) {
    throw new Error(`lint failed (${lint.errorCount} error(s)):\n${lint.output}`);
  }
  if (lint.output.trim().length > 0) console.warn(lint.output);

  // --- publishes: reconciliation IS generation (north-star §5.2). The name-set comes from code;
  //     "self" is auto-derived; an authored block must agree exactly; dynamic names are refused. ---
  if (scan.dynamicPublishSites.length > 0) {
    throw new Error(
      `ctx.publish name must be a string literal (the manifest publishes block is derived from code):\n  ` +
        scan.dynamicPublishSites.join("\n  "),
    );
  }
  let effectivePublishes = s2.publishes;
  if (!hasPublishes(effectivePublishes)) {
    if (scan.publishNames.length === 1 && scan.publishNames[0] === pkg.name) {
      effectivePublishes = "self"; // generated: code publishes exactly this package's own contract
    } else if (scan.publishNames.length > 0) {
      throw new Error(
        `code publishes ${JSON.stringify(scan.publishNames)} but s2script.publishes is missing — ` +
          `a contract named differently from the package needs an authored entry with a concrete version`,
      );
    }
  } else {
    const authoredNames = Object.keys(expandPublishes(effectivePublishes, pkg.name, pkg.version)).sort();
    const codeNames = [...scan.publishNames].sort();
    if (JSON.stringify(authoredNames) !== JSON.stringify(codeNames)) {
      throw new Error(
        `publishes drift: package.json declares ${JSON.stringify(authoredNames)} but the code's ` +
          `ctx.publish calls are ${JSON.stringify(codeNames)} — fix whichever is wrong (the manifest ` +
          `is generated from code; the loader re-verifies at Active)`,
      );
    }
  }

  // --- publishes ⇒ types gate + hash (unchanged mechanics, now fed the EFFECTIVE block). ---
  const gate = assertPublishesTypes({ ...pkg, s2script: { ...s2, publishes: effectivePublishes as never } }, absDir);
  if (!gate.ok) {
    throw new Error(`publish gate failed: ${gate.error}`);
  }
  const derivedPublishes = derivePublishes(
    effectivePublishes as never, pkg.name, pkg.version, gate.typesPath,
  );

  // --- Dependency advisories (lint-grade, WARN not error — spec §5.2 table, last row). ---
  const pluginDependencies = s2.pluginDependencies ?? {};
  const optionalPluginDependencies = s2.optionalPluginDependencies ?? {};
  const declaredDeps = new Set([
    ...Object.keys(pluginDependencies),
    ...Object.keys(optionalPluginDependencies),
  ]);
  for (const used of scan.useNames) {
    if (!declaredDeps.has(used)) {
      console.warn(
        `WARN: ctx.use/tryUse(${JSON.stringify(used)}) is not declared under s2script.pluginDependencies/` +
          `optionalPluginDependencies — it will throw at runtime`,
      );
    }
  }
  for (const dep of declaredDeps) {
    if (!scan.useNames.includes(dep)) {
      console.warn(`WARN: dependency ${JSON.stringify(dep)} is declared but never ctx.use()d`);
    }
  }

  const { name, version } = pkg;
  // --- apiVersion is DERIVED at build (north-star §5.2, locked decision #6). The SDK stamps the
  // host major it types; an authored s2script.apiVersion is vestigial and ignored (warn so authors
  // delete it). The loader's major gate stays as the runtime backstop for stale .s2sp files.
  if (s2.apiVersion !== undefined) {
    console.warn(
      `WARN: ${pkgPath}: s2script.apiVersion is ignored — s2s build derives apiVersion from the ` +
        `SDK (stamping ${JSON.stringify(STAMPED_API_VERSION)}). Remove the field.`,
    );
  }
  const apiVersion = STAMPED_API_VERSION;

  // Every builtin package + every inter-plugin dependency name is esbuild-external (resolved at
  // runtime by core, never bundled).
  const external = Array.from(new Set([
    "@s2script/*",
    ...Object.keys(pluginDependencies),
    ...Object.keys(optionalPluginDependencies),
  ]));

  // Entry point: s2script.main takes precedence, then package.main.
  const entryRelative = s2.main ?? pkg.main;
  if (!entryRelative) {
    throw new Error(
      `buildPlugin: no entry point found in ${pkgPath} (set s2script.main or main)`
    );
  }
  const entryPoint = join(absDir, entryRelative);

  // --- Bundle with esbuild ---
  const result = await esbuild.build({
    entryPoints: [entryPoint],
    bundle: true,
    platform: "neutral",
    format: "cjs",
    external,
    target: "es2020",
    // platform:"neutral" defaults mainFields to EMPTY, so a workspace-sibling package that
    // declares `main` (rather than `exports`) fails to resolve with an undiagnosable error.
    // Set it explicitly so a monorepo plugin bundles whichever field the author wrote.
    mainFields: ["module", "main"],
    write: false,
  });

  const pluginJs = result.outputFiles[0].text;

  // --- Derive manifest (keys must match loader::Manifest serde fields) ---
  // loader.rs: id (String), version (String), api_version (#[serde(rename="apiVersion")] String)
  const manifest: Record<string, unknown> = {
    id: name,
    version,
    apiVersion,           // <-- MUST be "apiVersion" to match #[serde(rename = "apiVersion")]
    pluginDependencies,
    optionalPluginDependencies,
  };
  // publishes.ts owns the grammar; the block was derived + validated up front (fail fast).
  if (Object.keys(derivedPublishes).length > 0) {
    manifest.publishes = derivedPublishes;
  }
  if (config !== undefined) manifest.config = config;
  // Declared capabilities travel with the package so `s2s install` can surface them and the loader
  // can gate the gamedata calls against the operator allow-list (default-deny).
  if (permissions.length > 0) manifest.permissions = permissions;

  // --- compiledAgainst (B1): hash every contract this consumer typechecked against. The loader
  // compares these to the producer's published typesSha256 at load (fail-fast) and per-call
  // (late-producer backstop).
  //
  // A workspace sibling (§5.3) hashes the PRODUCER'S OWN `types` file, not a copy of it — no copy
  // is made anywhere. That is what makes the loader's drift check pass for structural reasons
  // rather than by luck: the producer hashes that same path for its own `publishes` block, so the
  // two sha256s are equal by construction. Everything else keeps the verified-copy path exactly.
  const compiledAgainst: Record<string, string> = {};
  for (const dep of [
    ...Object.keys(pluginDependencies),
    ...Object.keys(optionalPluginDependencies),
  ]) {
    const sibling = siblingContracts.siblings.get(dep);
    if (sibling !== undefined) {
      compiledAgainst[dep] = hashContract(sibling.typesPath);
      continue;
    }
    const contractPath = localContractPath(absDir, dep);
    if (contractPath !== null) compiledAgainst[dep] = hashContract(contractPath);
  }
  if (Object.keys(compiledAgainst).length > 0) manifest.compiledAgainst = compiledAgainst;

  // --- Zip manifest.json + plugin.js ---
  const zipFiles: Record<string, Uint8Array> = {
    "manifest.json": Buffer.from(JSON.stringify(manifest, null, 2)),
    "plugin.js": Buffer.from(pluginJs),
  };

  // --- The plugin's own gamedata rides along as its own member. read_s2sp reads manifest.json/
  // plugin.js by_name and ignores every other member, so an older runtime simply ignores it.
  if (gamedata) zipFiles["gamedata.json"] = Buffer.from(JSON.stringify(gamedata, null, 2));

  // --- Embedded verified copy (spec §4.5): redundant, hash-checked, NEVER authoritative.
  // core's read_s2sp reads manifest.json/plugin.js by_name and ignores every other member,
  // so this needs no loader change and can be dropped without breaking anyone.
  if (gate.typesPath !== null && Object.keys(derivedPublishes).length > 0) {
    const contract = readFileSync(gate.typesPath);
    for (const iface of Object.keys(derivedPublishes)) {
      const safe = iface.replace(/[^a-zA-Z0-9._-]/g, "_");
      zipFiles[`types/${safe}.d.ts`] = contract;
    }
  }

  // --- Write to dir/dist/<sanitized-id>.s2sp ---
  const sanitizedId = name.replace(/[^a-zA-Z0-9._-]/g, "_");
  const outDir = join(absDir, "dist");
  mkdirSync(outDir, { recursive: true });
  const outPath = join(outDir, `${sanitizedId}.s2sp`);
  // fflate emits standard DEFLATE members; core's read_s2sp (zip crate, deflate feature) reads them.
  writeFileSync(outPath, zipSync(zipFiles));

  return outPath;
}

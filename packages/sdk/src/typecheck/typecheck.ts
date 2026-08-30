import ts from "typescript";
import { existsSync, readdirSync, readFileSync, writeFileSync, rmSync, mkdtempSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { resolvePackagesDir } from "../packages-resolve.ts";
import { sharedProgramOptions } from "../tsconfig-shared.ts";
import { localContractPath } from "../contracts.ts";
import { resolveSiblingContracts } from "../workspace/siblings.ts";
import { resolveLibraries } from "../libraries.ts";

export interface TypecheckDiag { file: string; line: number; col: number; code: number; message: string; }
export interface TypecheckResult { ok: boolean; diagnostics: TypecheckDiag[]; program?: ts.Program; }

/** Every `.d.ts` the plugin ships under `src/` (non-recursive: matches the scaffold's layout).
 *  These are the plugin's own ambient declarations and belong in its typecheck. */
function localDeclarationFiles(pluginDir: string): string[] {
  const srcDir = join(pluginDir, "src");
  if (!existsSync(srcDir)) return [];
  return readdirSync(srcDir)
    .filter((f) => f.endsWith(".d.ts"))
    .map((f) => join(srcDir, f));
}

/** The build-generated gamedata augmentations (`.s2script/gamedata.d.ts`, `.s2script/hooks.d.ts`),
 *  when `s2s build` wrote them. They augment `@s2script/sdk/unsafe`'s `EngineCalls` / `EngineHooks`,
 *  and a module augmentation nothing imports is invisible to the program — so they MUST be typecheck
 *  ROOTs or every declared `Engine.call(name)` / `Engine.hook(name)` fails as `never`. */
function generatedDeclarationFiles(pluginDir: string): string[] {
  const out: string[] = [];
  for (const name of ["gamedata.d.ts", "hooks.d.ts"]) {
    const p = join(pluginDir, ".s2script", name);
    if (existsSync(p)) out.push(p);
  }
  return out;
}

/**
 * The game package's OWN generated ctx augmentation (e.g. `packages/cs2/hooks.generated.d.ts`,
 * `declare module "@s2script/sdk/plugin" { interface PluginContext { readonly gameRules: … } }`).
 *
 * Exactly the same reachability problem `generatedDeclarationFiles` exists for: a module
 * augmentation nothing imports is invisible to the program. A CS2 plugin whose source never
 * happens to import a NAME from `@s2script/cs2` (it may use only `ctx.gameRules`/`ctx.players` and
 * nothing else the package exports) would otherwise see no `ctx.gameRules` at all — `PluginContext`
 * would typecheck as if the game package had never loaded. Forced in whenever the plugin declares
 * `@s2script/cs2` as a (dev)dependency, matching the same declared-dependency signal `s2script`
 * itself uses to decide whether a plugin is a CS2 plugin; a no-op for a purely engine-generic
 * plugin, and a no-op today for any future `@s2script/<game>` that ships no such file yet. */
function gamePackageDeclarationFiles(pkg: { dependencies?: Record<string, string>; devDependencies?: Record<string, string> }, packagesDir: string): string[] {
  const deps = { ...(pkg.dependencies ?? {}), ...(pkg.devDependencies ?? {}) };
  if (!("@s2script/cs2" in deps)) return [];
  const hooks = join(packagesDir, "cs2", "hooks.generated.d.ts");
  return existsSync(hooks) ? [hooks] : [];
}

/** Module specifiers the plugin declares itself, e.g. `declare module "@demo/greeter" { … }`.
 *  Deliberately a scan, not a parse: we only need to know whether to skip generating a
 *  conflicting shorthand stub, and a false negative merely restores the old behaviour. */
function declaredModules(dtsFiles: string[]): Set<string> {
  const out = new Set<string>();
  for (const f of dtsFiles) {
    const body = readFileSync(f, "utf8");
    for (const m of body.matchAll(/declare\s+module\s+["']([^"']+)["']/g)) out.add(m[1]);
  }
  return out;
}

/** Typecheck a plugin dir (full strict) against the shipped engine .d.ts.
 *  `@s2script/sdk` → packagesDir/sdk/index.d.ts; `@s2script/sdk/*` → packagesDir/sdk/<cap>.d.ts;
 *  `@s2script/cs2` → packagesDir/cs2/index.d.ts;
 *  the global `console` -> packagesDir/sdk/globals.d.ts; each declared pluginDependency that is not
 *  always-resolved -> an ambient `declare module "<dep>";` (any). Never emits.
 *
 *  `packagesDir` may be omitted — resolved via monorepo packages/, env, or the plugin's
 *  node_modules/@s2script (see packages-resolve.ts). */
export function typecheckPlugin(pluginDir: string, opts?: { packagesDir?: string }): TypecheckResult {
  const absDir = resolve(pluginDir);
  const packagesDir = opts?.packagesDir
    ? resolve(opts.packagesDir)
    : resolvePackagesDir({ pluginDir: absDir });
  const pkg = JSON.parse(readFileSync(join(absDir, "package.json"), "utf8"));
  const s2 = pkg.s2script ?? {};
  const entryRel = s2.main ?? pkg.main;
  if (!entryRel) throw new Error(`typecheckPlugin: no entry point in ${join(absDir, "package.json")}`);
  const entry = resolve(absDir, entryRel);
  // A dep gets an ambient `declare module "<dep>";` (any) stub UNLESS it is always-resolved.
  //
  // Shape-based (post-consolidation): the framework builtins are `@s2script/sdk/<cap>` subpaths
  // and the game package is the separate scoped `@s2script/cs2` — both live in npm `dependencies`
  // and resolve via `paths` below (a miss = TS2307, a real error, never a silent `any`). Only
  // presence-conditional inter-plugin interfaces (a first-party plugin's PUBLISHED interface such
  // as `@s2script/zones`, or a third-party one) declared in pluginDependencies stub to `any` until
  // fetched. No disk-existence guess — the old check that made `@s2script/sdk/frmae` (a typo the
  // plugin DECLARES) stub to `any` instead of erroring is gone (the finding fix).
  //
  // The `any` stub above is the fallback for a declared dep with NO verified contract copy on
  // disk. When one exists, it resolves to REAL types instead (see the B1 block below) — a
  // consumer gets this by keeping a byte-copy of the producer's published `.d.ts` under
  // `.s2script/types/<iface>/index.d.ts` (see examples/cookbook's zones recipe for the pattern;
  // design spec 2026-07-15 §4.6, plan 2, landed as B1).
  const isAlwaysResolved = (d: string): boolean =>
    d === "@s2script/sdk" || d.startsWith("@s2script/sdk/") || d === "@s2script/cs2" || d.startsWith("@s2script/cs2/");

  // A plugin's OWN .d.ts files are part of its typecheck. They carry ambient declarations for
  // interfaces it consumes (see examples/*-consumer). Before this they were compiled only by the
  // editor via tsconfig `include`, never by the gate — so a hand-written declaration could drift
  // from its producer and the gate would not notice. Compiling them here closes that.
  const localDts = localDeclarationFiles(absDir);
  const locallyDeclared = declaredModules(localDts);

  // B1: a dep with a verified contract copy (.s2script/types/<dep>/index.d.ts) resolves to REAL
  // types via an exact `paths` entry — never the ambient `any` stub. This is what makes the
  // manifest's compiledAgainst hash a statement about types the build actually checked.
  const allDeclaredDeps = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
  ];
  // Workspace siblings (design spec 2026-07-27 §3.2/§5.3) get no ambient stub: npm already
  // symlinked the producer into node_modules, so `moduleResolution: "bundler"` finds its `types`
  // on its own — one copy of the bytes, drift impossible by construction. The ambient stub is what
  // shadows that resolution (§3.3), which is why suppressing it is the whole feature.
  // Empty for a non-workspace plugin, so every existing build is bit-identical (§11).
  const { siblings } = resolveSiblingContracts(absDir, allDeclaredDeps);

  const contractPaths: Record<string, string[]> = {};
  for (const d of allDeclaredDeps) {
    const sibling = siblings.get(d);
    if (sibling !== undefined) {
      // Decision #9: the SIBLING beats a stale `.s2script/types/<dep>/index.d.ts` left over from
      // before the migration. The copy is the drift vector this design removes; leaving it
      // authoritative here would defeat the point. `build.ts` warns that it is now ignored.
      //
      // ALWAYS map the interface to the producer's own contract — never lean on the symlink.
      //
      // §3.2 originally resolved siblings through npm's `node_modules` link, on the strength of
      // §3.1: npm links EVERY workspace package, declared as a dependency or not. That is true of
      // npm and does not generalise. pnpm and bun link only packages something declares as an NPM
      // dependency, and an s2script plugin dep lives under `s2script.pluginDependencies` — so they
      // create no link and the suppressed stub became `TS2307: Cannot find module`. yarn PnP has no
      // `node_modules` at all. Measured on pnpm 10 and bun 1.3; both failed, and both pass with
      // this entry.
      //
      // A dependency name is also an INTERFACE name (§5.3.0) while a symlink carries the PACKAGE
      // name, so the link could never reach a producer publishing under a decoupled name anyway
      // (@edge/mce publishing @community/mapchooser). One mechanism now covers both: point tsc at
      // the producer's OWN contract — the very file `build.ts` hashes into `compiledAgainst`, so
      // there is still exactly one copy of the bytes and drift stays impossible by construction.
      contractPaths[d] = [sibling.typesPath];
      continue;
    }
    const p = localContractPath(absDir, d);
    if (p !== null) contractPaths[d] = [p];
  }

  // Vendored libraries resolve to REAL types by exact `paths` entry, same as contracts.
  // A declared-but-missing one is caught earlier by assertLibrariesResolved (build.ts) —
  // it is deliberately NOT stubbed to `any` here.
  const libraryPaths = resolveLibraries(absDir, s2.libraries ?? {}).paths;
  const libraryNames = new Set(Object.keys(s2.libraries ?? {}));

  const deps = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
    // Never stub a module the plugin declares itself — a shorthand `declare module "X";` and a
    // full `declare module "X" { … }` for the same X collide.
  ].filter(
    (d) =>
      !isAlwaysResolved(d) &&
      !locallyDeclared.has(d) &&
      contractPaths[d] === undefined &&
      !siblings.has(d) &&
      !libraryNames.has(d),
  );

  const options: ts.CompilerOptions = {
    // Accept explicit `.ts` import extensions (node type-stripping requires them for source-to-source
    // imports; esbuild strips them at bundle time). Backward-compatible — extensionless imports still resolve.
    ...sharedProgramOptions(ts),
    baseUrl: packagesDir,
    paths: {
      // Builtins: the root barrel `@s2script/sdk` → packages/sdk/index.d.ts; subpaths
      // `@s2script/sdk/<cap>` → packages/sdk/<cap>.d.ts. The `@s2script/*` fallback serves
      // @s2script/cs2 → packages/cs2/index.d.ts (and would also hit sdk/index.d.ts; the exact
      // `@s2script/sdk` entry makes the barrel obvious). tsc picks the longest matching prefix,
      // so `@s2script/sdk/*` wins for capability imports.
      "@s2script/sdk": ["sdk/index.d.ts"],
      "@s2script/sdk/*": ["sdk/*.d.ts"],
      // Game-package SUBPATHS (e.g. @s2script/cs2/econ -> packages/cs2/econ.d.ts). Needed because
      // the `@s2script/*` fallback below maps to `<pkg>/index.d.ts`, which cannot express a
      // subpath. tsc picks the longest matching prefix, so this wins for cs2 subpath imports.
      "@s2script/cs2/*": ["cs2/*.d.ts"],
      "@s2script/*": ["*/index.d.ts"],
      ...contractPaths,
      ...libraryPaths,
    },
  };

  // Globals live at the consolidated path (the legacy packages/globals/ dir is deleted).
  const rootNames = [
    entry, join(packagesDir, "sdk", "globals.d.ts"), ...localDts, ...generatedDeclarationFiles(absDir),
    ...gamePackageDeclarationFiles(pkg, packagesDir),
  ];
  const tmp = mkdtempSync(join(tmpdir(), "s2tc-"));
  try {
    if (deps.length) {
      const stub = join(tmp, "ambient.d.ts");
      writeFileSync(stub, deps.map((d) => `declare module ${JSON.stringify(d)};`).join("\n") + "\n");
      rootNames.push(stub);
    }
    const program = ts.createProgram(rootNames, options);
    const diags = [
      ...program.getSyntacticDiagnostics(),
      ...program.getSemanticDiagnostics(),
      ...program.getGlobalDiagnostics(),
    ];
    const out: TypecheckDiag[] = diags.map((d) => {
      let file = "?", line = 0, col = 0;
      if (d.file && d.start !== undefined) {
        const lc = d.file.getLineAndCharacterOfPosition(d.start);
        file = d.file.fileName; line = lc.line + 1; col = lc.character + 1;
      }
      return { file, line, col, code: d.code, message: ts.flattenDiagnosticMessageText(d.messageText, "\n") };
    });
    return { ok: out.length === 0, diagnostics: out, program };
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

export function formatDiagnostics(diags: TypecheckDiag[]): string {
  return diags.map((d) => `  ${d.file}:${d.line}:${d.col} — TS${d.code}: ${d.message}`).join("\n");
}

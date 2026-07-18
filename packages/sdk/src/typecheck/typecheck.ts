import ts from "typescript";
import { existsSync, readdirSync, readFileSync, writeFileSync, rmSync, mkdtempSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { resolvePackagesDir } from "../packages-resolve.ts";

export interface TypecheckDiag { file: string; line: number; col: number; code: number; message: string; }
export interface TypecheckResult { ok: boolean; diagnostics: TypecheckDiag[]; }

/** Every `.d.ts` the plugin ships under `src/` (non-recursive: matches the scaffold's layout).
 *  These are the plugin's own ambient declarations and belong in its typecheck. */
function localDeclarationFiles(pluginDir: string): string[] {
  const srcDir = join(pluginDir, "src");
  if (!existsSync(srcDir)) return [];
  return readdirSync(srcDir)
    .filter((f) => f.endsWith(".d.ts"))
    .map((f) => join(srcDir, f));
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
 *  @s2script/sdk/* -> packagesDir/sdk/<cap>.d.ts; @s2script/cs2 -> packagesDir/cs2/index.d.ts;
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
  // FOLLOW-ON (design spec 2026-07-15 §4.6, plan 2): the `any` for a stubbed interface is a
  // placeholder. A consumer should resolve a plugin-published interface to its REAL contract via
  // `s2script add` → `.s2script/types/<iface>/index.d.ts`. Until that lands,
  // `examples/zones-consumer-demo` has weaker types than it did when packages/zones existed.
  // Tracked in the spec's §10.
  const isAlwaysResolved = (d: string): boolean =>
    d.startsWith("@s2script/sdk/") || d === "@s2script/cs2" || d.startsWith("@s2script/cs2/");

  // A plugin's OWN .d.ts files are part of its typecheck. They carry ambient declarations for
  // interfaces it consumes (see examples/*-consumer). Before this they were compiled only by the
  // editor via tsconfig `include`, never by the gate — so a hand-written declaration could drift
  // from its producer and the gate would not notice. Compiling them here closes that.
  const localDts = localDeclarationFiles(absDir);
  const locallyDeclared = declaredModules(localDts);

  const deps = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
    // Never stub a module the plugin declares itself — a shorthand `declare module "X";` and a
    // full `declare module "X" { … }` for the same X collide.
  ].filter((d) => !isAlwaysResolved(d) && !locallyDeclared.has(d));

  const options: ts.CompilerOptions = {
    strict: true,
    noEmit: true,
    // Accept explicit `.ts` import extensions (node type-stripping requires them for source-to-source
    // imports; esbuild strips them at bundle time). Backward-compatible — extensionless imports still resolve.
    allowImportingTsExtensions: true,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
    lib: ["lib.es2020.d.ts"],
    types: [],
    baseUrl: packagesDir,
    paths: {
      // Builtins are `@s2script/sdk/<cap>` → packages/sdk/<cap>.d.ts. The `@s2script/*` fallback
      // now serves only @s2script/cs2 → packages/cs2/index.d.ts (the legacy per-package builtin
      // dirs are deleted). tsc picks the longest matching prefix, so `@s2script/sdk/*` wins for
      // sdk imports.
      "@s2script/sdk/*": ["sdk/*.d.ts"],
      "@s2script/*": ["*/index.d.ts"],
    },
    skipLibCheck: true,
  };

  // Globals live at the consolidated path (the legacy packages/globals/ dir is deleted).
  const rootNames = [entry, join(packagesDir, "sdk", "globals.d.ts"), ...localDts];
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
    return { ok: out.length === 0, diagnostics: out };
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

export function formatDiagnostics(diags: TypecheckDiag[]): string {
  return diags.map((d) => `  ${d.file}:${d.line}:${d.col} — TS${d.code}: ${d.message}`).join("\n");
}

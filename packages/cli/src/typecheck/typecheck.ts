import ts from "typescript";
import { readFileSync, writeFileSync, rmSync, mkdtempSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

export interface TypecheckDiag { file: string; line: number; col: number; code: number; message: string; }
export interface TypecheckResult { ok: boolean; diagnostics: TypecheckDiag[]; }

/** Typecheck a plugin dir (full strict) against the shipped engine .d.ts under `packagesDir`.
 *  @s2script/* -> packagesDir/<name>/index.d.ts; the global `console` -> packagesDir/globals/globals.d.ts;
 *  each declared pluginDependency -> an ambient `declare module "<dep>";` (any). Never emits. */
export function typecheckPlugin(pluginDir: string, opts: { packagesDir: string }): TypecheckResult {
  const absDir = resolve(pluginDir);
  const pkg = JSON.parse(readFileSync(join(absDir, "package.json"), "utf8"));
  const s2 = pkg.s2script ?? {};
  const entryRel = s2.main ?? pkg.main;
  if (!entryRel) throw new Error(`typecheckPlugin: no entry point in ${join(absDir, "package.json")}`);
  const entry = resolve(absDir, entryRel);
  const deps = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
  ].filter((d) => !d.startsWith("@s2script/"));

  const options: ts.CompilerOptions = {
    strict: true,
    noEmit: true,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
    lib: ["lib.es2020.d.ts"],
    types: [],
    baseUrl: opts.packagesDir,
    paths: { "@s2script/*": ["*/index.d.ts"] },
    skipLibCheck: true,
  };

  const rootNames = [entry, join(opts.packagesDir, "globals", "globals.d.ts")];
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

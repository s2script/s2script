/**
 * B2 (north-star §5.3): run the pinned @s2script/eslint-plugin rules in-process, AFTER the tsc
 * gate — the same engine + rule versions the editor runs. A plugin's own eslint.config.* wins
 * (editor/build parity: what the author's editor shows is what the build enforces); otherwise
 * the canonical config runs against the typecheck gate's ALREADY-BUILT ts.Program, giving the
 * lint byte-identical module resolution to the gate with no tsconfig/node_modules dependence.
 */
import { ESLint } from "eslint";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import type ts from "typescript";
import s2lint from "@s2script/eslint-plugin";

export interface LintResult { ok: boolean; output: string; errorCount: number; }

const CONFIG_FILES = ["eslint.config.js", "eslint.config.mjs", "eslint.config.cjs", "eslint.config.ts"];

export async function lintPlugin(pluginDir: string, program: ts.Program): Promise<LintResult> {
  const absDir = resolve(pluginDir);
  const hasOwnConfig = CONFIG_FILES.some((f) => existsSync(join(absDir, f)));

  const eslint = hasOwnConfig
    ? new ESLint({ cwd: absDir, errorOnUnmatchedPattern: false })
    : new ESLint({
        cwd: absDir,
        overrideConfigFile: true,
        overrideConfig: s2lint.configs.build!([program]) as never,
        errorOnUnmatchedPattern: false,
      });

  // Canonical path: lint exactly the program's own in-dir sources (provided-program parsing
  // rejects files outside the program). Own-config path: the project's config governs.
  const dirPrefix = absDir.replace(/\\/g, "/").replace(/\/+$/, "") + "/";
  const targets = hasOwnConfig
    ? ["**/*.ts"]
    : program
        .getSourceFiles()
        .filter((sf) => !sf.isDeclarationFile && sf.fileName.replace(/\\/g, "/").startsWith(dirPrefix))
        .map((sf) => sf.fileName);

  if (targets.length === 0) return { ok: true, output: "", errorCount: 0 };

  const results = await eslint.lintFiles(targets);
  const errorCount = results.reduce((n, r) => n + r.errorCount, 0);
  const formatter = await eslint.loadFormatter("stylish");
  const output = String(await formatter.format(results));
  return { ok: errorCount === 0, output, errorCount };
}

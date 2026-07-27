/**
 * B2 (north-star §5.3): run the pinned @s2script/eslint-plugin rules in-process, AFTER the tsc
 * gate — the same engine + rule versions the editor runs. A plugin's own eslint.config.* wins
 * (editor/build parity: what the author's editor shows is what the build enforces); otherwise
 * the canonical config runs against the typecheck gate's ALREADY-BUILT ts.Program, giving the
 * lint byte-identical module resolution to the gate with no tsconfig/node_modules dependence.
 */
import { ESLint } from "eslint";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import type ts from "typescript";
import s2lint from "@s2script/eslint-plugin";
import { findWorkspaceRoot } from "../workspace/workspace.ts";

export interface LintResult { ok: boolean; output: string; errorCount: number; }

const CONFIG_FILES = ["eslint.config.js", "eslint.config.mjs", "eslint.config.cjs", "eslint.config.ts"];

/**
 * The directory whose `eslint.config.*` governs this plugin, or null for the canonical in-memory
 * config.
 *
 * The plugin directory is checked first, then — in a workspace (design spec 2026-07-27 §5.3 item
 * 3) — each directory up to and including the workspace root. A monorepo keeps ONE root
 * `eslint.config.mjs`, exactly as ESLint's own flat-config resolution does, and before this the
 * gate could not see it: it checked the plugin directory only and silently fell back to the
 * canonical config, breaking the editor/build parity guarantee that is the whole point of B2.
 *
 * Outside a workspace the search still stops at the plugin directory, so every existing plugin
 * lints against exactly what it linted against before.
 */
export function ownConfigDir(pluginDir: string): string | null {
  const stopAt = findWorkspaceRoot(pluginDir);
  let dir = pluginDir;
  for (;;) {
    if (CONFIG_FILES.some((f) => existsSync(join(dir, f)))) return dir;
    if (stopAt === null || dir === stopAt) return null;
    const parent = dirname(dir);
    if (parent === dir) return null; // defensive: a workspace root must be an ancestor, but never loop
    dir = parent;
  }
}

export async function lintPlugin(pluginDir: string, program: ts.Program): Promise<LintResult> {
  const absDir = resolve(pluginDir);
  const configDir = ownConfigDir(absDir);
  const hasOwnConfig = configDir !== null;

  // cwd is the CONFIG's directory, not the plugin's: a workspace-root flat config resolves its
  // `files`/`ignores` patterns and its plugin imports relative to itself. For the single-plugin
  // case the two are the same directory, so this is a no-op there.
  const eslint = hasOwnConfig
    ? new ESLint({ cwd: configDir, errorOnUnmatchedPattern: false })
    : new ESLint({
        cwd: absDir,
        overrideConfigFile: true,
        overrideConfig: s2lint.configs.build!([program]) as never,
        errorOnUnmatchedPattern: false,
      });

  // Canonical path: lint exactly the program's own in-dir sources (provided-program parsing
  // rejects files outside the program). Own-config path: the project's config governs, but the
  // TARGETS stay scoped to this plugin — a root config must not fan one plugin's build out over
  // its siblings. Absolute so it means the same thing whatever cwd the config sits at.
  const dirPrefix = absDir.replace(/\\/g, "/").replace(/\/+$/, "") + "/";
  const targets = hasOwnConfig
    ? [join(absDir, "**", "*.ts")]
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

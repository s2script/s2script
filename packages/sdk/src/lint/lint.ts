/**
 * B2 (north-star §5.3): run the pinned @s2script/eslint-plugin rules in-process, AFTER the tsc
 * gate — the same engine + rule versions the editor runs. A plugin's own eslint.config.* wins
 * (editor/build parity: what the author's editor shows is what the build enforces); otherwise
 * the canonical config runs against the typecheck gate's ALREADY-BUILT ts.Program, giving the
 * lint byte-identical module resolution to the gate with no tsconfig/node_modules dependence.
 */
import { ESLint } from "eslint";
import { existsSync, readdirSync } from "node:fs";
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

/**
 * Every `.ts` file under `dir`, walked by hand rather than expressed as a glob string.
 *
 * The own-config path used to hand ESLint `join(absDir, "**", "*.ts")` — the plugin's absolute
 * filesystem PATH spliced into a glob pattern. A plugin directory named e.g. `pl[ug]in` is
 * ordinary on disk but a bracket EXPRESSION to a glob matcher, so `[ug]` is read as a character
 * class instead of a literal name and the pattern matches nothing; combined with
 * `errorOnUnmatchedPattern:false`, zero files is not an error and the gate reports green while
 * checking nothing. `join` also emits backslashes on Windows, which ESLint glob patterns never
 * accept either way. Enumerating literal, already-resolved file paths sidesteps pattern parsing
 * entirely — ESLint stats an exact path before it ever tries to glob-match it.
 *
 * `node_modules` is excluded because a workspace plugin's own `node_modules` holds SYMLINKS to
 * sibling packages (§3.1) — walking into it would lint the siblings' sources under this plugin's
 * name. Dot-directories (`.s2script/`, `.git/`) are excluded to match what a bare recursive
 * `*.ts` glob already did by default (dotfiles are invisible to a glob without an explicit
 * `dot: true`), so this is not a new exclusion — it is the same file set matched before
 * workspaces existed.
 */
function collectTsFiles(dir: string): string[] {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return []; // a plugin dir that vanished mid-build matches nothing; the caller handles empty
  }
  const out: string[] = [];
  for (const e of entries) {
    if (e.name === "node_modules" || e.name.startsWith(".")) continue;
    const full = join(dir, e.name);
    if (e.isDirectory()) out.push(...collectTsFiles(full));
    else if (e.isFile() && e.name.endsWith(".ts")) out.push(full);
  }
  return out;
}

export async function lintPlugin(pluginDir: string, program: ts.Program): Promise<LintResult> {
  const absDir = resolve(pluginDir);
  const configDir = ownConfigDir(absDir);
  const hasOwnConfig = configDir !== null;

  // cwd is the CONFIG's directory, not the plugin's: a workspace-root flat config resolves its
  // `files`/`ignores` patterns and its plugin imports relative to itself. For the single-plugin
  // case the two are the same directory, so this is a no-op there.
  const eslint = hasOwnConfig
    // `warnIgnored: false` because we now hand ESLint an explicit FILE LIST rather than a glob.
    // ESLint warns once per explicitly-named file that the config's own `ignores` excludes, so a
    // plugin with, say, a generated/ directory would emit one warning per ignored file on every
    // build — noise that did not exist when the target was a glob (globs skip ignored files
    // silently). The files are still ignored either way; only the reporting changes.
    ? new ESLint({ cwd: configDir, errorOnUnmatchedPattern: false, warnIgnored: false })
    : new ESLint({
        cwd: absDir,
        overrideConfigFile: true,
        overrideConfig: s2lint.configs.build!([program]) as never,
        errorOnUnmatchedPattern: false,
      });

  // Canonical path: lint exactly the program's own in-dir sources (provided-program parsing
  // rejects files outside the program). Own-config path: the project's config governs, but the
  // TARGETS stay scoped to this plugin — a root config must not fan one plugin's build out over
  // its siblings. Enumerated as literal files (collectTsFiles), never a directory-embedded glob —
  // see that function's doc for why the glob form silently lints nothing on some directory names.
  const dirPrefix = absDir.replace(/\\/g, "/").replace(/\/+$/, "") + "/";
  const targets = hasOwnConfig
    ? collectTsFiles(absDir)
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

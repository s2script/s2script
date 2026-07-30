/**
 * Runtime `dependencies` gate.
 *
 * esbuild bundles anything in node_modules into the .s2sp, and plugins run in bare
 * V8 — no fs, no net, no process, no Buffer. An npm package touching a Node API
 * therefore builds green and fails on a live CS2 server, far from its cause. This
 * gate moves that failure to build time with a sentence explaining why.
 *
 * Two exemptions, both because the code is not a registry download:
 *   - `@s2script/*`   — builtins, external at bundle time, resolved by core
 *   - workspace source — a `file:`/`workspace:` range, or a symlink in node_modules
 *                        (what an npm workspaces install creates)
 */

import { lstatSync, realpathSync } from "node:fs";
import { dirname, join, resolve, sep } from "node:path";

export interface PkgForNpmGate {
  dependencies?: Record<string, string>;
}

/**
 * Find where `dep` actually landed in node_modules, walking ancestor directories the same way
 * Node's own resolution algorithm does. Required because npm workspaces hoists a sibling's
 * symlink to the INSTALL ROOT's node_modules, not necessarily the plugin's own directory — verified
 * against a real `npm install` (see npm-deps-gate.test.mjs): a plugin nested under a workspace
 * root (the shape every plugin/example in this repo actually uses) gets no node_modules of its
 * own at all, so checking only `pluginDir/node_modules` missed every real case.
 */
function findInNodeModules(startDir: string, dep: string): string | null {
  const segs = dep.split("/");
  let dir = resolve(startDir);
  for (;;) {
    const candidate = join(dir, "node_modules", ...segs);
    try {
      lstatSync(candidate);
      return candidate;
    } catch {
      // Not at this level — keep climbing.
    }
    const parent = dirname(dir);
    if (parent === dir) return null; // reached the filesystem root
    dir = parent;
  }
}

/**
 * A dependency is workspace-linked — local source the author wrote, not a registry download —
 * when its REALPATH escapes node_modules entirely. A symlink (what npm/pnpm workspaces leave
 * behind for a sibling) resolves to wherever the package actually lives on disk (`packages/`,
 * `plugins/`, …), which itself is never inside a node_modules directory. A genuine registry
 * install's realpath is always still inside one — npm: the package's own directory under
 * node_modules; pnpm: the content-addressed store under node_modules/.pnpm — so this is robust to
 * hoisting depth and package-manager symlink strategy, unlike checking `isSymbolicLink()` at one
 * fixed directory (which only caught the case where the PLUGIN ITSELF is the workspace root).
 */
function isWorkspaceLinked(pluginDir: string, dep: string): boolean {
  const found = findInNodeModules(pluginDir, dep);
  if (found === null) return false;
  let real: string;
  try {
    real = realpathSync(found);
  } catch {
    return false;
  }
  return !real.split(sep).includes("node_modules");
}

export function assertNoNpmRuntimeDeps(pkg: PkgForNpmGate, pluginDir: string): void {
  const offenders: string[] = [];
  for (const [dep, range] of Object.entries(pkg.dependencies ?? {})) {
    if (dep.startsWith("@s2script/")) continue;
    if (typeof range === "string" && (range.startsWith("file:") || range.startsWith("workspace:"))) continue;
    if (isWorkspaceLinked(pluginDir, dep)) continue;
    offenders.push(dep);
  }
  if (!offenders.length) return;
  throw new Error(
    `runtime dependenc${offenders.length === 1 ? "y" : "ies"} ` +
      offenders.map((d) => JSON.stringify(d)).join(", ") +
      ` ${offenders.length === 1 ? "is" : "are"} not s2script librar${offenders.length === 1 ? "y" : "ies"}.\n` +
      `  Plugins run in bare V8 — no Node APIs, no npm runtime. A package that touches\n` +
      `  fs/net/process builds green and fails on a live server.\n` +
      `  Use s2script.libraries (\`s2s add <pkg>\`), or vendor it as a workspace package.`,
  );
}

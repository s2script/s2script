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

import { lstatSync } from "node:fs";
import { join } from "node:path";

export interface PkgForNpmGate {
  dependencies?: Record<string, string>;
}

function isWorkspaceLinked(pluginDir: string, dep: string): boolean {
  try {
    return lstatSync(join(pluginDir, "node_modules", ...dep.split("/"))).isSymbolicLink();
  } catch {
    return false;
  }
}

export function assertNoNpmRuntimeDeps(pkg: PkgForNpmGate, pluginDir: string): void {
  const offenders: string[] = [];
  for (const [dep, range] of Object.entries(pkg.dependencies ?? {})) {
    if (dep === "@s2script/sdk" || dep.startsWith("@s2script/")) continue;
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

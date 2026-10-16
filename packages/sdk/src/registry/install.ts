/**
 * Registry install-plan helpers shared by the server commands (server/sync.ts, server/commands.ts):
 * spec parsing, merging several roots' plans into one install set, and the filename guard that
 * keeps a registry response from writing outside plugins/.
 */

import { basename } from "node:path";
import type { InstallPlan, PlanEntry } from "./client.ts";

/** Split "name@range" (scoped-safe: only the LAST @ separates). Bare name -> "*". */
export function parseSpec(spec: string): { name: string; range: string } {
  const at = spec.lastIndexOf("@");
  if (at > 0) return { name: spec.slice(0, at), range: spec.slice(at + 1) || "*" };
  return { name: spec, range: "*" };
}

/** Only bare filenames matching this shape are trusted — mirrors the sanitization in
 *  config/config-path.ts's configFileName ([A-Za-z0-9._-]). No directory separators, no ".". */
const SAFE_FILENAME_RE = /^[A-Za-z0-9._-]+$/;

/** entry.filename comes straight from the (possibly compromised) registry response — never trust
 *  it as a path. Derive a plain basename and refuse anything that isn't already exactly that. */
export function safeFilename(e: { name: string; version: string; filename: string }): string {
  const raw = e.filename;
  const base = basename(raw);
  if (
    !base ||
    base === "." ||
    base === ".." ||
    base !== raw ||
    base.includes("/") ||
    base.includes("\\") ||
    base.includes("..") ||
    !SAFE_FILENAME_RE.test(base)
  ) {
    throw new Error(`refusing ${e.name}@${e.version}: unsafe filename "${raw}"`);
  }
  return base;
}

export async function resolveMerged(
  client: { plan(name: string, range?: string, withMembers?: string[]): Promise<InstallPlan> },
  specs: Record<string, string>,
  /** Bundle root name -> optional members to add (forwarded as the plan's `with`). */
  withMembers: Record<string, string[]> = {}
): Promise<{
  install: PlanEntry[];
  warnings: string[];
  errors: string[];
  /** plugin name → the requested roots whose plan includes it. */
  pulledBy: Record<string, string[]>;
}> {
  const chosen = new Map<string, PlanEntry>();
  const pulledBy: Record<string, string[]> = {};
  const warnings: string[] = [];
  const errors: string[] = [];

  for (const [name, range] of Object.entries(specs)) {
    const plan = await client.plan(name, range, withMembers[name]);
    if (plan.errors.length) {
      errors.push(...plan.errors);
      continue;
    }
    warnings.push(...plan.warnings);
    for (const entry of plan.install) {
      (pulledBy[entry.name] ??= []).push(name);
      const prior = chosen.get(entry.name);
      if (prior && prior.version !== entry.version) {
        errors.push(
          `version conflict for ${entry.name}: ${prior.version} vs ${entry.version} across requested plugins`
        );
        continue;
      }
      chosen.set(entry.name, entry);
    }
  }

  return {
    install: [...chosen.values()].sort((a, b) => a.name.localeCompare(b.name)),
    warnings: [...new Set(warnings)],
    errors: [...new Set(errors)],
    pulledBy,
  };
}

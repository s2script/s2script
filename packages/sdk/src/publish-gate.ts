/**
 * publishes ⇒ types gate (design spec 2026-07-15 §4.6): if s2script.publishes is
 * set, package.json must point "types"/"typings" at an existing non-empty .d.ts.
 */

import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

export interface PluginPkgForGate {
  types?: string;
  typings?: string;
  s2script?: {
    publishes?: Record<string, unknown> | string | null;
  };
}

export interface PublishGateOk {
  ok: true;
  typesPath: string | null; // absolute path, or null when no publishes
}

export interface PublishGateErr {
  ok: false;
  error: string;
}

export type PublishGateResult = PublishGateOk | PublishGateErr;

export function hasPublishes(publishes: unknown): boolean {
  if (publishes == null) return false;
  if (typeof publishes === "string") return publishes.trim().length > 0;
  if (typeof publishes === "object") return Object.keys(publishes as object).length > 0;
  return false;
}

/** Validate publishes ⇒ types. `pluginDir` is the package root. */
export function assertPublishesTypes(
  pkg: PluginPkgForGate,
  pluginDir: string
): PublishGateResult {
  const publishes = pkg.s2script?.publishes;
  if (!hasPublishes(publishes)) {
    return { ok: true, typesPath: null };
  }

  const typesRel = pkg.types ?? pkg.typings;
  if (!typesRel || typeof typesRel !== "string") {
    return {
      ok: false,
      error: 'publishes is set but "types" is missing — add api.d.ts and set "types": "api.d.ts"',
    };
  }
  if (!typesRel.endsWith(".d.ts")) {
    return {
      ok: false,
      error: `published API must be a .d.ts file (got ${JSON.stringify(typesRel)})`,
    };
  }

  const typesPath = resolve(pluginDir, typesRel);
  if (!existsSync(typesPath)) {
    return { ok: false, error: `types file not found: ${typesRel}` };
  }
  const st = statSync(typesPath);
  if (!st.isFile() || st.size === 0) {
    return { ok: false, error: `types file is empty or not a file: ${typesRel}` };
  }

  const body = readFileSync(typesPath, "utf8").trim();
  if (!body) {
    return { ok: false, error: `types file is empty: ${typesRel}` };
  }

  return { ok: true, typesPath };
}

/** Read package.json from a plugin dir and run the gate. */
export function assertPublishesTypesInDir(pluginDir: string): PublishGateResult {
  const pkgPath = join(pluginDir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8")) as PluginPkgForGate;
  return assertPublishesTypes(pkg, pluginDir);
}

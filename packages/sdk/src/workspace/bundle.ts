/**
 * Bundles (design: s2script-website docs/superpowers/specs/2026-10-07-bundle-packages-design.md).
 *
 * A bundle is a registry package with no artifact of its own: a versioned set of one publisher's
 * plugins, listed and installed as one. The workspace ROOT is the bundle — a root package.json
 * with `s2script.kind: "bundle"` is published by `s2s deploy` after its member plugins, pinning
 * each member to the exact version just built. The members are the workspace's own non-private
 * plugins; `s2script.bundle.optional` names the ones a default install leaves out.
 *
 * The runtime never sees any of this. The membership claim rides in each member's REGISTRY
 * deploy manifest, never in the `.s2sp` manifest the loader reads.
 */

import type { Workspace, WorkspacePlugin } from "./workspace.ts";

/** A bundle version's member pins, exactly as the registry's deploy manifest carries them. */
export type BundleMembers = Record<string, { version: string; optional?: true }>;

export interface Bundle {
  name: string;
  version: string;
  /** Non-private workspace plugins, in workspace order. */
  members: WorkspacePlugin[];
  /** Member names a default `s2s install` leaves out (`s2script.bundle.optional`). */
  optional: Set<string>;
}

/** The registry's own cap — refusing locally saves a build-everything-then-fail round trip. */
export const MAX_BUNDLE_MEMBERS = 64;

/** True when the workspace root declares itself a bundle. */
export function isBundleWorkspace(ws: Workspace): boolean {
  return ws.pkg.s2script?.kind === "bundle";
}

/** `@scope` of a package name, or null when unscoped. */
function scopeOf(name: string): string | null {
  if (!name.startsWith("@")) return null;
  const slash = name.indexOf("/");
  return slash > 1 ? name.slice(1, slash) : null;
}

/**
 * Every reason the bundle root in `ws` cannot be published, as report-ready text. Empty for a
 * coherent bundle — and for a workspace that is not a bundle at all.
 */
export function bundleProblems(ws: Workspace): string[] {
  if (!isBundleWorkspace(ws)) return [];
  const pkg = ws.pkg;
  const problems: string[] = [];
  const where = "package.json (workspace root)";

  if (typeof pkg.name !== "string" || pkg.name === "") {
    problems.push(`${where}: a bundle needs a "name" — it is the bundle's registry name`);
  }
  if (typeof pkg.version !== "string" || pkg.version === "") {
    problems.push(`${where}: a bundle needs a "version"`);
  }
  if (pkg.private === true) {
    problems.push(
      `${where}: s2script.kind is "bundle" but the root is "private": true — a private root is ` +
        `never published; remove "private" or drop the bundle kind`,
    );
  }
  if (pkg.main !== undefined || pkg.s2script?.main !== undefined) {
    problems.push(`${where}: a bundle has no code of its own — remove "main"`);
  }

  const members = ws.plugins.filter((p) => !p.private);
  if (members.length === 0) {
    problems.push(`${where}: a bundle needs at least one non-private plugin in s2script.workspace.plugins`);
  }
  if (members.length > MAX_BUNDLE_MEMBERS) {
    problems.push(`${where}: a bundle may have at most ${MAX_BUNDLE_MEMBERS} members (found ${members.length})`);
  }

  const bundleScope = typeof pkg.name === "string" ? scopeOf(pkg.name) : null;
  for (const m of members) {
    if (scopeOf(m.name) !== bundleScope) {
      problems.push(
        `${m.relDir}: ${m.name} cannot be a member of ${String(pkg.name)} — a bundle and its members ` +
          `must share a scope (the registry only accepts members with the same publisher)`,
      );
    }
  }

  const raw = (pkg.s2script as { bundle?: unknown } | undefined)?.bundle;
  let optional: unknown[] = [];
  if (raw !== undefined) {
    const block = raw as { optional?: unknown } | null;
    if (!block || typeof block !== "object" || Array.isArray(block)) {
      problems.push(`${where}: s2script.bundle must be an object`);
    } else if (block.optional !== undefined) {
      if (!Array.isArray(block.optional) || block.optional.some((o) => typeof o !== "string")) {
        problems.push(`${where}: s2script.bundle.optional must be an array of member names`);
      } else {
        optional = block.optional;
      }
    }
  }
  const memberNames = new Set(members.map((m) => m.name));
  for (const name of optional) {
    if (!memberNames.has(name as string)) {
      problems.push(
        `${where}: s2script.bundle.optional names ${JSON.stringify(name)}, which is not a non-private ` +
          `plugin of this workspace`,
      );
    }
  }
  if (members.length > 0 && members.every((m) => optional.includes(m.name))) {
    problems.push(`${where}: every member is optional — a bundle needs at least one required member`);
  }
  return problems;
}

/** The bundle `ws` declares, or null when it is not a bundle workspace. Call after preflight. */
export function readBundle(ws: Workspace): Bundle | null {
  if (!isBundleWorkspace(ws)) return null;
  const optional = (ws.pkg.s2script as { bundle?: { optional?: string[] } }).bundle?.optional ?? [];
  return {
    name: ws.pkg.name as string,
    version: ws.pkg.version as string,
    members: ws.plugins.filter((p) => !p.private),
    optional: new Set(optional),
  };
}

/** The member pins for `bundle`, from each member's own (local) package.json version. */
export function bundleMembers(bundle: Bundle): BundleMembers {
  const out: BundleMembers = {};
  for (const m of bundle.members) {
    out[m.name] = bundle.optional.has(m.name) ? { version: m.version, optional: true } : { version: m.version };
  }
  return out;
}

/** The deploy manifest the registry expects for a bundle version: manifest.json alone, no artifact. */
export function bundleManifest(bundle: Bundle): Record<string, unknown> {
  return { id: bundle.name, version: bundle.version, kind: "bundle", members: bundleMembers(bundle) };
}

/** Whether two member pin sets are the same release (order-insensitive). */
export function sameMembers(a: BundleMembers, b: BundleMembers): boolean {
  const ka = Object.keys(a).sort();
  const kb = Object.keys(b).sort();
  if (ka.length !== kb.length || ka.some((k, i) => k !== kb[i])) return false;
  return ka.every((k) => a[k].version === b[k].version && !!a[k].optional === !!b[k].optional);
}

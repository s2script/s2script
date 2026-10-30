/**
 * Bundles (design: s2script-website docs/superpowers/specs/2026-10-07-bundle-packages-design.md).
 *
 * A bundle is a registry package with no artifact of its own: a versioned set of one publisher's
 * plugins, listed and installed as one. `s2s deploy` publishes it after its member plugins,
 * pinning each member to the exact version just built; `s2s version` bumps it whenever a member is
 * released.
 *
 * A bundle is declared in one of two ways, never both in one workspace:
 *
 *   - ROOT bundle: the workspace root's package.json carries `s2script.kind: "bundle"`. Its
 *     members are every non-private plugin of the workspace.
 *   - BUNDLE DIRECTORIES: the root lists `s2script.workspace.bundles` (dirs or globs). Each one
 *     holds a package.json with `s2script.kind: "bundle"` — a descriptor carrying the bundle's
 *     name, version and `s2script.bundle` block — and its members are the non-private plugins
 *     under that directory. This is how several plugin monorepos (each its own bundle) live in one
 *     parent workspace with one `.changeset/` and one release flow.
 *
 * `s2script.bundle.optional` names the members a default install leaves out.
 *
 * The runtime never sees any of this. The membership claim rides in each member's REGISTRY
 * deploy manifest, never in the `.s2sp` manifest the loader reads.
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { expandDirGlob } from "./workspace.ts";
import type { Workspace, WorkspacePackageJson, WorkspacePlugin } from "./workspace.ts";

/** A bundle version's member pins, exactly as the registry's deploy manifest carries them. */
export type BundleMembers = Record<string, { version: string; optional?: true }>;

export interface Bundle {
  name: string;
  version: string;
  /** Absolute directory of the package.json that declares the bundle (the root, for a root bundle). */
  dir: string;
  /** `dir` relative to the workspace root, `/`-separated; `""` for a root bundle. */
  relDir: string;
  /** The declaring package.json — `publishConfig.access` and the `s2script.bundle` block live here. */
  pkg: WorkspacePackageJson;
  /** Non-private member plugins, in workspace order. */
  members: WorkspacePlugin[];
  /** Member names a default `s2s install` leaves out (`s2script.bundle.optional`). */
  optional: Set<string>;
}

/** The registry's own cap — refusing locally saves a build-everything-then-fail round trip. */
export const MAX_BUNDLE_MEMBERS = 64;

/** True when the workspace root itself declares itself a bundle. */
export function isBundleWorkspace(ws: Workspace): boolean {
  return ws.pkg.s2script?.kind === "bundle";
}

/** `@scope` of a package name, or null when unscoped. */
function scopeOf(name: string): string | null {
  if (!name.startsWith("@")) return null;
  const slash = name.indexOf("/");
  return slash > 1 ? name.slice(1, slash) : null;
}

/** The raw `s2script.workspace.bundles` field, or undefined when absent. */
function bundlesField(ws: Workspace): unknown {
  return (ws.pkg.s2script?.workspace as { bundles?: unknown } | undefined)?.bundles;
}

/** One declared bundle directory, as read off disk. `pkg` is null when it could not be read. */
interface BundleSource {
  relDir: string;
  dir: string;
  pkg: WorkspacePackageJson | null;
  /** Why `pkg` is null, report-ready. */
  error?: string;
}

/**
 * The bundle directories `s2script.workspace.bundles` names, expanded and read, plus the problems
 * with the field itself. A glob that matches nothing, or a directory with no package.json, is a
 * problem: a bundle that silently vanishes is a release that silently stops shipping.
 */
function bundleSources(ws: Workspace): { sources: BundleSource[]; problems: string[] } {
  const raw = bundlesField(ws);
  const where = "package.json (workspace root)";
  if (raw === undefined) return { sources: [], problems: [] };
  if (!Array.isArray(raw) || raw.some((g) => typeof g !== "string" || g === "")) {
    return { sources: [], problems: [`${where}: s2script.workspace.bundles must be an array of directories or globs`] };
  }
  const problems: string[] = [];
  const seen = new Set<string>();
  const sources: BundleSource[] = [];
  for (const glob of raw as string[]) {
    const dirs = expandDirGlob(ws.root, glob).filter((rel) => existsSync(join(ws.root, rel, "package.json")));
    if (dirs.length === 0) {
      problems.push(`${where}: s2script.workspace.bundles entry ${JSON.stringify(glob)} matches no directory with a package.json`);
      continue;
    }
    for (const rel of dirs) {
      if (seen.has(rel)) continue;
      seen.add(rel);
      const dir = join(ws.root, rel);
      try {
        sources.push({ relDir: rel, dir, pkg: JSON.parse(readFileSync(join(dir, "package.json"), "utf8")) });
      } catch (e) {
        sources.push({ relDir: rel, dir, pkg: null, error: `cannot read ${rel}/package.json: ${(e as Error).message}` });
      }
    }
  }
  return { sources: sources.sort((a, b) => (a.relDir < b.relDir ? -1 : 1)), problems };
}

/** True when `relDir` (a plugin's) lies inside the bundle directory `bundleRel`. */
function isUnder(relDir: string, bundleRel: string): boolean {
  return bundleRel === "" || relDir.startsWith(`${bundleRel}/`);
}

/** The non-private plugins a bundle declared at `bundleRel` groups. */
function membersOf(ws: Workspace, bundleRel: string): WorkspacePlugin[] {
  return ws.plugins.filter((p) => !p.private && isUnder(p.relDir, bundleRel));
}

/** The `s2script.bundle.optional` names of `pkg`, or [] when absent or malformed. */
function optionalOf(pkg: WorkspacePackageJson): string[] {
  const optional = (pkg.s2script as { bundle?: { optional?: unknown } } | undefined)?.bundle?.optional;
  return Array.isArray(optional) ? optional.filter((o): o is string => typeof o === "string") : [];
}

/** Every reason one bundle declaration cannot be published. `where` names its package.json. */
function declarationProblems(where: string, pkg: WorkspacePackageJson, members: WorkspacePlugin[]): string[] {
  const problems: string[] = [];

  if (typeof pkg.name !== "string" || pkg.name === "") {
    problems.push(`${where}: a bundle needs a "name" — it is the bundle's registry name`);
  }
  if (typeof pkg.version !== "string" || pkg.version === "") {
    problems.push(`${where}: a bundle needs a "version"`);
  }
  if (pkg.private === true) {
    problems.push(
      `${where}: s2script.kind is "bundle" but the package is "private": true — a private bundle is ` +
        `never published; remove "private" or drop the bundle kind`,
    );
  }
  if (pkg.main !== undefined || pkg.s2script?.main !== undefined) {
    problems.push(`${where}: a bundle has no code of its own — remove "main"`);
  }

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
          `plugin of this bundle`,
      );
    }
  }
  if (members.length > 0 && members.every((m) => optional.includes(m.name))) {
    problems.push(`${where}: every member is optional — a bundle needs at least one required member`);
  }
  return problems;
}

/**
 * Every reason the bundles `ws` declares cannot be published, as report-ready text. Empty for
 * coherent bundles — and for a workspace that declares none at all.
 */
export function bundleProblems(ws: Workspace): string[] {
  const rootBundle = isBundleWorkspace(ws);
  const { sources, problems } = bundleSources(ws);

  if (rootBundle && bundlesField(ws) !== undefined) {
    problems.push(
      `package.json (workspace root): s2script.kind is "bundle" and s2script.workspace.bundles is set — ` +
        `a root bundle cannot also contain bundles; drop the root's kind, or drop the bundles list`,
    );
    return problems;
  }
  if (rootBundle) return [...problems, ...declarationProblems("package.json (workspace root)", ws.pkg, membersOf(ws, ""))];

  const memberDirs = new Set([...ws.plugins, ...ws.libs].map((m) => m.relDir));
  const byName = new Map<string, string>();
  for (const src of sources) {
    const where = `${src.relDir}/package.json`;
    if (src.pkg === null) {
      problems.push(src.error as string);
      continue;
    }
    if (src.pkg.s2script?.kind !== "bundle") {
      problems.push(`${where}: is listed in s2script.workspace.bundles but has no s2script.kind "bundle"`);
      continue;
    }
    if (src.pkg.s2script.workspace !== undefined) {
      problems.push(
        `${where}: a bundle directory must not carry s2script.workspace — s2s would treat it as its own ` +
          `workspace root when run inside it; remove the block (the parent root is the workspace)`,
      );
    }
    if (memberDirs.has(src.relDir)) {
      problems.push(
        `${src.relDir}: is both a workspace package and a bundle directory — narrow the root's ` +
          `"workspaces" globs so the bundle descriptor is not a package`,
      );
    }
    const nested = sources.find((o) => o !== src && isUnder(src.relDir, o.relDir));
    if (nested) {
      problems.push(`${src.relDir}: is inside bundle directory ${nested.relDir} — bundle directories cannot nest`);
    }
    if (typeof src.pkg.name === "string" && src.pkg.name !== "") {
      const first = byName.get(src.pkg.name);
      if (first !== undefined) {
        problems.push(`${src.relDir}: duplicate bundle name ${JSON.stringify(src.pkg.name)} — already declared by ${first}`);
      } else {
        byName.set(src.pkg.name, src.relDir);
      }
    }
    problems.push(...declarationProblems(where, src.pkg, membersOf(ws, src.relDir)));
  }
  return problems;
}

function toBundle(ws: Workspace, relDir: string, dir: string, pkg: WorkspacePackageJson): Bundle {
  return {
    name: pkg.name as string,
    version: pkg.version as string,
    dir,
    relDir,
    pkg,
    members: membersOf(ws, relDir),
    optional: new Set(optionalOf(pkg)),
  };
}

/** The root bundle `ws` declares, or null when the root is not a bundle. Call after preflight. */
export function readBundle(ws: Workspace): Bundle | null {
  return isBundleWorkspace(ws) ? toBundle(ws, "", ws.root, ws.pkg) : null;
}

/**
 * Every bundle `ws` declares, root or directories, in directory order. Empty when it declares
 * none. Call after preflight: a declaration preflight would refuse is skipped here, not reported.
 */
export function readBundles(ws: Workspace): Bundle[] {
  const root = readBundle(ws);
  if (root) return [root];
  return bundleSources(ws)
    .sources.filter((s) => s.pkg?.s2script?.kind === "bundle")
    .map((s) => toBundle(ws, s.relDir, s.dir, s.pkg as WorkspacePackageJson));
}

/** Plugin name -> the name of the bundle it is a member of, across every bundle in `bundles`. */
export function bundleOfPlugin(bundles: readonly Bundle[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const b of bundles) for (const m of b.members) out.set(m.name, b.name);
  return out;
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

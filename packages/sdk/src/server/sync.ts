/**
 * Make a server's `plugins/` match a resolved set of registry plugins, and produce the lock that
 * describes it. Shared by `s2s install`, `s2s update` and `s2s server init`; the runtime's boot
 * updater (core/src/server_update.rs) follows the same steps:
 *
 *   1. Stage: for each target whose file is missing or differs, download it, check its sha256,
 *      and write it beside the destination as `<file>.s2s-tmp` (the loader only scans `*.s2sp`).
 *   2. Commit: only once every download succeeded, rename each staged file into place.
 *   3. Prune: remove files the OLD lock owned that are no longer targets — but only when the file
 *      still has the bytes the lock recorded; one an operator changed is left, with a warning.
 *
 * A failure in step 1 leaves plugins/ exactly as it was. Files the lock never owned are never
 * replaced unless a target writes the same filename, and never removed.
 */

import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve, sep } from "node:path";
import type { PlanEntry } from "../registry/client.ts";
import { resolveMerged, safeFilename } from "../registry/install.ts";
import {
  integrityFromSha256Hex,
  sha256Hex,
  sha256HexFromIntegrity,
  type LockedPlugin,
  type ServerLock,
} from "./lock.ts";
import { PLUGINS_SUBDIR, type ServerManifest } from "./manifest.ts";

export interface Target {
  name: string;
  version: string;
  /** Bare filename inside plugins/. */
  filename: string;
  url: string;
  sha256: string;
  reviewState?: string;
}

export interface Downloader {
  downloadS2sp(name: string, version: string): Promise<Buffer>;
}

export interface Planner {
  plan(name: string, range?: string, withMembers?: string[]): Promise<{
    install: PlanEntry[];
    warnings: string[];
    errors: string[];
    root: { name: string; version: string } | null;
    skipped: { name: string; reason: string }[];
  }>;
}

export function lockKey(filename: string): string {
  return `${PLUGINS_SUBDIR}/${filename}`;
}

/** Resolve `specs` (name → range) through the registry's install plan into checked targets. */
export async function resolveTargets(
  client: Planner,
  specs: Record<string, string>,
  withMembers: Record<string, string[]>,
  opts: { reviewedOnly?: boolean } = {},
): Promise<{ targets: Target[]; warnings: string[] }> {
  const { install, warnings, errors } = await resolveMerged(client, specs, withMembers);
  if (errors.length) throw new Error(`cannot resolve plugins:\n  - ${errors.join("\n  - ")}`);
  if (opts.reviewedOnly) {
    const unreviewed = install.filter((e) => e.reviewState !== "reviewed");
    if (unreviewed.length) {
      throw new Error(
        `reviewedOnly: refusing unreviewed plugins: ${unreviewed.map((e) => `${e.name}@${e.version}`).join(", ")}`,
      );
    }
  }
  const targets = install.map((e) => {
    if (!e.sha256) {
      throw new Error(`refusing ${e.name}@${e.version}: registry did not provide a sha256 to verify against`);
    }
    return {
      name: e.name,
      version: e.version,
      filename: safeFilename(e),
      url: e.url,
      sha256: e.sha256,
      reviewState: e.reviewState,
    };
  });
  return { targets, warnings };
}

/** The lock's plugins as targets — `s2s install` with an up-to-date lock installs exactly these. */
export function targetsFromLock(lock: ServerLock): Target[] {
  return Object.entries(lock.plugins).map(([key, p]) => {
    const sha = sha256HexFromIntegrity(p.integrity);
    if (!sha) throw new Error(`plugins-lock.json: ${key} has an unsupported integrity ${p.integrity}`);
    const filename = key.slice(PLUGINS_SUBDIR.length + 1);
    return { name: p.name, version: p.version, filename: safeFilename({ ...p, filename }), url: p.resolved, sha256: sha };
  });
}

/**
 * The range to resolve each root at so that `s2s install` (or `s2s update <some>`) keeps the
 * others where the lock has them: the locked version when it still satisfies, else the range.
 */
export function pinnedSpecs(
  manifest: ServerManifest,
  lock: ServerLock | null,
  satisfies: (version: string, range: string) => boolean,
  unpinned: ReadonlySet<string> = new Set(),
): Record<string, string> {
  const locked = new Map<string, string>();
  for (const p of Object.values(lock?.plugins ?? {})) locked.set(p.name, p.version);
  const out: Record<string, string> = {};
  for (const [name, range] of Object.entries(manifest.dependencies)) {
    const v = locked.get(name);
    out[name] = !unpinned.has(name) && v !== undefined && satisfies(v, range) ? v : range;
  }
  return out;
}

export interface SyncResult {
  plugins: Record<string, LockedPlugin>;
  installed: string[];
  kept: string[];
  removed: string[];
  /** Files the old lock owned and no target wants, left because they were changed on disk. */
  leftAlone: string[];
}

export async function syncPlugins(opts: {
  root: string;
  targets: Target[];
  oldLock: ServerLock | null;
  client: Downloader;
  dryRun?: boolean;
  log?: (m: string) => void;
}): Promise<SyncResult> {
  const log = opts.log ?? (() => {});
  const dir = resolve(opts.root, PLUGINS_SUBDIR);
  const result: SyncResult = { plugins: {}, installed: [], kept: [], removed: [], leftAlone: [] };

  const seen = new Set<string>();
  for (const t of opts.targets) {
    if (seen.has(t.filename)) throw new Error(`two plugins resolve to the same file ${t.filename}`);
    seen.add(t.filename);
    const dest = resolve(dir, t.filename);
    if (!dest.startsWith(dir + sep)) throw new Error(`refusing ${t.name}: ${t.filename} escapes ${dir}`);
  }

  // 1. Stage.
  const staged: { tmp: string; dest: string; t: Target }[] = [];
  try {
    for (const t of opts.targets) {
      const dest = join(dir, t.filename);
      result.plugins[lockKey(t.filename)] = {
        name: t.name,
        version: t.version,
        resolved: t.url,
        integrity: integrityFromSha256Hex(t.sha256),
      };
      if (existsSync(dest) && sha256Hex(readFileSync(dest)) === t.sha256) {
        result.kept.push(t.filename);
        continue;
      }
      result.installed.push(t.filename);
      if (opts.dryRun) {
        log(`would install ${t.name}@${t.version} -> ${t.filename}`);
        continue;
      }
      const bytes = await opts.client.downloadS2sp(t.name, t.version);
      const got = sha256Hex(bytes);
      if (got !== t.sha256) {
        throw new Error(`sha256 mismatch for ${t.name}@${t.version}: expected ${t.sha256}, got ${got}`);
      }
      mkdirSync(dir, { recursive: true });
      const tmp = `${dest}.s2s-tmp`;
      writeFileSync(tmp, bytes);
      staged.push({ tmp, dest, t });
    }
  } catch (e) {
    for (const s of staged) rmSync(s.tmp, { force: true });
    throw e;
  }

  // 2. Commit.
  for (const s of staged) {
    renameSync(s.tmp, s.dest);
    log(`installed ${s.t.name}@${s.t.version} -> ${s.t.filename}`);
  }

  // 3. Prune what the old lock owned and nothing wants now.
  for (const [key, old] of Object.entries(opts.oldLock?.plugins ?? {})) {
    if (key in result.plugins) continue;
    const file = join(opts.root, key);
    if (!existsSync(file)) continue;
    const want = sha256HexFromIntegrity(old.integrity);
    if (want && sha256Hex(readFileSync(file)) === want) {
      if (opts.dryRun) log(`would remove ${old.name}@${old.version} (${key})`);
      else {
        rmSync(file);
        log(`removed ${old.name}@${old.version} (${key})`);
      }
      result.removed.push(key);
    } else {
      log(`warning: left ${key} in place — it no longer matches the lock, so it was changed by hand`);
      result.leftAlone.push(key);
    }
  }
  return result;
}

export function lockFor(manifest: ServerManifest, plugins: Record<string, LockedPlugin>): ServerLock {
  return {
    name: manifest.name,
    root: {
      name: manifest.name,
      dependencies: { ...manifest.dependencies },
      ...(Object.keys(manifest.with).length ? { with: manifest.with } : {}),
    },
    plugins,
  };
}

/** True when the lock was resolved from exactly this manifest's dependencies and `with`. */
export function lockMatchesManifest(lock: ServerLock, manifest: ServerManifest): boolean {
  const same = (a: Record<string, unknown>, b: Record<string, unknown>) =>
    JSON.stringify(Object.entries(a).sort()) === JSON.stringify(Object.entries(b).sort());
  return same(lock.root.dependencies, manifest.dependencies) && same(lock.root.with ?? {}, manifest.with);
}

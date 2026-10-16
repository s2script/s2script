/**
 * The server-manifest workflows behind `s2s server init`, `s2s install` and `s2s update`, kept
 * free of argv parsing and process.exit so they are testable with an injected registry client.
 */

import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { unzipSync } from "fflate";
import { satisfies, valid } from "semver";
import { RegistryError, type PlanEntry } from "../registry/client.ts";
import { parseSpec } from "../registry/install.ts";
import { readLock, writeLock, integrityFromSha256Hex, sha256Hex, type LockedPlugin } from "./lock.ts";
import {
  PLUGINS_SUBDIR,
  SERVER_LOCK,
  SERVER_MANIFEST,
  newServerManifestJson,
  parseServerManifest,
  readServerManifest,
  writeServerManifest,
  type ServerManifest,
} from "./manifest.ts";
import {
  lockFor,
  lockKey,
  lockMatchesManifest,
  pinnedSpecs,
  resolveTargets,
  syncPlugins,
  targetsFromLock,
  type Downloader,
  type Planner,
} from "./sync.ts";

export type ServerClient = Planner & Downloader;

const sat = (v: string, r: string) => satisfies(v, r, { includePrerelease: true });

function summary(r: { installed: string[]; kept: string[]; removed: string[] }): string {
  return `${r.installed.length} installed, ${r.kept.length} up to date, ${r.removed.length} removed`;
}

/** The caret range npm writes for a newly added dependency. */
function caret(version: string): string {
  return valid(version) ? `^${version}` : version;
}

/**
 * `s2s install` in a server: with names, add them to dependencies (a bare name gets ^latest, as
 * npm does); without, install the lock exactly when it still matches plugins.json, else resolve,
 * keeping every plugin the lock already has wherever its range still allows.
 */
export async function serverInstall(opts: {
  root: string;
  client: ServerClient;
  specs?: string[];
  /** Optional bundle members to install with the ONE bundle named in `specs` (`--with`). */
  with?: string[];
  dryRun?: boolean;
  log?: (m: string) => void;
}): Promise<string> {
  const log = opts.log ?? (() => {});
  let manifest = readServerManifest(opts.root);
  if (opts.with && opts.with.length > 0) {
    // --with names optional members of one bundle; with several names it would be a guess
    // which bundle each member belongs to.
    if (opts.specs?.length !== 1) {
      throw new Error(
        `--with applies to exactly one bundle named on the command line (got ${opts.specs?.length ?? 0}) — ` +
          `for several bundles, edit "with" in ${SERVER_MANIFEST}`,
      );
    }
    manifest = { ...manifest, with: { ...manifest.with, [parseSpec(opts.specs[0]!).name]: opts.with } };
  }
  const lockPath = join(opts.root, SERVER_LOCK);
  const lock = readLock(lockPath);

  const withChanged = !!opts.with?.length;
  const added = new Set<string>();
  if (opts.specs && opts.specs.length > 0) {
    const deps = { ...manifest.dependencies };
    for (const spec of opts.specs) {
      const explicit = spec.lastIndexOf("@") > 0;
      const { name, range } = parseSpec(spec);
      deps[name] = explicit ? range : "*";
      added.add(name);
    }
    manifest = { ...manifest, dependencies: deps };
  }

  let targets;
  if (added.size === 0 && !withChanged && lock && lockMatchesManifest(lock, manifest)) {
    targets = targetsFromLock(lock);
  } else {
    const specs = pinnedSpecs(manifest, lock, sat, added);
    const resolved = await resolveTargets(opts.client, specs, manifest.with, { reviewedOnly: manifest.reviewedOnly });
    for (const w of resolved.warnings) log(`warning: ${w}`);
    targets = resolved.targets;
    // A bare `s2s install name` records ^<resolved version>, like npm.
    if (added.size > 0) {
      const deps = { ...manifest.dependencies };
      for (const name of added) {
        if (deps[name] !== "*") continue;
        const t = targets.find((x) => x.name === name);
        if (t) deps[name] = caret(t.version);
      }
      manifest = { ...manifest, dependencies: deps };
    }
  }

  const res = await syncPlugins({ root: opts.root, targets, oldLock: lock, client: opts.client, dryRun: opts.dryRun, log });
  if (!opts.dryRun) {
    if (added.size > 0 || withChanged) writeServerManifest(manifest);
    writeLock(lockPath, lockFor(manifest, res.plugins));
  }
  return summary(res);
}

/**
 * `s2s update [names]`: re-resolve the named plugins (all when none) to the newest version their
 * range allows, as `npm update` does — the same thing the runtime does at every startup.
 */
export async function serverUpdate(opts: {
  root: string;
  client: ServerClient;
  names?: string[];
  dryRun?: boolean;
  log?: (m: string) => void;
}): Promise<string> {
  const log = opts.log ?? (() => {});
  const manifest = readServerManifest(opts.root);
  const lockPath = join(opts.root, SERVER_LOCK);
  const lock = readLock(lockPath);
  const names = opts.names && opts.names.length > 0 ? opts.names : Object.keys(manifest.dependencies);
  for (const n of names) {
    if (!(n in manifest.dependencies)) throw new Error(`${n} is not in ${SERVER_MANIFEST} dependencies`);
  }
  const specs = pinnedSpecs(manifest, lock, sat, new Set(names));
  const { targets, warnings } = await resolveTargets(opts.client, specs, manifest.with, {
    reviewedOnly: manifest.reviewedOnly,
  });
  for (const w of warnings) log(`warning: ${w}`);
  const res = await syncPlugins({ root: opts.root, targets, oldLock: lock, client: opts.client, dryRun: opts.dryRun, log });
  if (!opts.dryRun) writeLock(lockPath, lockFor(manifest, res.plugins));
  return summary(res);
}

interface LocalPlugin {
  filename: string;
  id: string;
  version: string;
  sha256: string;
}

/**
 * Every `*.s2sp` directly in plugins/ (the loader's own non-recursive rule), with its manifest.
 * `onUnreadable` hears about files that are not valid archives.
 */
export function scanLocalPlugins(root: string, onUnreadable: (filename: string) => void = () => {}): LocalPlugin[] {
  const dir = join(root, PLUGINS_SUBDIR);
  if (!existsSync(dir)) return [];
  const out: LocalPlugin[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith(".s2sp")) continue;
    const bytes = readFileSync(join(dir, entry.name));
    try {
      const files = unzipSync(new Uint8Array(bytes), { filter: (f) => f.name === "manifest.json" });
      const m = JSON.parse(Buffer.from(files["manifest.json"]!).toString("utf8")) as { id?: unknown; version?: unknown };
      if (typeof m.id !== "string" || typeof m.version !== "string") throw new Error("no id/version");
      out.push({ filename: entry.name, id: m.id, version: m.version, sha256: sha256Hex(bytes) });
    } catch {
      onUnreadable(entry.name);
    }
  }
  return out.sort((a, b) => a.filename.localeCompare(b.filename));
}

export interface InitReport {
  managed: { filename: string; name: string; version: string }[];
  unmanaged: { filename: string; name: string; reason: string }[];
}

/**
 * `s2s server init`: write plugins.json + plugins-lock.json for the plugins already installed.
 * A plugin becomes a dependency (^its version) when the registry publishes that exact version with
 * the same bytes; anything else (not published, or a local build) stays unmanaged and untouched.
 */
export async function serverInit(opts: {
  root: string;
  client: Planner;
  name?: string;
  force?: boolean;
}): Promise<InitReport> {
  const root = resolve(opts.root);
  const pkgPath = join(root, SERVER_MANIFEST);
  if (existsSync(pkgPath) && !opts.force) {
    throw new Error(`${pkgPath} already exists — pass --force to replace it`);
  }
  const report: InitReport = { managed: [], unmanaged: [] };
  const dependencies: Record<string, string> = {};
  const plugins: Record<string, LockedPlugin> = {};

  const local = scanLocalPlugins(root, (filename) =>
    report.unmanaged.push({ filename, name: "", reason: "not a readable .s2sp" }),
  );
  for (const p of local) {
    let entry: PlanEntry | undefined;
    try {
      entry = (await opts.client.plan(p.id, p.version)).install.find((e) => e.name === p.id);
    } catch (e) {
      if (!(e instanceof RegistryError) || e.status !== 404) throw e;
    }
    if (!entry || entry.version !== p.version) {
      report.unmanaged.push({ filename: p.filename, name: p.id, reason: `${p.id}@${p.version} is not in the registry` });
      continue;
    }
    if (entry.sha256 !== p.sha256) {
      report.unmanaged.push({
        filename: p.filename,
        name: p.id,
        reason: `differs from the published ${p.id}@${p.version} (a local build?)`,
      });
      continue;
    }
    dependencies[p.id] = caret(p.version);
    plugins[lockKey(p.filename)] = {
      name: p.id,
      version: p.version,
      resolved: entry.url,
      integrity: integrityFromSha256Hex(p.sha256),
    };
    report.managed.push({ filename: p.filename, name: p.id, version: p.version });
  }

  const raw = newServerManifestJson(opts.name ?? "s2script-server", dependencies);
  writeFileSync(pkgPath, JSON.stringify(raw, null, 2) + "\n");
  const manifest: ServerManifest = parseServerManifest(root, raw);
  writeLock(join(root, SERVER_LOCK), lockFor(manifest, plugins));
  return report;
}

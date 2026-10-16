/**
 * A server's `plugins-lock.json`: npm's lockfile v3 shape, describing which registry plugin files
 * sit in `plugins/`. Keys of `packages` are install paths relative to the lock, as in npm
 * (`plugins/_edge_rtv.s2sp` where npm would say `node_modules/rtv`); `""` is the server itself.
 *
 * The lock is also the ownership record: the CLI and the runtime's boot updater replace or remove
 * only files the lock lists, so a `.s2sp` an operator dropped in by hand is never touched.
 *
 * Serialization is byte-for-byte fixed (field order, sorted keys, 2-space indent, trailing
 * newline) because core/src/server_update.rs writes the same file at boot; the shared fixture in
 * test/fixtures/server-lock pins both writers to it.
 */

import { createHash } from "node:crypto";
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

export const LOCKFILE_VERSION = 3;

export interface LockedPlugin {
  name: string;
  version: string;
  /** Where the file was downloaded from (the registry's download URL). */
  resolved: string;
  /** Subresource-integrity string over the .s2sp bytes: `sha256-<base64>`. */
  integrity: string;
}

export interface LockRoot {
  name: string;
  /** plugins.json `dependencies` as last resolved; a change means the lock is stale. */
  dependencies: Record<string, string>;
  /** plugins.json `with` as last resolved (bundle optional members). */
  with?: Record<string, string[]>;
  /** plugins.json `disabled` as last resolved. */
  disabled?: string[];
}

export interface ServerLock {
  name: string;
  root: LockRoot;
  /** Install path (`plugins/<file>`) → plugin. */
  plugins: Record<string, LockedPlugin>;
}

export function integrityFromSha256Hex(hex: string): string {
  return `sha256-${Buffer.from(hex, "hex").toString("base64")}`;
}

/** The hex sha256 an integrity string names, or null when it is not a sha256 SRI. */
export function sha256HexFromIntegrity(integrity: string): string | null {
  const m = /^sha256-([A-Za-z0-9+/]+=*)$/.exec(integrity);
  if (!m) return null;
  const bytes = Buffer.from(m[1]!, "base64");
  return bytes.length === 32 ? bytes.toString("hex") : null;
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function sortedRecord<T>(r: Record<string, T>): Record<string, T> {
  const out: Record<string, T> = {};
  for (const k of Object.keys(r).sort()) out[k] = r[k]!;
  return out;
}

/** The exact bytes of a lockfile. */
export function serializeLock(lock: ServerLock): string {
  const root: Record<string, unknown> = {
    name: lock.root.name,
    dependencies: sortedRecord(lock.root.dependencies),
  };
  if (lock.root.with && Object.keys(lock.root.with).length > 0) {
    root.with = sortedRecord(lock.root.with);
  }
  if (lock.root.disabled && lock.root.disabled.length > 0) {
    root.disabled = [...lock.root.disabled].sort();
  }
  const packages: Record<string, unknown> = { "": root };
  for (const key of Object.keys(lock.plugins).sort()) {
    const p = lock.plugins[key]!;
    packages[key] = { name: p.name, version: p.version, resolved: p.resolved, integrity: p.integrity };
  }
  return (
    JSON.stringify(
      { name: lock.name, lockfileVersion: LOCKFILE_VERSION, requires: true, packages },
      null,
      2,
    ) + "\n"
  );
}

export function parseLock(text: string, label = "plugins-lock.json"): ServerLock {
  const j = JSON.parse(text) as {
    name?: unknown;
    lockfileVersion?: unknown;
    packages?: Record<string, unknown>;
  };
  if (!j || typeof j !== "object" || !j.packages || typeof j.packages !== "object") {
    throw new Error(`${label}: not a lockfile (no "packages")`);
  }
  if (j.lockfileVersion !== LOCKFILE_VERSION) {
    throw new Error(`${label}: lockfileVersion ${String(j.lockfileVersion)} is not supported (want ${LOCKFILE_VERSION})`);
  }
  const rootRaw = (j.packages[""] ?? {}) as {
    name?: unknown;
    dependencies?: unknown;
    with?: unknown;
    disabled?: unknown;
  };
  const plugins: Record<string, LockedPlugin> = {};
  for (const [key, raw] of Object.entries(j.packages)) {
    if (key === "") continue;
    const p = raw as Partial<LockedPlugin>;
    if (
      !key.startsWith("plugins/") ||
      typeof p.name !== "string" ||
      typeof p.version !== "string" ||
      typeof p.resolved !== "string" ||
      typeof p.integrity !== "string"
    ) {
      throw new Error(`${label}: malformed entry ${JSON.stringify(key)}`);
    }
    plugins[key] = { name: p.name, version: p.version, resolved: p.resolved, integrity: p.integrity };
  }
  const deps = rootRaw.dependencies && typeof rootRaw.dependencies === "object" ? rootRaw.dependencies : {};
  const withRaw = rootRaw.with;
  const disabledRaw = Array.isArray(rootRaw.disabled) ? (rootRaw.disabled as unknown[]).filter((d): d is string => typeof d === "string") : [];
  return {
    name: typeof j.name === "string" ? j.name : "",
    root: {
      name: typeof rootRaw.name === "string" ? rootRaw.name : "",
      dependencies: deps as Record<string, string>,
      ...(withRaw && typeof withRaw === "object" ? { with: withRaw as Record<string, string[]> } : {}),
      ...(disabledRaw.length > 0 ? { disabled: disabledRaw } : {}),
    },
    plugins,
  };
}

export function readLock(path: string): ServerLock | null {
  if (!existsSync(path)) return null;
  return parseLock(readFileSync(path, "utf8"), path);
}

/** Write via a temp file and rename, so a crash never leaves a half-written lock. */
export function writeLock(path: string, lock: ServerLock): void {
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, serializeLock(lock));
  renameSync(tmp, path);
}

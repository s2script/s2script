/**
 * A game server's `plugins.json` (in `addons/s2script/`, beside `plugins/`): the plugins the
 * server runs, as npm-style `dependencies` (name → semver range). Deliberately not
 * `package.json`, so npm never treats the folder as a Node project — `npm install` there would
 * fetch same-named packages from npmjs.com and rewrite the lock.
 *
 *   {
 *     "name": "my-server",
 *     "dependencies": { "@edge/rtv": "^1.2.0" },
 *     "with": { "@edge/jailbreak": ["@edge/jailbreak-gangperks"] },  // optional bundle members
 *     "disabled": ["@edge/rtv"],   // kept in plugins/disabled/: installed and updated, never loaded
 *     "autoUpdate": true,          // the runtime updates within these ranges at startup (default)
 *     "reviewedOnly": false,       // refuse plugins s2script has not reviewed
 *     "updateTimeoutSeconds": 60,  // how long plugin loading may wait for that update
 *     "registry": "https://www.s2script.com"
 *   }
 *
 * Read narrowly, written back wide: unknown fields survive every rewrite.
 */

import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { validRange } from "semver";

export const SERVER_MANIFEST = "plugins.json";
export const SERVER_LOCK = "plugins-lock.json";
export const PLUGINS_SUBDIR = "plugins";
/** Under plugins/: the loader's scan is non-recursive, so files here never load. */
export const DISABLED_SUBDIR = "disabled";

export interface ServerManifest {
  /** Absolute addon root (the directory holding plugins.json and plugins/). */
  root: string;
  /** The whole parsed file, kept so a rewrite preserves fields we do not model. */
  raw: Record<string, unknown>;
  name: string;
  dependencies: Record<string, string>;
  with: Record<string, string[]>;
  /** Dependencies kept in plugins/disabled/, where the loader never looks. */
  disabled: string[];
  autoUpdate: boolean;
  reviewedOnly: boolean;
}

function readJson(path: string): Record<string, unknown> | null {
  const j = JSON.parse(readFileSync(path, "utf8"));
  if (!j || typeof j !== "object" || Array.isArray(j)) throw new Error(`${path}: must be a JSON object`);
  return j as Record<string, unknown>;
}

/**
 * The addon root a server command applies to: `start` itself when it holds plugins.json, else
 * `start/addons/s2script` (run from the game directory). Null when neither does.
 */
export function findServerRoot(start: string): string | null {
  for (const dir of [resolve(start), resolve(start, "addons", "s2script")]) {
    if (existsSync(join(dir, SERVER_MANIFEST))) return dir;
  }
  return null;
}

/**
 * Where a server's plugins.json belongs when it has none yet: `start` when it holds plugins/,
 * else `start/addons/s2script` when that exists (run from the game directory). Null otherwise.
 */
export function findAddonRoot(start: string): string | null {
  const found = findServerRoot(start);
  if (found) return found;
  if (existsSync(resolve(start, PLUGINS_SUBDIR))) return resolve(start);
  const nested = resolve(start, "addons", "s2script");
  return existsSync(nested) ? nested : null;
}

export function parseServerManifest(root: string, raw: Record<string, unknown>): ServerManifest {
  const label = join(root, SERVER_MANIFEST);
  const depsRaw = raw.dependencies ?? {};
  if (typeof depsRaw !== "object" || Array.isArray(depsRaw)) {
    throw new Error(`${label}: "dependencies" must be an object (plugin name → version range)`);
  }
  const dependencies: Record<string, string> = {};
  for (const [name, range] of Object.entries(depsRaw as Record<string, unknown>)) {
    if (typeof range !== "string" || validRange(range) === null) {
      throw new Error(`${label}: dependency ${name} has an invalid range ${JSON.stringify(range)}`);
    }
    dependencies[name] = range;
  }
  const withMembers: Record<string, string[]> = {};
  if (raw.with !== undefined) {
    if (!raw.with || typeof raw.with !== "object" || Array.isArray(raw.with)) {
      throw new Error(`${label}: "with" must map a bundle name to its optional members`);
    }
    for (const [bundle, members] of Object.entries(raw.with as Record<string, unknown>)) {
      if (!Array.isArray(members) || members.some((m) => typeof m !== "string")) {
        throw new Error(`${label}: with[${JSON.stringify(bundle)}] must be an array of names`);
      }
      if (!(bundle in dependencies)) {
        throw new Error(`${label}: "with" names ${bundle}, which is not in dependencies`);
      }
      withMembers[bundle] = members as string[];
    }
  }
  const disabled: string[] = [];
  if (raw.disabled !== undefined) {
    if (!Array.isArray(raw.disabled) || raw.disabled.some((d) => typeof d !== "string")) {
      throw new Error(`${label}: "disabled" must be an array of plugin names`);
    }
    for (const d of raw.disabled as string[]) {
      if (!(d in dependencies)) throw new Error(`${label}: "disabled" names ${d}, which is not in dependencies`);
      if (!disabled.includes(d)) disabled.push(d);
    }
  }
  return {
    root,
    raw,
    name: typeof raw.name === "string" && raw.name ? raw.name : basename(root),
    dependencies,
    with: withMembers,
    disabled: disabled.sort(),
    autoUpdate: raw.autoUpdate !== false,
    reviewedOnly: raw.reviewedOnly === true,
  };
}

export function readServerManifest(root: string): ServerManifest {
  const path = join(root, SERVER_MANIFEST);
  if (!existsSync(path)) throw new Error(`no ${SERVER_MANIFEST} in ${root} — run \`s2s server init\` first`);
  return parseServerManifest(root, readJson(path)!);
}

/** A fresh plugins.json, for `s2s server init` or the first `s2s install`. */
export function newServerManifestJson(name: string, dependencies: Record<string, string>): Record<string, unknown> {
  return { name, dependencies, autoUpdate: true };
}

/** Write plugins.json back with `dependencies` and `with` from `m`, every other field kept. */
export function writeServerManifest(m: ServerManifest): void {
  const sorted: Record<string, string> = {};
  for (const k of Object.keys(m.dependencies).sort()) sorted[k] = m.dependencies[k]!;
  const out: Record<string, unknown> = { ...m.raw, dependencies: sorted };
  if (Object.keys(m.with).length > 0) out.with = m.with;
  else delete out.with;
  if (m.disabled.length > 0) out.disabled = [...m.disabled].sort();
  else delete out.disabled;
  const path = join(m.root, SERVER_MANIFEST);
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify(out, null, 2) + "\n");
  renameSync(tmp, path);
}

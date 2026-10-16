/**
 * A game server's `s2script.json` (in `addons/s2script/`, beside `plugins/`): the plugins the
 * server runs, as npm-style `dependencies` (name → semver range). Its own filename, not
 * `package.json`, so npm never treats the folder as a Node project — `npm install` there would
 * fetch same-named packages from npmjs.com and rewrite the lock.
 *
 *   {
 *     "name": "my-server",
 *     "dependencies": { "@edge/rtv": "^1.2.0" },
 *     "with": { "@edge/jailbreak": ["@edge/jailbreak-gangperks"] },  // optional bundle members
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

export const SERVER_MANIFEST = "s2script.json";
export const SERVER_LOCK = "s2script-lock.json";
export const PLUGINS_SUBDIR = "plugins";

export interface ServerManifest {
  /** Absolute addon root (the directory holding s2script.json and plugins/). */
  root: string;
  /** The whole parsed file, kept so a rewrite preserves fields we do not model. */
  raw: Record<string, unknown>;
  name: string;
  dependencies: Record<string, string>;
  with: Record<string, string[]>;
  autoUpdate: boolean;
  reviewedOnly: boolean;
}

function readJson(path: string): Record<string, unknown> | null {
  const j = JSON.parse(readFileSync(path, "utf8"));
  if (!j || typeof j !== "object" || Array.isArray(j)) throw new Error(`${path}: must be a JSON object`);
  return j as Record<string, unknown>;
}

/**
 * The addon root a server command applies to: `start` itself when it holds s2script.json, else
 * `start/addons/s2script` (run from the game directory). Null when neither does.
 */
export function findServerRoot(start: string): string | null {
  for (const dir of [resolve(start), resolve(start, "addons", "s2script")]) {
    if (existsSync(join(dir, SERVER_MANIFEST))) return dir;
  }
  return null;
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
  return {
    root,
    raw,
    name: typeof raw.name === "string" && raw.name ? raw.name : basename(root),
    dependencies,
    with: withMembers,
    autoUpdate: raw.autoUpdate !== false,
    reviewedOnly: raw.reviewedOnly === true,
  };
}

export function readServerManifest(root: string): ServerManifest {
  const path = join(root, SERVER_MANIFEST);
  if (!existsSync(path)) throw new Error(`no ${SERVER_MANIFEST} in ${root} — run \`s2s server init\` first`);
  return parseServerManifest(root, readJson(path)!);
}

/** A fresh s2script.json for `s2s server init`. */
export function newServerManifestJson(name: string, dependencies: Record<string, string>): Record<string, unknown> {
  return { name, dependencies, autoUpdate: true };
}

/** Write s2script.json back with `dependencies` replaced, every other field kept. */
export function writeServerManifest(m: ServerManifest, dependencies: Record<string, string>): void {
  const sorted: Record<string, string> = {};
  for (const k of Object.keys(dependencies).sort()) sorted[k] = dependencies[k]!;
  const out = { ...m.raw, dependencies: sorted };
  const path = join(m.root, SERVER_MANIFEST);
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify(out, null, 2) + "\n");
  renameSync(tmp, path);
}

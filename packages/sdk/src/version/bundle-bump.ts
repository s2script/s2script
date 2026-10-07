/**
 * `s2s version`'s bundle step: a bundle release pins its members' exact versions, so whenever any
 * member is released the bundle root must be released too — by the largest bump among its members
 * (a major member bump is a major bundle bump). Changesets cannot do this itself: the workspace
 * root is not a package it versions.
 *
 * Runs after `applyReleasePlan`, reading and rewriting the root package.json in place with its own
 * formatting, the same way the sibling-range step does.
 */

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { inc } from "semver";
import type { BumpType, ComprehensiveRelease } from "./changesets.ts";
import { readJsonFile } from "./ranges.ts";
import { readBundle } from "../workspace/bundle.ts";
import type { Workspace } from "../workspace/workspace.ts";

export interface BundleBump {
  name: string;
  from: string;
  to: string;
  type: Exclude<BumpType, "none">;
  /** The member releases that caused it. */
  because: string[];
}

const LEVEL: Record<BumpType, number> = { none: 0, patch: 1, minor: 2, major: 3 };

/** The bump the bundle needs for `releases`, or null when no member was released. Pure. */
export function plannedBundleBump(ws: Workspace, releases: readonly ComprehensiveRelease[]): BundleBump | null {
  const bundle = readBundle(ws);
  if (!bundle) return null;
  const members = new Set(bundle.members.map((m) => m.name));
  const memberReleases = releases.filter((r) => members.has(r.name) && r.type !== "none");
  if (memberReleases.length === 0) return null;
  const type = memberReleases.reduce<BumpType>((t, r) => (LEVEL[r.type] > LEVEL[t] ? r.type : t), "none") as
    | "major"
    | "minor"
    | "patch";
  const to = inc(bundle.version, type);
  if (to === null) {
    throw new Error(`s2s version: the bundle's version ${JSON.stringify(bundle.version)} is not valid semver`);
  }
  return {
    name: bundle.name,
    from: bundle.version,
    to,
    type,
    because: memberReleases.map((r) => `${r.name}@${r.newVersion}`),
  };
}

/** Apply `plannedBundleBump` to the root package.json. Returns the bump and the file it wrote. */
export function bumpBundle(
  ws: Workspace,
  releases: readonly ComprehensiveRelease[],
): { bump: BundleBump | null; touchedFiles: string[] } {
  const bump = plannedBundleBump(ws, releases);
  if (!bump) return { bump: null, touchedFiles: [] };
  const path = join(ws.root, "package.json");
  const { json, indent, eol } = readJsonFile(path);
  json.version = bump.to;
  writeFileSync(path, JSON.stringify(json, null, indent) + eol);
  return { bump, touchedFiles: [path] };
}

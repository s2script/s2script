/**
 * `s2s version`'s bundle step: a bundle release pins its members' exact versions, so whenever any
 * member is released the bundle must be released too — by the largest bump among its members
 * (a major member bump is a major bundle bump). Changesets cannot do this itself: a bundle's
 * package.json (the workspace root, or a bundle directory's descriptor) is not a package it
 * versions. Each bundle in the workspace follows only its own members.
 *
 * Runs after `applyReleasePlan`, reading and rewriting each bumped bundle's package.json in place
 * with its own formatting, the same way the sibling-range step does.
 */

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { inc } from "semver";
import type { BumpType, ComprehensiveRelease } from "./changesets.ts";
import { readJsonFile } from "./ranges.ts";
import { readBundles } from "../workspace/bundle.ts";
import type { Bundle } from "../workspace/bundle.ts";
import type { Workspace } from "../workspace/workspace.ts";

export interface BundleBump {
  name: string;
  from: string;
  to: string;
  type: Exclude<BumpType, "none">;
  /** The member releases that caused it. */
  because: string[];
  /** The bundle's package.json, absolute. */
  path: string;
}

const LEVEL: Record<BumpType, number> = { none: 0, patch: 1, minor: 2, major: 3 };

/** The bump `bundle` needs for `releases`, or null when none of its members was released. Pure. */
export function plannedBundleBump(bundle: Bundle, releases: readonly ComprehensiveRelease[]): BundleBump | null {
  const members = new Set(bundle.members.map((m) => m.name));
  const memberReleases = releases.filter((r) => members.has(r.name) && r.type !== "none");
  if (memberReleases.length === 0) return null;
  const type = memberReleases.reduce<BumpType>((t, r) => (LEVEL[r.type] > LEVEL[t] ? r.type : t), "none") as
    | "major"
    | "minor"
    | "patch";
  const to = inc(bundle.version, type);
  if (to === null) {
    throw new Error(
      `s2s version: the bundle ${bundle.name}'s version ${JSON.stringify(bundle.version)} is not valid semver`,
    );
  }
  return {
    name: bundle.name,
    from: bundle.version,
    to,
    type,
    because: memberReleases.map((r) => `${r.name}@${r.newVersion}`),
    path: join(bundle.dir, "package.json"),
  };
}

/** The bumps every bundle in `ws` needs for `releases`, in directory order. Pure. */
export function plannedBundleBumps(ws: Workspace, releases: readonly ComprehensiveRelease[]): BundleBump[] {
  return readBundles(ws)
    .map((b) => plannedBundleBump(b, releases))
    .filter((b): b is BundleBump => b !== null);
}

/** Apply `plannedBundleBumps` to each bundle's package.json. Returns the bumps and the files written. */
export function bumpBundles(
  ws: Workspace,
  releases: readonly ComprehensiveRelease[],
): { bumps: BundleBump[]; touchedFiles: string[] } {
  const bumps = plannedBundleBumps(ws, releases);
  for (const bump of bumps) {
    const { json, indent, eol } = readJsonFile(bump.path);
    json.version = bump.to;
    writeFileSync(bump.path, JSON.stringify(json, null, indent) + eol);
  }
  return { bumps, touchedFiles: bumps.map((b) => b.path) };
}

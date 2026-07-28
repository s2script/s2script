/**
 * `s2s version` (design spec 2026-07-27 §7) — changesets' release plan, taught to see the
 * s2script interface graph.
 *
 * The five steps, in order, each one deliberate:
 *
 *   1. Read the REAL workspace packages.
 *   2. Build an in-memory MIRROR injecting each plugin's sibling `s2script.pluginDependencies` as
 *      real dependency edges (`mirror.ts`). Nothing fake is ever written to disk.
 *   3. Hand the MIRROR to `assembleReleasePlan`, so dependents cascade.
 *   4. Apply that plan against the REAL packages via `applyReleasePlan` (versions + CHANGELOGs).
 *      It finds no sibling edges in the real dependency fields, so it correctly no-ops there.
 *   5. One additional pass rewrites `s2script.pluginDependencies` ranges (`ranges.ts`), governed
 *      by the same `updateInternalDependencies` config changesets uses for step 4.
 *
 * The `.changeset/` directory, the config and the pre-release state are all read from the
 * workspace root with the workspace's OWN changesets (`changesets.ts`), so `s2s version` and a
 * plain `changeset version` disagree about nothing except the extra graph edges — which is exactly
 * the difference this command exists to add.
 */

import { existsSync } from "node:fs";
import { join } from "node:path";
import { loadChangesets } from "./changesets.ts";
import { mirrorPackages, realPackages } from "./mirror.ts";
import { rewriteSiblingRanges } from "./ranges.ts";
import type { ComprehensiveRelease } from "./changesets.ts";
import type { RangeRewrite, RangeSkip } from "./ranges.ts";
import { findWorkspaceRoot, loadWorkspace } from "../workspace/workspace.ts";

export interface VersionOptions {
  /** Passed straight through to `@changesets/read` — only changesets added since this git ref. */
  sinceRef?: string;
}

export interface VersionResult {
  /** Absolute workspace root. */
  root: string;
  /** How many `.changeset/*.md` files fed the plan. Zero means the command is a named no-op. */
  changesetCount: number;
  /** Every package the plan released, cascaded dependents included. */
  releases: ComprehensiveRelease[];
  /** Sibling `pluginDependencies` ranges rewritten by step 5. */
  rewrites: RangeRewrite[];
  /** Sibling ranges step 5 deliberately left alone, each with a named reason. */
  skips: RangeSkip[];
  /** Every file written, by changesets (step 4) and by step 5. */
  touchedFiles: string[];
  /** The `@changesets/*` versions the gate accepted — worth showing when a plan looks wrong. */
  changesetsVersions: Record<string, string>;
}

/**
 * Version the workspace rooted at `root`. Writes package versions, CHANGELOGs and sibling ranges;
 * consumes the changesets it applied, exactly as `changeset version` does.
 */
export async function versionWorkspace(root: string, opts: VersionOptions = {}): Promise<VersionResult> {
  const ws = loadWorkspace(root); // step 1 — and the §4.3 shape rules, aggregated
  const configPath = join(ws.root, ".changeset", "config.json");
  if (!existsSync(configPath)) {
    // changesets' own failure here is a bare ENOENT naming a path and no remedy. `s2s create
    // --workspace` writes this file, so a workspace missing it was almost certainly hand-rolled.
    throw new Error(
      `s2s version: ${configPath} does not exist — this workspace is not set up for changesets. ` +
        `Run \`npx changeset init\` at ${ws.root}.`,
    );
  }
  const cs = await loadChangesets(ws.root);

  const real = realPackages(ws);
  const mirror = mirrorPackages(ws); // step 2

  // The config is read against the REAL packages: its `ignore` validation is a statement about the
  // repo as authored, and validating it against injected edges would report problems nobody wrote.
  const config = await cs.readConfig(ws.root, real);
  const preState = await cs.readPreState(ws.root);
  const changesets = await cs.readChangesets(ws.root, opts.sinceRef);

  const plan = cs.assembleReleasePlan(changesets, mirror, config, preState); // step 3
  const touchedFiles = await cs.applyReleasePlan(plan, real, config); // step 4
  const ranges = rewriteSiblingRanges(ws, plan.releases, config); // step 5

  return {
    root: ws.root,
    changesetCount: changesets.length,
    releases: plan.releases,
    rewrites: ranges.rewrites,
    skips: ranges.skips,
    touchedFiles: [...touchedFiles, ...ranges.touchedFiles],
    changesetsVersions: cs.versions,
  };
}

/**
 * The workspace root to version, starting from `from`. A named refusal outside a workspace —
 * versioning is a statement about a SET of packages, so there is no single-plugin meaning to fall
 * back to (contrast §4.2, where every other command silently stays in single-plugin mode).
 */
export function resolveVersionRoot(from: string): string {
  const root = findWorkspaceRoot(from);
  if (root === null) {
    throw new Error(
      `s2s version is a workspace command and ${from} is not inside one — no package.json above it ` +
        `carries an s2script.workspace block. Version a single plugin by editing its package.json.`,
    );
  }
  return root;
}

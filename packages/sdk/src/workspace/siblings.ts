/**
 * Sibling contract resolution (design spec 2026-07-27 §3.2, §5.3, §11).
 *
 * In a workspace the producer's published `.d.ts` is right there on disk, and npm has already
 * symlinked it into `node_modules/<name>` whether or not anyone declared a dependency (§3.1).
 * `moduleResolution: "bundler"` therefore resolves a sibling's `types` field with no `paths`
 * entry and no copy (§3.2) — the ONE thing standing in the way is `typecheck.ts`'s ambient
 * `declare module "<dep>";`, which shadows that resolution (§3.3). This module decides, for one
 * plugin, which of its declared dependencies are workspace siblings, so the gate can leave those
 * alone and `build.ts` can hash the producer's own bytes instead of a copy of them.
 *
 * Two deliberate narrowings, each protecting today's behaviour (§11's compatibility hinge — a
 * dependency that is NOT a workspace sibling keeps its verified copy or its `any` stub, no error):
 *
 *   - **Matched by PACKAGE name, not interface name.** The `publishes` grammar decouples the two
 *     (@edge/mce@3.1.0 may publish @community/mapchooser@1.2.0), but node resolution only knows
 *     the symlink npm made, which carries the PACKAGE name. A dependency naming an interface whose
 *     producer is a differently-named sibling has no symlink to resolve through, so suppressing its
 *     stub would turn a working build into TS2307. The §5.2 range gate is where the interface-name
 *     mapping is honoured; resolution is package-name only.
 *   - **The consumer itself must be covered by npm `workspaces`.** A directory npm does not cover
 *     gets no symlink, so there is nothing to resolve — and it keeps the blast radius off
 *     standalone trees that merely happen to sit under a workspace root (`examples/*`, the SDK's
 *     own test fixtures).
 */

import { readFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { matchGlob, toPosix } from "./glob.ts";
import { findWorkspaceRoot, loadWorkspace, workspaceGlobs } from "./workspace.ts";
import type { WorkspacePackageJson, WorkspacePlugin } from "./workspace.ts";
import { assertPublishesTypes } from "../publish-gate.ts";
import { localContractPath } from "../contracts.ts";

/** A declared dependency that resolves in place to a workspace sibling's published contract. */
export interface SiblingContract {
  /** The sibling plugin's package name — equal to the dependency specifier by construction. */
  name: string;
  /** Absolute directory of the sibling plugin. */
  dir: string;
  /** The sibling's directory relative to the workspace root, `/`-separated (for messages). */
  relDir: string;
  /** Absolute path of the sibling's OWN `types` .d.ts. Never a copy — that is the whole point. */
  typesPath: string;
}

export interface SiblingResolution {
  /** dependency specifier -> the sibling contract resolving it. Empty outside a workspace. */
  siblings: Map<string, SiblingContract>;
  /** Sibling-resolved deps that ALSO carry a stale `.s2script/types/<dep>/index.d.ts` (§5.3). */
  shadowedCopies: string[];
}

/** True when `dir` is one of the directories npm `workspaces` covers — i.e. npm symlinked it. */
function isWorkspaceMemberDir(root: string, rootPkg: WorkspacePackageJson, dir: string): boolean {
  const rel = toPosix(relative(root, dir));
  if (rel === "" || rel.startsWith("..")) return false;
  return workspaceGlobs(rootPkg.workspaces).some((g) => matchGlob(g, rel));
}

/** The warning text for a stale verified copy the sibling now overrides (§5.3, decision #9). */
export function shadowedCopyWarning(pluginDir: string, dep: string): string {
  const copy = localContractPath(pluginDir, dep);
  const where = copy === null ? join(".s2script", "types", dep, "index.d.ts") : relative(pluginDir, copy);
  return (
    `WARN: ${dep} resolves to a workspace sibling, so the verified copy at ${toPosix(where)} is ` +
    `IGNORED and can be deleted — the sibling's own contract is what this build typechecks and hashes`
  );
}

/**
 * Which of `deps` resolve in place to workspace siblings of the plugin at `pluginDir`.
 *
 * Returns empty (never throws) when there is no workspace, when the plugin is not a workspace
 * member, or when no declared dependency names a sibling — the single-plugin path, unchanged.
 *
 * Throws, naming EVERY offender at once (§11), for the two cases that cannot compile:
 *   - a sibling declaring `publishes` but no usable `types`
 *   - a sibling declaring no `publishes` at all, named as a dependency: resolution would fall
 *     through to its `main`, typechecking the consumer against implementation internals that
 *     never cross the context boundary.
 */
export function resolveSiblingContracts(pluginDir: string, deps: string[]): SiblingResolution {
  const siblings = new Map<string, SiblingContract>();
  const shadowedCopies: string[] = [];
  // Fast path first: a plugin with no declared inter-plugin dependency needs no workspace at all,
  // which keeps every non-workspace build (and every SDK fixture) exactly as cheap as it was.
  if (deps.length === 0) return { siblings, shadowedCopies };

  const absDir = resolve(pluginDir);
  const root = findWorkspaceRoot(absDir);
  if (root === null) return { siblings, shadowedCopies };

  const rootPkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as WorkspacePackageJson;
  if (!isWorkspaceMemberDir(root, rootPkg, absDir)) return { siblings, shadowedCopies };

  const ws = loadWorkspace(root);
  const byName = new Map<string, WorkspacePlugin>(ws.plugins.map((p) => [p.name, p]));
  const self = ws.plugins.find((p) => resolve(p.dir) === absDir);
  const label = self?.name ?? toPosix(relative(root, absDir));

  const problems: string[] = [];
  for (const dep of deps) {
    const sib = byName.get(dep);
    if (sib === undefined) continue; // a registry dependency: today's behaviour, no error (§11)
    if (resolve(sib.dir) === absDir) continue; // a plugin naming itself imposes nothing

    const gate = assertPublishesTypes(sib.pkg, sib.dir);
    if (!gate.ok) {
      problems.push(
        `${dep} (workspace sibling at ${sib.relDir}) declares s2script.publishes but its contract ` +
          `cannot be resolved: ${gate.error}`,
      );
      continue;
    }
    if (gate.typesPath === null) {
      problems.push(
        `${dep} (workspace sibling at ${sib.relDir}) declares no s2script.publishes — you cannot ` +
          `depend on an interface nobody publishes. Resolution would fall through to its "main" ` +
          `(${sib.entry}), typechecking this plugin against the sibling's IMPLEMENTATION internals, ` +
          `which never cross the context boundary. Add s2script.publishes and "types" to ` +
          `${sib.relDir}/package.json.`,
      );
      continue;
    }

    siblings.set(dep, { name: sib.name, dir: sib.dir, relDir: sib.relDir, typesPath: gate.typesPath });
    // §5.3 / decision #9: the sibling WINS over a pre-migration copy. Leaving the copy
    // authoritative would reintroduce exactly the silent drift this design removes.
    if (localContractPath(absDir, dep) !== null) shadowedCopies.push(dep);
  }

  if (problems.length > 0) {
    throw new Error(
      `${label} cannot resolve ${problems.length} workspace sibling contract` +
        `${problems.length === 1 ? "" : "s"}:\n  ` +
        problems.join("\n  "),
    );
  }

  return { siblings, shadowedCopies };
}

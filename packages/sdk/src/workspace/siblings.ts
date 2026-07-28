/**
 * Sibling contract resolution (design spec 2026-07-27 §3.2, §5.3, §11).
 *
 * In a workspace the producer's published `.d.ts` is right there on disk, so a consumer needs no
 * copy of it. This module decides, for one plugin, which of its declared dependencies are workspace
 * siblings — so `typecheck.ts` can point tsc straight at each producer's own contract and
 * `build.ts` can hash those same bytes into `compiledAgainst` instead of a copy of them.
 *
 * **The matching key is the PUBLISHED INTERFACE name (§5.3.0, decision #10), never the package
 * name**, and it comes from the one shared index in `interfaces.ts` that `graph.ts` also uses —
 * see that module for why the engine settles it.
 *
 * §3.1/§3.2 originally resolved siblings through npm's `node_modules` symlink, which npm creates
 * for every workspace package whether or not anything depends on it. That is an npm-specific
 * behaviour: pnpm and bun link only what is declared as an NPM dependency, and an s2script plugin
 * dep is declared under `s2script.pluginDependencies`, so neither links anything; yarn PnP has no
 * `node_modules` at all. The symlink also carries the PACKAGE name, so it could never reach a
 * producer publishing under a decoupled interface name. `typecheck.ts` therefore maps every sibling
 * interface explicitly and depends on no package manager. There is still exactly one copy of the
 * bytes, which is what makes drift impossible.
 *
 * One deliberate narrowing, protecting today's behaviour (§11's compatibility hinge — a dependency
 * that is NOT a workspace sibling keeps its verified copy or its `any` stub, no error): **the
 * consumer itself must be a workspace member** (package.json `workspaces`, or `pnpm-workspace.yaml`).
 * That keeps the blast radius off standalone trees which merely happen to sit under a workspace
 * root (`examples/*`, the SDK's own test fixtures).
 */

import { readFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { matchGlob, toPosix } from "./glob.ts";
import { findWorkspaceRoot, scanWorkspace, workspaceGlobs, pnpmWorkspaceGlobs } from "./workspace.ts";
import type { WorkspacePackageJson } from "./workspace.ts";
import { ambiguityProblem, indexInterfaces } from "./interfaces.ts";
import { assertPublishesTypes } from "../publish-gate.ts";
import { localContractPath } from "../contracts.ts";

/** A declared dependency that resolves in place to a workspace sibling's published contract. */
export interface SiblingContract {
  /** The PRODUCER'S package name. Equal to the dependency specifier only for `publishes: "self"`. */
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

/**
 * True when `dir` is a package manager workspace member — package.json `workspaces`
 * (npm/yarn/bun) or `pnpm-workspace.yaml` (pnpm).
 *
 * This is no longer a proxy for "was it symlinked" — `typecheck.ts` maps interfaces directly at the
 * producer's contract and needs no link. It survives as the BLAST-RADIUS guard: it is what keeps a
 * plugin that merely happens to sit beneath a workspace root — `examples/*` and the SDK's own test
 * fixtures, once this repo dogfoods §9 — on the untouched single-plugin path.
 */
function isWorkspaceMemberDir(root: string, rootPkg: WorkspacePackageJson, dir: string): boolean {
  const rel = toPosix(relative(root, dir));
  if (rel === "" || rel.startsWith("..")) return false;
  const globs = [...workspaceGlobs(rootPkg.workspaces), ...pnpmWorkspaceGlobs(root)];
  return globs.some((g) => matchGlob(g, rel));
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
 * member, or when no declared dependency names an interface a sibling publishes — the
 * single-plugin path, unchanged.
 *
 * Throws, naming EVERY offender at once (§11), for the three cases that cannot compile:
 *   - a sibling publishing the named interface but with no usable `types`
 *   - a sibling whose PACKAGE name is the dependency while it publishes nothing at all: you cannot
 *     depend on an interface nobody publishes, and `loader.rs:175`'s `iface_published` can never
 *     be true for it, so the consumer would park in `waiting` forever
 *   - two siblings publishing the named interface: the workspace cannot say which one you get.
 *
 * A dependency naming an interface that a DIFFERENTLY-published sibling happens to be named after
 * is none of those: it is an ordinary registry dependency, keeping its verified copy or its `any`
 * stub (§11's compatibility hinge, §5.3.0's second silent failure).
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

  // SCANNED, not loaded (§4.2, §11): resolving one consumer's declared dependencies needs only the
  // members those dependencies name. `loadWorkspace` aggregates and throws on ANY workspace-wide
  // shape problem, which would make `s2s build plugins/healthy-plugin` fail because an unrelated
  // sibling is malformed. That validation belongs to `preflightWorkspace`, which every
  // workspace-mode path runs before it builds anything.
  const scan = scanWorkspace(root);
  const ws = scan.workspace;
  const { producers, ambiguous, byPlugin } = indexInterfaces(ws.plugins);
  const self = ws.plugins.find((p) => resolve(p.dir) === absDir);
  const label = self?.name ?? toPosix(relative(root, absDir));

  // A member the scan could not read is ABSENT from `ws.plugins`, so it is also absent from
  // `indexInterfaces` — and a dependency it would have published then misses `producers` and looks
  // exactly like an ordinary registry dependency. That silently restores the `any` stub whose
  // removal is the whole point of §5.3, and rule 3's own error text calls that degradation out as
  // worth a hard error. We cannot know whether a dropped member was the producer, so a dep that
  // resolves to NOTHING while members went unread is reported rather than assumed benign.
  //
  // A WARNING, not an error, and only on that conjunction: an unresolved dep is legitimately a
  // registry dependency (§11's compatibility hinge), so failing here would break real builds, and
  // failing whenever ANY member is malformed would undo the leniency this scan exists to provide.
  // Workspace mode is unaffected — `preflightWorkspace`/`loadWorkspace` still hard-fail on the same
  // tree before anything is built. This only softens the single-plugin path, where the author
  // explicitly targeted one plugin.
  const unread = scan.problems ?? [];

  const problems: string[] = [];
  for (const dep of deps) {
    const ambiguousPublishers = ambiguous.get(dep);
    if (ambiguousPublishers !== undefined) {
      problems.push(ambiguityProblem(dep, ambiguousPublishers));
      continue;
    }

    const producer = producers.get(dep);
    if (producer === undefined) {
      // Not published by any sibling. One shape of that is still an authoring error worth naming:
      // a sibling whose PACKAGE name is exactly this dependency while publishing NOTHING can never
      // satisfy it — `iface_published(dep)` is never true, so the consumer parks in `waiting`
      // forever and every `ctx.use` throws `InterfaceUnavailable`. A sibling that publishes some
      // OTHER interface is deliberately not implicated: the name may legitimately be a registry
      // interface, which is §11's compatibility hinge.
      const namesake = ws.plugins.find((p) => p.name === dep && resolve(p.dir) !== absDir);
      if (namesake !== undefined && (byPlugin.get(namesake)?.length ?? 0) === 0) {
        problems.push(
          `${dep} (workspace sibling at ${namesake.relDir}) declares no s2script.publishes — you ` +
            `cannot depend on an interface nobody publishes. The engine resolves a dependency name ` +
            `through iface_published (loader.rs:175), which is never true for a plugin that ` +
            `publishes nothing, so this consumer would park in "waiting" forever and every ctx.use ` +
            `would throw InterfaceUnavailable. Add s2script.publishes and "types" to ` +
            `${namesake.relDir}/package.json.`,
        );
      }
      if (unread.length > 0) {
        console.warn(
          `WARN: ${label}: "${dep}" resolves to no workspace producer, but ${unread.length} ` +
            `workspace member${unread.length === 1 ? "" : "s"} could not be read — one of them may ` +
            `be its producer, in which case this build silently typechecks against \`any\` instead ` +
            `of the real contract. Treating it as a registry dependency for now. Fix:\n  ` +
            unread.join("\n  "),
        );
      }
      continue; // otherwise a registry dependency: today's behaviour, no error (§11)
    }

    const sib = producer.plugin;
    if (resolve(sib.dir) === absDir) continue; // a plugin naming its OWN interface imposes nothing

    const gate = assertPublishesTypes(sib.pkg, sib.dir);
    // `ok` with a null typesPath means "publishes nothing", which the index has already ruled out
    // (a producer is in it BECAUSE it publishes) — the branch is here to keep the type honest.
    if (!gate.ok || gate.typesPath === null) {
      const why = gate.ok ? "it declares no s2script.publishes" : gate.error;
      problems.push(
        `${dep} (published by workspace sibling ${sib.name} at ${sib.relDir}) declares ` +
          `s2script.publishes but its contract cannot be resolved: ${why}`,
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

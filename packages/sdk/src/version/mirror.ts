/**
 * The in-memory mirror (design spec 2026-07-27 §7 step 2) — the central trick of `s2s version`.
 *
 * Changesets cannot see `s2script.pluginDependencies`, so on its own it will version a producer
 * and leave every consumer of that producer's interface untouched. Rather than reimplement its
 * dependents algorithm, we show it the s2script graph through a field it already understands: a
 * DEEP CLONE of the workspace's packages with each sibling plugin dependency injected as a real
 * dependency edge.
 *
 * **Nothing fake is ever written to disk.** That is the invariant this whole module exists to
 * uphold, and it is why every injection happens on a `structuredClone`: `applyReleasePlan` writes
 * back the WHOLE `packageJson` object it was handed (`updatePackageJson` re-serialises it), so a
 * mirror that shared one object with the real package set would persist the injected edge. The
 * mirror is handed to `assembleReleasePlan` (which only reads); the REAL packages are handed to
 * `applyReleasePlan` (which writes). `version.test.mjs` asserts no injected dependency reaches any
 * package.json on disk.
 */

import { isConcreteVersion } from "../publishes.ts";
import { publishedInterfaces } from "../workspace/graph.ts";
import type { Workspace, WorkspaceMember, WorkspacePlugin } from "../workspace/workspace.ts";
import type { ChangesetsPackage, ChangesetsPackages } from "./changesets.ts";

/** One `s2script.pluginDependencies` entry whose producer is a plugin in this same workspace. */
export interface SiblingEdge {
  /** The consuming plugin's PACKAGE name (what changesets keys on). */
  consumer: string;
  /** The producing plugin's PACKAGE name. */
  producer: string;
  /** The INTERFACE name as authored in `pluginDependencies` — may differ from `producer`. */
  iface: string;
  /** The authored range. */
  range: string;
}

/**
 * Every sibling dependency edge changesets should cascade through.
 *
 * Two narrowings, both about the `publishes` grammar deliberately decoupling the interface name
 * and version from the package's (`@edge/mce@3.1.0` may publish `@community/mapchooser@1.2.0`):
 *
 *   - The edge is keyed on the PRODUCER PACKAGE, because that is the only identity changesets
 *     versions. The interface name is carried along so §7 step 5 can rewrite the right key.
 *   - An interface whose published version does NOT track its package version is skipped
 *     entirely. Its version is authored by hand in `publishes` and changesets never touches it, so
 *     cascading a package bump into that consumer's range would rewrite `^1.2.0` to the producer's
 *     unrelated `^3.2.0` — a fabricated dependency, which is worse than no cascade at all.
 */
export function siblingEdges(ws: Workspace): SiblingEdge[] {
  // interface name -> producing plugin, restricted to interfaces that track the package version.
  const producerOf = new Map<string, WorkspacePlugin>();
  for (const p of ws.plugins) {
    for (const [iface, version] of Object.entries(publishedInterfaces(p))) {
      if (!isConcreteVersion(version)) continue;
      if (version.trim() !== p.version) continue; // hand-authored interface version: not ours to bump
      producerOf.set(iface, p);
    }
  }

  const edges: SiblingEdge[] = [];
  for (const consumer of ws.plugins) {
    for (const [iface, range] of Object.entries(consumer.pkg.s2script?.pluginDependencies ?? {})) {
      if (typeof range !== "string") continue;
      const producer = producerOf.get(iface);
      if (producer === undefined) continue; // a registry dependency — no in-workspace edge (§11)
      if (producer.name === consumer.name) continue; // its own interface imposes nothing
      edges.push({ consumer: consumer.name, producer: producer.name, iface, range });
    }
  }
  return edges;
}

function toPackage(member: WorkspaceMember): ChangesetsPackage {
  return { packageJson: member.pkg as unknown as Record<string, unknown>, dir: member.dir };
}

/**
 * The workspace exactly as it is on disk, in changesets' `@manypkg/get-packages` shape. These are
 * the objects `applyReleasePlan` mutates and writes back, so they must be the real ones.
 *
 * `tool: "root"` is what manypkg reports for an npm-workspaces repo (npm workspaces are declared in
 * the root package.json, which is manypkg's "root" tool). Neither entry point branches on it.
 */
export function realPackages(ws: Workspace): ChangesetsPackages {
  const rootName = ws.pkg.name;
  if (typeof rootName !== "string" || rootName === "") {
    // get-dependents-graph indexes the root by name; an unnamed root fails deep inside changesets
    // with a message that names neither the file nor the field.
    throw new Error(
      `s2s version: the workspace root package.json at ${ws.root} has no "name" — changesets keys ` +
        `its dependency graph on package names, including the root's`,
    );
  }
  return {
    tool: "root",
    packages: [...ws.plugins, ...ws.libs].map(toPackage),
    root: { packageJson: ws.pkg as unknown as Record<string, unknown>, dir: ws.root },
  };
}

/**
 * The same workspace with every sibling plugin dependency injected as a `dependencies` edge, on
 * clones. Feed this to `assembleReleasePlan` and NOTHING else.
 *
 * **Why `dependencies` and not `devDependencies`.** The spec's §7 step 2 says devDependencies;
 * measured against the installed `@changesets/assemble-release-plan`, that field provably cannot
 * cascade: `determineDependents` switches on the dependency type and assigns `type = "none"` for a
 * devDependency ("we don't need a version bump if the package is only in the devDependencies"), so
 * the dependent lands in the plan at its OLD version. A `pluginDependency` is a hard runtime
 * dependency — the loader refuses a consumer whose declared range no longer matches the producer's
 * published interface — so rewriting that range (§7 step 5) without republishing the consumer would
 * change a shipped artifact's contract at an unchanged version. `dependencies` is therefore both
 * the semantically honest field and the only one that produces the cascade §7 asks for. The
 * injection is still invisible on disk either way; the choice only decides what changesets infers.
 */
export function mirrorPackages(ws: Workspace): ChangesetsPackages {
  const real = realPackages(ws);
  const mirror: ChangesetsPackages = {
    tool: real.tool,
    // structuredClone, not a spread: `versionPackage` mutates `packageJson[depType]` in place, and
    // a shallow copy would hand it the real `dependencies` object.
    packages: real.packages.map((p) => ({ dir: p.dir, packageJson: structuredClone(p.packageJson) })),
    root: { dir: real.root.dir, packageJson: structuredClone(real.root.packageJson) },
  };

  const byName = new Map(mirror.packages.map((p) => [String(p.packageJson.name), p]));
  for (const edge of siblingEdges(ws)) {
    const consumer = byName.get(edge.consumer);
    if (consumer === undefined) continue;
    const deps = (consumer.packageJson.dependencies ?? {}) as Record<string, string>;
    // A real npm dependency on the same package wins: it is already on disk, already honoured by
    // changesets, and overwriting it would make the mirror disagree with what will be written back.
    if (deps[edge.producer] === undefined) deps[edge.producer] = edge.range;
    consumer.packageJson.dependencies = deps;
  }

  return mirror;
}

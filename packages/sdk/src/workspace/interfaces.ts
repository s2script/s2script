/**
 * The sibling matching key (design spec 2026-07-27 §5.3.0, decision #10).
 *
 * A declared dependency name `D` resolves to a workspace sibling **iff `D` is one of the
 * interfaces that sibling `publishes`** — never merely because some sibling's PACKAGE name equals
 * `D`. The two coincide only for `publishes: "self"`; the map form deliberately decouples them
 * (`@edge/mce@3.1.0` publishing `@community/mapchooser@1.2.0`).
 *
 * The engine settles it. `core/src/loader.rs:175`:
 *
 * ```rust
 * fn deps_satisfied(manifest: &Manifest) -> bool {
 *     manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n))
 * }
 * ```
 *
 * A dependency key is an INTERFACE name at load time, so it must be an interface name at build
 * time. Keying on the package name produces two failures no gate can see: a map-form sibling named
 * by its package name builds green but `iface_published(D)` is never true, so the consumer parks in
 * `waiting` forever; and a sibling whose package name happens to match a dependency it does not
 * publish shadows the consumer's legitimate `.s2script/types/<dep>/index.d.ts`, so the build hashes
 * the WRONG contract into `compiledAgainst` and `loader.rs:192` refuses the load.
 *
 * Hence this module: ONE index, keyed on published interface names, shared by `siblings.ts` (which
 * decides what typechecks and what gets hashed) and `graph.ts` (ordering + the §5.2 range gate).
 * They must not each compute membership, because that is precisely how the two halves drifted.
 *
 * Everything that can go wrong with a `publishes` block is ATTRIBUTED to the plugin that wrote it
 * and returned rather than thrown (§11): `expandPublishes` throws on a malformed grammar, and an
 * uncaught throw here names no plugin, no directory, and aborts an eighteen-plugin build.
 */

import { expandPublishes } from "../publishes.ts";
import type { WorkspacePlugin } from "./workspace.ts";

/** The single sibling publishing one interface name, and the version it publishes it at. */
export interface InterfaceProducer {
  plugin: WorkspacePlugin;
  /**
   * The version from the authored `publishes` block — the PUBLISHED INTERFACE version, which is
   * what `interfaces.rs::call_target_inner` range-checks at runtime, not the package version.
   */
  version: string;
}

export interface InterfaceIndex {
  /** interface name -> its producer. An ambiguously-published name is deliberately absent. */
  producers: Map<string, InterfaceProducer>;
  /** interface name -> every publisher of it, when more than one plugin publishes that name. */
  ambiguous: Map<string, string[]>;
  /**
   * plugin -> the interface names it publishes. Keyed by object identity rather than by name,
   * because a duplicate `name` is a workspace problem this index must survive rather than assume
   * away (workspace.ts raises it; see `loadWorkspace`).
   */
  byPlugin: Map<WorkspacePlugin, string[]>;
  /** Attributed, report-ready problems: malformed `publishes` grammar, ambiguous interfaces. */
  problems: string[];
}

/** `<name> (<relDir>)` — every message here names the plugin AND where it lives on disk. */
function where(p: WorkspacePlugin): string {
  return `${p.name} (${p.relDir})`;
}

/** Index every interface the batch publishes, collecting rather than throwing on any problem. */
export function indexInterfaces(plugins: readonly WorkspacePlugin[]): InterfaceIndex {
  const producers = new Map<string, InterfaceProducer>();
  const ambiguous = new Map<string, string[]>();
  const byPlugin = new Map<WorkspacePlugin, string[]>();
  const problems: string[] = [];

  for (const p of plugins) {
    let expanded: Record<string, string>;
    try {
      expanded = expandPublishes(p.pkg.s2script?.publishes, p.name, p.version);
    } catch (e) {
      // Attributed + aggregated (§11): the grammar error names the plugin that wrote it and does
      // not abort the other seventeen builds.
      problems.push(`${where(p)}: invalid s2script.publishes — ${(e as Error).message}`);
      byPlugin.set(p, []);
      continue;
    }
    byPlugin.set(p, Object.keys(expanded));
    for (const [iface, version] of Object.entries(expanded)) {
      const existing = producers.get(iface);
      if (existing !== undefined && existing.plugin !== p) {
        const publishers = ambiguous.get(iface) ?? [where(existing.plugin)];
        publishers.push(where(p));
        ambiguous.set(iface, publishers);
        continue;
      }
      producers.set(iface, { plugin: p, version });
    }
  }

  // An ambiguous name resolves to NOBODY, not to whichever plugin the directory walk reached
  // first: a coin-flip producer is how a consumer typechecks against one contract and loads
  // against another.
  for (const [iface, publishers] of ambiguous) {
    producers.delete(iface);
    problems.push(ambiguityProblem(iface, publishers));
  }

  return { producers, ambiguous, byPlugin, problems };
}

/** The named hard error for two siblings publishing one interface name (§5.3.0). */
export function ambiguityProblem(iface: string, publishers: readonly string[]): string {
  return (
    `interface ${JSON.stringify(iface)} is published by ${publishers.length} plugins ` +
    `(${publishers.join(", ")}) — the workspace cannot say which producer a consumer gets, and the ` +
    `engine refuses the second publish anyway (interfaces.rs: implementations are alternatives, ` +
    `load only one)`
  );
}

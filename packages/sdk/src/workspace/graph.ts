/**
 * The plugin dependency graph: build/publish order and the sibling range gate
 * (design spec 2026-07-27 §4.4, §5.2).
 *
 * `topoOrder` is a deliberate PORT of `core/src/loader.rs::topo_order`, not an independent
 * implementation. Build order and load order must agree, so it is the same algorithm with the
 * same three decisions: edges from HARD dependencies only (an optional dep imposes none), a
 * plugin depending on its own published interface imposes no edge, and a cycle WARNS and falls
 * back to lexicographic order rather than failing. That last one matters: the engine deliberately
 * runs a cyclic set (the hard-dep proxy is lazy, so a mis-ordered pair still runs and throws
 * `InterfaceUnavailable` only at call time), so a build that hard-errored here would refuse to
 * produce a plugin set the runtime is designed to load. Its unit tests mirror loader.rs's
 * `topo_cycle_falls_back_to_name_order`.
 *
 * Ordering is NOT a build dependency: a producer's `api.d.ts` is authored, not generated, so a
 * consumer does not need its producer built first. Topological order buys deterministic output,
 * publish order, and cycle detection — nothing more (§5.1).
 */

import { satisfies } from "semver";
import { expandPublishes, isConcreteVersion } from "../publishes.ts";
import type { WorkspacePlugin } from "./workspace.ts";

/** One plugin as the ordering sees it: an id, the interfaces it hard-depends on, the ones it publishes. */
export interface GraphNode {
  id: string;
  /** Interface names from `s2script.pluginDependencies` — HARD deps only. */
  dependsOn: string[];
  /** Interface names from `s2script.publishes` (which need not equal the package name). */
  publishes: string[];
}

/**
 * Order a batch so an interface's producer comes before its hard-dep consumers.
 *
 * Kahn's algorithm with a stable lexicographic tie-break by id. On a cycle: `warn` is called once
 * and every not-yet-emitted id is appended in lexicographic order. NEVER throws.
 */
export function topoOrder(batch: GraphNode[], warn: (msg: string) => void = console.warn): string[] {
  // interface name -> the in-batch producer's id. A dep with no in-batch producer (satisfied from
  // the registry, or simply absent) imposes no edge, exactly as in loader.rs.
  const producerOf = new Map<string, string>();
  for (const node of batch) {
    for (const iface of node.publishes) producerOf.set(iface, node.id);
  }

  const consumersOf = new Map<string, string[]>();
  const indegree = new Map<string, number>();
  for (const node of batch) indegree.set(node.id, 0);
  for (const node of batch) {
    for (const dep of node.dependsOn) {
      const producer = producerOf.get(dep);
      if (producer === undefined) continue;
      if (producer === node.id) continue; // a plugin depending on its own interface: no edge
      const cs = consumersOf.get(producer);
      if (cs) cs.push(node.id);
      else consumersOf.set(producer, [node.id]);
      indegree.set(node.id, (indegree.get(node.id) ?? 0) + 1);
    }
  }

  const order: string[] = [];
  const ready: string[] = [...indegree.entries()].filter(([, d]) => d === 0).map(([id]) => id);
  ready.sort();
  while (ready.length > 0) {
    const next = ready.shift() as string;
    order.push(next);
    const cs = consumersOf.get(next);
    if (!cs) continue;
    let grew = false;
    for (const c of cs) {
      const d = (indegree.get(c) ?? 0) - 1;
      indegree.set(c, Math.max(0, d));
      if (d === 0) {
        ready.push(c);
        grew = true;
      }
    }
    if (grew) ready.sort();
  }

  if (order.length !== batch.length) {
    const emitted = new Set(order);
    const remaining = batch.map((n) => n.id).filter((id) => !emitted.has(id));
    remaining.sort();
    warn(
      `WARN: plugin dependency cycle (${remaining.join(", ")}) — using name order; the engine warns ` +
        `and does the same at load, and interface calls throw InterfaceUnavailable until each producer is Active`,
    );
    for (const id of remaining) order.push(id);
  }

  return order;
}

/**
 * The graph nodes for a set of workspace plugins. `optionalPluginDependencies` are deliberately
 * NOT read: an optional dep imposes no edge (loader.rs), because a consumer that resolves it to
 * null is designed to keep running.
 */
export function graphNodes(plugins: WorkspacePlugin[]): GraphNode[] {
  return plugins.map((p) => ({
    id: p.name,
    dependsOn: Object.keys(p.pkg.s2script?.pluginDependencies ?? {}),
    publishes: Object.keys(publishedInterfaces(p)),
  }));
}

/** `{interface: version}` this plugin publishes, expanded from the authored `publishes` grammar. */
export function publishedInterfaces(plugin: WorkspacePlugin): Record<string, string> {
  return expandPublishes(plugin.pkg.s2script?.publishes, plugin.name, plugin.version);
}

/** The plugins in dependency order (producers first), stable lexicographic tie-break. */
export function orderPlugins(
  plugins: WorkspacePlugin[],
  warn: (msg: string) => void = console.warn,
): WorkspacePlugin[] {
  const byId = new Map(plugins.map((p) => [p.name, p]));
  return topoOrder(graphNodes(plugins), warn)
    .map((id) => byId.get(id))
    .filter((p): p is WorkspacePlugin => p !== undefined);
}

// ---------------------------------------------------------------------------
// §5.2 — the sibling range gate
// ---------------------------------------------------------------------------

/**
 * `version` satisfies `range` under real semver. A malformed range is NOT an exception here —
 * `semver.satisfies` returns false — which is the fail-closed answer we want: an unparseable
 * range is reported as a violation naming both sides, not as a stack trace.
 *
 * Note this is STRICTER than the engine's `interfaces.rs::version_satisfies`, which is
 * major-only. Being stricter at build time is safe: anything this accepts, the loader accepts.
 */
export function satisfiesRange(version: string, range: string): boolean {
  return satisfies(version, range, { includePrerelease: true });
}

/** One `pluginDependencies` range that the workspace's own producer does not satisfy. */
export interface RangeViolation {
  /** The consuming plugin's package name. */
  consumer: string;
  /** The interface named in `pluginDependencies`. */
  iface: string;
  /** The declared range. */
  range: string;
  /** The sibling plugin publishing `iface`. */
  producer: string;
  /** The version that sibling publishes `iface` at. */
  version: string;
}

/**
 * Every sibling range in the workspace that does not match what the workspace actually ships.
 * This is what makes a monorepo honest: you cannot ship B declaring `^2.0.0` against an A you
 * are shipping alongside it at 3.0.0.
 *
 * The comparison is against the PUBLISHED INTERFACE version, not the producer's package version.
 * They are the same thing for `publishes: "self"` (the dominant case the spec's example shows),
 * but the map form decouples them — @edge/mce@3.1.0 may publish @community/mapchooser@1.2.0 —
 * and the interface version is what `interfaces.rs::call_target_inner` actually range-checks at
 * runtime. Gating on anything else would flag builds the engine would happily load, and pass
 * builds it would refuse.
 *
 * A dependency naming an interface no sibling publishes is NOT a violation: it is a registry
 * dependency, and today's behaviour for those is unchanged (§11).
 */
export function checkSiblingRanges(plugins: WorkspacePlugin[]): RangeViolation[] {
  // interface name -> (producer package, published version).
  const published = new Map<string, { producer: string; version: string }>();
  for (const p of plugins) {
    for (const [iface, version] of Object.entries(publishedInterfaces(p))) {
      published.set(iface, { producer: p.name, version });
    }
  }

  const out: RangeViolation[] = [];
  for (const p of plugins) {
    for (const [iface, range] of Object.entries(p.pkg.s2script?.pluginDependencies ?? {})) {
      const producer = published.get(iface);
      if (producer === undefined) continue; // resolved from the registry, not from here
      if (producer.producer === p.name) continue; // its own interface
      // A non-concrete published version cannot be range-checked. `derivePublishes` already fails
      // the producer's own build for that, so reporting it again here would be a second, worse
      // diagnostic pointing at the wrong plugin.
      if (!isConcreteVersion(producer.version)) continue;
      if (satisfiesRange(producer.version, range)) continue;
      out.push({ consumer: p.name, iface, range, producer: producer.producer, version: producer.version });
    }
  }
  return out;
}

/** The §5.2 report: every violation at once, aligned, never just the first. */
export function formatRangeViolations(violations: RangeViolation[]): string {
  const width = Math.max(...violations.map((v) => v.consumer.length), 0);
  const lines = violations.map(
    (v) =>
      `  ${v.consumer.padEnd(width)}  pluginDependencies[${JSON.stringify(v.iface)}] = ` +
      `${JSON.stringify(v.range)}  but ${v.producer} is ${v.version}`,
  );
  return (
    `${violations.length} dependency range${violations.length === 1 ? " does" : "s do"} not match this workspace:\n` +
    lines.join("\n")
  );
}

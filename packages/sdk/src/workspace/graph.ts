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

import { expandPublishes, isConcreteVersion } from "../publishes.ts";
import { indexInterfaces } from "./interfaces.ts";
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
  // Through the shared index (§5.3.0): the names this ordering treats as "published" must be the
  // exact names `siblings.ts` resolves against, and a malformed `publishes` grammar must not throw
  // an unattributed error out of the middle of an ordering pass — preflight names its author.
  const { byPlugin } = indexInterfaces(plugins);
  return plugins.map((p) => ({
    id: p.name,
    dependsOn: Object.keys(p.pkg.s2script?.pluginDependencies ?? {}),
    publishes: byPlugin.get(p) ?? [],
  }));
}

/**
 * `{interface: version}` ONE plugin publishes, expanded from the authored `publishes` grammar.
 * THROWS on a malformed grammar, so anything walking a whole workspace wants `indexInterfaces`
 * instead — it attributes that error to its author rather than aborting the batch (§11).
 */
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
 * Parse the leading semver major out of a version or a range operator (`^1.2.3`, `1.x`, `~1.0`,
 * `>=1.0.0` → 1). A deliberate PORT of `interfaces.rs::leading_major`, character for character:
 * skip everything before the first digit, then take the digit run.
 */
function leadingMajor(s: string): number | null {
  const digits = /^\d+/.exec(s.replace(/^[^0-9]*/, ""));
  if (digits === null) return null;
  const n = Number.parseInt(digits[0], 10);
  // The Rust parses into a `u32` and returns `None` on overflow, so a major at or above 2^32 is
  // "unparseable" to the engine and satisfies nothing. `parseInt` would happily return it (and
  // beyond 2^53 would silently round two distinct majors together), making the SDK accept a pair
  // the engine rejects — the dangerous direction for decision #11's both-ways agreement.
  return n > 0xffffffff ? null : n;
}

/**
 * `version` satisfies `range` **as the engine decides it** — a PORT of
 * `core/src/interfaces.rs::version_satisfies`, which is what actually range-checks every
 * inter-plugin call at runtime:
 *
 * ```rust
 * pub fn version_satisfies(range: &str, version: &str) -> bool {
 *     if range.trim() == "*" { return true; }
 *     match (leading_major(range), leading_major(version)) {
 *         (Some(ra), Some(va)) => ra == va,
 *         _ => false,
 *     }
 * }
 * ```
 *
 * Decision #11: the SDK's range check must AGREE with the engine, in both directions. Full semver
 * is the wrong tool here and the disagreement runs both ways — `satisfies("3.0.0", ">=1.0.0")` is
 * true, but the engine computes `1 != 3` and throws on every call, so a full-semver gate would
 * wave through a workspace the engine refuses; and `satisfies("1.1.0", "^1.2.0")` is false while
 * the engine loads it happily, so it would also fail builds that work. A gate that disagrees with
 * the runtime it gates is worse than no gate.
 *
 * A malformed range has no leading major, so it is `false` — the fail-closed answer we want: an
 * unparseable range is reported as a violation naming both sides, not as a stack trace.
 */
export function satisfiesRange(version: string, range: string): boolean {
  if (range.trim() === "*") return true;
  const ra = leadingMajor(range);
  const va = leadingMajor(version);
  return ra !== null && va !== null && ra === va;
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
  // The SHARED index (§5.3.0): this gate and `siblings.ts` must agree on which dependency names
  // the workspace owns. They used to compute membership separately — one on the interface name,
  // one on the package name — so the two halves of the feature disagreed about the same edge.
  const { producers } = indexInterfaces(plugins);

  const out: RangeViolation[] = [];
  for (const p of plugins) {
    for (const [iface, range] of Object.entries(p.pkg.s2script?.pluginDependencies ?? {})) {
      const producer = producers.get(iface);
      if (producer === undefined) continue; // resolved from the registry, not from here
      if (producer.plugin === p) continue; // its own interface
      // A non-concrete published version cannot be range-checked. `derivePublishes` already fails
      // the producer's own build for that, so reporting it again here would be a second, worse
      // diagnostic pointing at the wrong plugin.
      if (!isConcreteVersion(producer.version)) continue;
      if (satisfiesRange(producer.version, range)) continue;
      out.push({
        consumer: p.name,
        iface,
        range,
        producer: producer.plugin.name,
        version: producer.version,
      });
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

/**
 * `s2s build` in workspace mode (design spec 2026-07-27 §5.1, §5.4).
 *
 * The flow is the spec's, in its order: stamp versions if asked, preflight the WHOLE workspace
 * before building anything, order the plugins producers-first, then build each one through
 * `buildPackage()` (dispatches plugin vs. library by `s2script.kind`). Same code path, same
 * output location (`<plugin>/dist/<sanitized-id>.s2sp`) — a workspace build and a per-plugin
 * build produce the same artifact in the same place, which is what lets `package-release.sh`
 * keep globbing `plugins/<name>/dist/<id>.s2sp` unchanged.
 *
 * A workspace member declaring `s2script.kind === "library"` lands in `ws.libs`, not `ws.plugins`
 * — it is a STRUCTURAL notion ("an npm workspace member that is not a declared plugin"), and only
 * some of those members are libraries at all. `selectPlugins`/`orderPlugins`/`graphNodes` only
 * ever see `ws.plugins`, so a declared library would otherwise never be built — it need not be
 * listed under a glob named "plugins" to get one. It is built to its own `.s2lib` BEFORE the
 * plugin loop below, for a sensible log only: a workspace sibling is resolved from SOURCE by a
 * consumer's own build (Task 6, `libraries.ts`), never from this artifact, so there is no
 * dependency edge between a library's build and its consumers' — hence no graph participation
 * and no filtering by `--filter` (which narrows *plugin* selection).
 *
 * The behaviour worth stating twice: this is COLLECT-ALL, not fail-fast (§5.1 step 4). Every
 * selected plugin (and every declared library) is attempted even after one fails, the summary
 * names each failure, and the caller exits non-zero if any failed. That is the standing *degrade
 * per-descriptor, never crash globally* rule applied to a build — one broken plugin must not hide
 * the other seventeen.
 *
 * Ordering is NOT a build dependency (§5.1): a producer's `api.d.ts` is authored, not generated,
 * so a consumer does not need its producer built first. Topological order buys deterministic
 * output and cycle detection, nothing more — which is exactly why a failed producer does not
 * skip its consumers here.
 */

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildPackage } from "../build.ts";
import { isConcreteVersion } from "../publishes.ts";
import { matchGlob } from "./glob.ts";
import { orderPlugins, satisfiesRange } from "./graph.ts";
import { indexInterfaces } from "./interfaces.ts";
import { preflightWorkspace } from "./preflight.ts";
import { loadWorkspace } from "./workspace.ts";
import type { Workspace, WorkspaceMember, WorkspacePlugin } from "./workspace.ts";

// ---------------------------------------------------------------------------
// §5.4 — `--filter`
// ---------------------------------------------------------------------------

/**
 * Collect every `--filter <pattern>` / `--filter=<pattern>` from argv, in order.
 *
 * Lives here rather than in `cli/args.ts` because `s2s deploy` narrows its build AND its publish
 * set with the same flag (§6.1) — one parser, so the two commands cannot drift on what a filter
 * means. A `--filter` with no pattern is a named refusal, never a silently ignored flag.
 */
export function collectFilters(argv: readonly string[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    if (arg.startsWith("--filter=")) {
      const value = arg.slice("--filter=".length);
      if (value === "") throw new Error(`--filter= requires a pattern (e.g. --filter=@me/shop)`);
      out.push(value);
      continue;
    }
    if (arg !== "--filter") continue;
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("-")) {
      throw new Error(`--filter requires a pattern (e.g. --filter @me/shop or --filter "plugins/sh*")`);
    }
    out.push(value);
    i++;
  }
  return out;
}

/**
 * The plugins `filters` selects, in workspace (directory) order. No filters ⇒ every plugin.
 *
 * A pattern is matched against the PACKAGE NAME first (`@me/shop`, `@me/*`), because that is what
 * an author says out loud. A pattern containing `/` that matches no package name is retried as a
 * path glob relative to the workspace root (`plugins/sh*`) — the `/` is the discriminator, so a
 * bare `sh*` is never silently reinterpreted as a path.
 *
 * A pattern matching nothing at either form is an ERROR, not an empty build (§5.4): a typo in a
 * release script must fail loudly rather than quietly ship zero plugins. Every unmatched pattern
 * is named at once, per the standing report-all rule.
 */
export function selectPlugins(ws: Workspace, filters: readonly string[]): WorkspacePlugin[] {
  if (filters.length === 0) return ws.plugins;

  const chosen = new Set<string>();
  const unmatched: string[] = [];
  for (const pattern of filters) {
    let hits = ws.plugins.filter((p) => matchGlob(pattern, p.name));
    if (hits.length === 0 && pattern.includes("/")) {
      hits = ws.plugins.filter((p) => matchGlob(pattern, p.relDir));
    }
    if (hits.length === 0) {
      unmatched.push(pattern);
      continue;
    }
    for (const p of hits) chosen.add(p.name);
  }

  if (unmatched.length > 0) {
    const known = ws.plugins.map((p) => `${p.name} (${p.relDir})`);
    throw new Error(
      `--filter matched no plugin: ${unmatched.map((p) => JSON.stringify(p)).join(", ")}\n` +
        `  a filter matches a package name (@me/shop, @me/*) or, when it contains "/", a path ` +
        `relative to the workspace root (plugins/sh*)\n` +
        `  this workspace has ${known.length} plugin${known.length === 1 ? "" : "s"}: ${known.join(", ")}`,
    );
  }
  return ws.plugins.filter((p) => chosen.has(p.name));
}

// ---------------------------------------------------------------------------
// §5.4 — `--stamp-version`
// ---------------------------------------------------------------------------

/** One `pluginDependencies` range rewritten so it still matches the stamped producer. */
export interface RangeRewrite {
  consumer: string;
  /** `pluginDependencies` or `optionalPluginDependencies`. */
  field: string;
  iface: string;
  from: string;
  to: string;
}

export interface StampResult {
  /** The normalized version actually written (a leading `v` from a release tag is stripped). */
  version: string;
  /** Package names whose `version` changed. A plugin already at `version` is left untouched. */
  stamped: string[];
  rewrites: RangeRewrite[];
}

/**
 * Keep the author's comparator and swap in the stamped version: `^2.0.0` → `^1.5.0`, `2.0.0` →
 * `1.5.0`. Anything else — a compound range (`>=1 <2`, `1.x || 2.x`), an `x`-range, a bare `>`
 * that the stamped version would sit on the wrong side of — becomes a caret range, the form the
 * SDK's own scaffolding writes.
 *
 * The result is re-checked against the version before it is returned: the entire point of the
 * rewrite is that the §5.2 gate passes afterwards, so a clever comparator-preserving rewrite that
 * did not actually satisfy would be worse than no rewrite at all.
 */
function restampRange(range: string, version: string): string {
  const m = /^\s*(\^|~|>=|=)?\s*(\d+\.\d+\.\d+[0-9A-Za-z.+-]*)\s*$/.exec(range);
  const rewritten = m ? `${m[1] ?? ""}${version}` : `^${version}`;
  return satisfiesRange(version, rewritten) ? rewritten : `^${version}`;
}

/**
 * Rewrite every plugin's `version` to `version` AND every sibling range that the stamp would
 * otherwise invalidate (§5.4). Returns what changed; writes package.json files in place.
 *
 * The range rewrite is not optional garnish: stamping all plugins to 1.5.0 while leaving a
 * consumer's `"^2.0.0"` alone would trip the §5.2 gate on every consumer, so `--stamp-version`
 * without it could never build a workspace that had ever released twice.
 *
 * Three deliberate calls:
 *
 *   - **Every plugin is stamped, never just the `--filter` set.** Stamping a subset is precisely
 *     what would leave the consumers outside the filter declaring ranges against a producer that
 *     just moved.
 *   - **`optionalPluginDependencies` ranges are rewritten too**, though §5.2 gates only the hard
 *     ones. An optional dep resolves to `Interface | null` and is still range-checked at call
 *     time, so a range left pointing at a version the workspace no longer ships would silently
 *     null out a producer sitting right there on disk — a worse outcome than the build error the
 *     hard case would have produced.
 *   - **A range is rewritten only when the stamped version no longer satisfies it.** The invariant
 *     that matters is "the §5.2 gate passes after stamping"; a consumer declaring `^1.0.0` against
 *     a producer stamped to 1.5.0 is still true, and narrowing it to `^1.5.0` would churn a
 *     checked-in file for nothing. "Satisfies" is `satisfiesRange`, which is the ENGINE's rule
 *     (decision #11) — so a comparator range the engine rejects, like `>=1.0.0` against a stamp of
 *     3.0.0, IS rewritten. Under the old full-semver check that range read as satisfied, and the
 *     stamp left behind a range every call would then have thrown on. The comparison is against
 *     the PUBLISHED INTERFACE version (the thing `interfaces.rs` range-checks at runtime), so a
 *     `publishes` map form pinning an explicit contract version is correctly left alone — its
 *     interface did not move just because its package did.
 *
 * The in-memory `Workspace` is NOT mutated: package.json is the authority and callers reload from
 * disk (see `buildWorkspace`), so there is never a half-updated model to reason about.
 */
export function stampWorkspaceVersion(ws: Workspace, version: string): StampResult {
  // Release tags carry the `v`; build-base-plugins.sh has always stripped it before stamping.
  const target = version.trim().replace(/^v/, "");
  if (!isConcreteVersion(target)) {
    throw new Error(
      `--stamp-version needs a concrete version like 1.5.0 (got ${JSON.stringify(version)}) — ` +
        `it stamps every plugin in the workspace, and a manifest carries a version, never a range`,
    );
  }

  // What each interface WOULD be published at once the stamp lands. The index reads each plugin's
  // version, so ask it about the stamped copies rather than the on-disk ones. Its `problems` are
  // deliberately ignored here: a malformed `publishes` block is preflight's to name (§11), and
  // failing the stamp on it would report it against the wrong command.
  const after = indexInterfaces(ws.plugins.map((p) => ({ ...p, version: target }))).producers;

  const result: StampResult = { version: target, stamped: [], rewrites: [] };
  for (const p of ws.plugins) {
    // A structural clone so the write is the only side effect — see the doc comment.
    const next = structuredClone(p.pkg);
    let dirty = false;
    if (next.version !== target) {
      next.version = target;
      result.stamped.push(p.name);
      dirty = true;
    }

    for (const field of ["pluginDependencies", "optionalPluginDependencies"] as const) {
      const ranges = next.s2script?.[field] as Record<string, string> | undefined;
      if (ranges === undefined) continue;
      for (const [iface, range] of Object.entries(ranges)) {
        const producer = after.get(iface);
        if (producer === undefined) continue; // resolved from the registry, not from here
        // By name, not by identity: `after` was built from stamped CLONES. `loadWorkspace` has
        // already refused a workspace with two plugins sharing a name, so a name is an identity.
        if (producer.plugin.name === p.name) continue; // its own interface
        if (!isConcreteVersion(producer.version)) continue; // not range-checkable; §5.2 says the same
        if (satisfiesRange(producer.version, range)) continue;
        const to = restampRange(range, producer.version);
        ranges[iface] = to;
        result.rewrites.push({ consumer: p.name, field, iface, from: range, to });
        dirty = true;
      }
    }

    // Same serialization build-base-plugins.sh has always used, so the release path's diff shape
    // is unchanged by moving the stamp into the CLI.
    if (dirty) writeFileSync(join(p.dir, "package.json"), `${JSON.stringify(next, null, 2)}\n`);
  }
  return result;
}

/** The stamp report: what moved, and every range that moved with it. */
export function formatStampResult(stamp: StampResult): string {
  const lines = [`stamped ${stamp.stamped.length} plugin version(s) → ${stamp.version}`];
  for (const r of stamp.rewrites) {
    lines.push(
      `  ${r.consumer} ${r.field}[${JSON.stringify(r.iface)}] ${JSON.stringify(r.from)} → ${JSON.stringify(r.to)}`,
    );
  }
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// §5.1 — build every plugin, collect every failure
// ---------------------------------------------------------------------------

/**
 * Wraps one plugin's build so a UI layer can render progress without this module importing one.
 * The interactive implementation is `ui.task`; the non-interactive one prints the artifact path,
 * keeping stdout machine-readable exactly as single-plugin mode does.
 */
export type StepRunner = (
  label: string,
  fn: () => Promise<string>,
  done: (outPath: string) => string,
) => Promise<string>;

/** One plugin's (or declared library's) build result. Exactly one of `outPath` / `error` is set. */
export interface PluginBuildOutcome {
  name: string;
  version: string;
  relDir: string;
  outPath?: string;
  error?: string;
}

export interface BuildWorkspaceOptions {
  workspace: Workspace;
  /** Where the @s2script/* stubs live; resolved once for the whole workspace by the caller. */
  packagesDir?: string;
  filters?: readonly string[];
  stampVersion?: string;
  step?: StepRunner;
  warn?: (msg: string) => void;
  /** Called once after preflight with the final build order — the caller's chance to print a plan. */
  onPlan?: (plugins: readonly WorkspacePlugin[], stamp?: StampResult) => void;
}

export interface BuildWorkspaceResult {
  root: string;
  stamp?: StampResult;
  /** Every attempted plugin AND declared library, libraries first, each in build order. */
  outcomes: PluginBuildOutcome[];
  built: PluginBuildOutcome[];
  failed: PluginBuildOutcome[];
}

/** `ws.libs` members that opted in with `s2script.kind === "library"` — the buildable subset of
 *  a structural notion ("an npm workspace member that is not a declared plugin") that otherwise
 *  says nothing about whether a member is a library at all (a plain shared-code package is not). */
function declaredLibraries(ws: Workspace): WorkspaceMember[] {
  return ws.libs.filter((m) => m.pkg.s2script?.kind === "library");
}

export async function buildWorkspace(opts: BuildWorkspaceOptions): Promise<BuildWorkspaceResult> {
  const warn = opts.warn ?? console.warn;
  const step: StepRunner = opts.step ?? ((_label, fn) => fn());

  let ws = opts.workspace;
  let stamp: StampResult | undefined;
  if (opts.stampVersion !== undefined) {
    stamp = stampWorkspaceVersion(ws, opts.stampVersion);
    // Re-read rather than patch the model: the range gate and the graph below must see exactly
    // the bytes `buildPackage()` is about to read.
    ws = loadWorkspace(ws.root);
  }

  // A workspace whose globs match nothing is a misconfiguration, not a zero-plugin success —
  // build-base-plugins.sh has always failed with "no base plugins found" for the same reason.
  if (ws.plugins.length === 0) {
    throw new Error(
      `workspace at ${ws.root} contains no plugins — s2script.workspace.plugins ` +
        `(${JSON.stringify(ws.pkg.s2script?.workspace?.plugins ?? [])}) matched no directory with a package.json`,
    );
  }

  const selected = selectPlugins(ws, opts.filters ?? []);

  // §5.1 step 2: preflight the ENTIRE workspace before building anything, even under --filter.
  // A range violation is a statement about the plugin set as a whole; narrowing the build set
  // does not make it any less wrong (see preflight.ts).
  preflightWorkspace(ws);

  const ordered = orderPlugins(selected, warn);
  opts.onPlan?.(ordered, stamp);

  const outcomes: PluginBuildOutcome[] = [];

  // Libraries first, plugins after — log ordering only (see the module doc comment): a workspace
  // sibling is resolved from SOURCE by the consumer's own build, never from this .s2lib, so there
  // is no dependency edge to respect either way. Every declared library is attempted regardless of
  // `--filter`, which narrows *plugin* selection (`selectPlugins` reads `ws.plugins` only) — a
  // library is workspace-wide supporting output, not one of the things being filtered.
  for (const lib of declaredLibraries(ws)) {
    const base = { name: lib.name, version: lib.version, relDir: lib.relDir };
    try {
      const outPath = await step(
        `Building ${lib.name}`,
        () => buildPackage(lib.dir, opts.packagesDir),
        (out) => `Built ${out}`,
      );
      outcomes.push({ ...base, outPath });
    } catch (e) {
      outcomes.push({ ...base, error: e instanceof Error ? e.message : String(e) });
    }
  }

  for (const p of ordered) {
    const base = { name: p.name, version: p.version, relDir: p.relDir };
    try {
      const outPath = await step(
        `Building ${p.name}`,
        () => buildPackage(p.dir, opts.packagesDir),
        (out) => `Built ${out}`,
      );
      outcomes.push({ ...base, outPath });
    } catch (e) {
      // Collect-all (§5.1 step 4): record and keep going. The caller names every failure at the
      // end and exits non-zero.
      outcomes.push({ ...base, error: e instanceof Error ? e.message : String(e) });
    }
  }

  return {
    root: ws.root,
    stamp,
    outcomes,
    built: outcomes.filter((o) => o.outPath !== undefined),
    failed: outcomes.filter((o) => o.error !== undefined),
  };
}

/** The collect-all summary: every failure named, with its full message, indented under it. */
export function formatBuildFailures(failed: readonly PluginBuildOutcome[], total: number): string {
  const head = `${failed.length} of ${total} plugin${total === 1 ? "" : "s"} failed to build:`;
  const blocks = failed.map((f) => {
    const body = (f.error ?? "")
      .split("\n")
      .map((l) => `      ${l}`)
      .join("\n");
    return `\n  ${f.name} (${f.relDir}):\n${body}`;
  });
  return head + blocks.join("");
}

/**
 * §7 step 5 — the one pass changesets cannot do for us: rewriting
 * `s2script.pluginDependencies` ranges (design spec 2026-07-27).
 *
 * `applyReleasePlan` rewrites internal ranges wherever it finds them in npm's four dependency
 * fields. It finds no sibling edge there (the mirror lives only in memory, §7 step 2), so it
 * correctly no-ops — and the range that actually matters, the one the loader range-checks at
 * runtime, is still the old one. This module closes that gap: `@me/a` 2.0.1 -> 3.0.0 turns a
 * consumer's `"@me/a": "^2.0.0"` into `"^3.0.0"`.
 *
 * The update decision is a port of `apply-release-plan`'s own
 * `shouldUpdateDependencyBasedOnConfig`, so the same `updateInternalDependencies` setting governs
 * both fields. Diverging here would mean one repo setting produced two behaviours depending on
 * which file the dependency happened to be declared in.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { satisfies } from "semver";
import { isConcreteVersion } from "../publishes.ts";
import type { Workspace } from "../workspace/workspace.ts";
import { siblingEdges } from "./mirror.ts";
import type { BumpType, ChangesetsConfig, ComprehensiveRelease } from "./changesets.ts";

/** One `pluginDependencies` range this pass rewrote. */
export interface RangeRewrite {
  consumer: string;
  producer: string;
  iface: string;
  from: string;
  to: string;
}

/** One range left alone, with the reason — a silent no-op here is a §5.2 build failure later. */
export interface RangeSkip {
  consumer: string;
  producer: string;
  iface: string;
  range: string;
  reason: string;
}

export interface RangeRewriteResult {
  rewrites: RangeRewrite[];
  skips: RangeSkip[];
  /** Absolute paths of the package.json files this pass wrote. */
  touchedFiles: string[];
}

const BUMP_LEVEL: Record<BumpType, number> = { none: 0, patch: 1, minor: 2, major: 3 };

/**
 * The range's leading operator, byte-for-byte what `@changesets/get-version-range-type` returns.
 * Reimplemented rather than imported: it is six lines, and every import of a changesets internal
 * is another thing the §7 version gate has to keep honest.
 */
function rangePrefix(range: string): string {
  if (range.startsWith(">=")) return ">=";
  if (range.startsWith("<=")) return "<=";
  const first = range.charAt(0);
  if (first === "^" || first === "~" || first === ">") return first;
  return "";
}

/** `*`, `x`, `X` and the empty range all mean "any version" — changesets deliberately leaves
 *  those as authored, on the reasoning that whoever wrote one meant it. */
function isWildcardRange(range: string): boolean {
  const t = range.trim();
  return t === "" || t === "*" || t === "x" || t === "X";
}

/**
 * Port of `apply-release-plan`'s `shouldUpdateDependencyBasedOnConfig`, minus the `workspace:` and
 * peer-dependency branches (§14 rules out workspace protocols, and a `pluginDependency` is never a
 * peer dep). A dependency leaving its range is ALWAYS updated; one still inside it is updated only
 * when the bump reaches the `updateInternalDependencies` threshold.
 */
function shouldUpdate(release: ComprehensiveRelease, range: string, config: ChangesetsConfig): boolean {
  if (!satisfies(release.newVersion, range, { includePrerelease: true })) return true;
  const minLevel = BUMP_LEVEL[config.updateInternalDependencies] ?? BUMP_LEVEL.patch;
  return BUMP_LEVEL[release.type] >= minLevel;
}

/** Read a package.json back off disk, keeping enough of its formatting to rewrite it in place. */
function readJsonFile(path: string): { json: Record<string, unknown>; indent: string; eol: string } {
  const raw = readFileSync(path, "utf8");
  const match = /^[ \t]*[{[]\s*\n([ \t]+)/.exec(raw);
  return {
    json: JSON.parse(raw) as Record<string, unknown>,
    indent: match?.[1] ?? "  ",
    eol: raw.endsWith("\n") ? "\n" : "",
  };
}

/**
 * Rewrite every sibling `pluginDependencies` range the plan's releases invalidate.
 *
 * Runs AFTER `applyReleasePlan`, and re-reads each package.json from disk rather than reusing the
 * in-memory objects: `applyReleasePlan` has already written those files, and re-serialising a stale
 * copy would silently undo the version it just wrote.
 */
export function rewriteSiblingRanges(
  ws: Workspace,
  releases: ComprehensiveRelease[],
  config: ChangesetsConfig,
): RangeRewriteResult {
  const releaseOf = new Map(releases.map((r) => [r.name, r]));
  const rewrites: RangeRewrite[] = [];
  const skips: RangeSkip[] = [];

  // consumer package name -> the edits to apply to its package.json, keyed by interface name.
  const edits = new Map<string, Map<string, string>>();

  for (const edge of siblingEdges(ws)) {
    const release = releaseOf.get(edge.producer);
    if (release === undefined) continue; // the producer is not in this release
    if (release.newVersion === release.oldVersion) continue; // a `type: "none"` carry-along
    if (!shouldUpdate(release, edge.range, config)) continue;

    const skip = (reason: string): void => {
      skips.push({ consumer: edge.consumer, producer: edge.producer, iface: edge.iface, range: edge.range, reason });
    };
    if (isWildcardRange(edge.range)) {
      skip("range matches any version — left exactly as authored");
      continue;
    }
    const prefix = rangePrefix(edge.range);
    if (!isConcreteVersion(edge.range.slice(prefix.length))) {
      // A compound range (`>=1.0.0 <2.0.0`) or a partial one (`1.x`) cannot be rebuilt without
      // guessing what the author meant. Reported, never silently rewritten — and if the result is
      // genuinely out of range the §5.2 preflight gate fails the next build by name.
      skip(`not a simple <operator><version> range — rewrite it by hand to track ${release.newVersion}`);
      continue;
    }
    const to = `${prefix}${release.newVersion}`;
    if (to === edge.range) continue;

    rewrites.push({ consumer: edge.consumer, producer: edge.producer, iface: edge.iface, from: edge.range, to });
    const forConsumer = edits.get(edge.consumer) ?? new Map<string, string>();
    forConsumer.set(edge.iface, to);
    edits.set(edge.consumer, forConsumer);
  }

  const touchedFiles: string[] = [];
  for (const [consumerName, ifaceRanges] of edits) {
    const plugin = ws.plugins.find((p) => p.name === consumerName);
    if (plugin === undefined) continue;
    const path = join(plugin.dir, "package.json");
    const { json, indent, eol } = readJsonFile(path);
    const s2 = json.s2script as { pluginDependencies?: Record<string, string> } | undefined;
    if (s2?.pluginDependencies === undefined) continue; // it was there a moment ago; nothing to do
    for (const [iface, range] of ifaceRanges) s2.pluginDependencies[iface] = range;
    writeFileSync(path, JSON.stringify(json, null, indent) + eol);
    touchedFiles.push(path);
  }

  return { rewrites, skips, touchedFiles };
}

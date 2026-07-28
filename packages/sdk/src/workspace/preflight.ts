/**
 * Workspace preflight (design spec 2026-07-27 §5.1 step 2, §5.2).
 *
 * The whole workspace is validated BEFORE anything is built, so a workspace that cannot ship
 * coherently never produces a single `.s2sp`. Two of §11's three preflight classes — a plugin
 * with no entry point, a plugin directory npm `workspaces` does not cover — are already raised
 * (aggregated) by `loadWorkspace`, because they are properties of the workspace's SHAPE. What is
 * left, and what lives here, is the workspace's CONTENT: what each plugin publishes (§5.3.0) and
 * whether the ranges its consumers declare match it.
 *
 * The gate always runs over the entire workspace, even under `--filter`. A range violation is a
 * statement about the plugin set as a whole — B declaring `^2.0.0` against an A you are shipping
 * alongside it at 3.0.0 — and narrowing the build set does not make that any less wrong.
 */

import { checkSiblingRanges, formatRangeViolations } from "./graph.ts";
import type { RangeViolation } from "./graph.ts";
import { indexInterfaces } from "./interfaces.ts";
import type { Workspace } from "./workspace.ts";

/** Every preflight problem in `ws`, as report-ready text. Empty when the workspace is coherent. */
export function preflightProblems(ws: Workspace): string[] {
  // The `publishes` blocks first, because the range gate reads them: a malformed grammar or an
  // interface two plugins both publish is attributed to its author here (§11) rather than thrown
  // out of the middle of the gate, where `expandPublishes`'s message named no plugin, no directory
  // and no package while aborting all eighteen builds.
  const problems = [...indexInterfaces(ws.plugins).problems];
  const violations: RangeViolation[] = checkSiblingRanges(ws.plugins);
  if (violations.length > 0) problems.push(formatRangeViolations(violations));
  return problems;
}

/**
 * Throw unless `ws` is coherent, naming EVERY violation at once (§5.2) rather than the first.
 * Call this before building anything.
 */
export function preflightWorkspace(ws: Workspace): void {
  const problems = preflightProblems(ws);
  if (problems.length > 0) throw new Error(problems.join("\n"));
}

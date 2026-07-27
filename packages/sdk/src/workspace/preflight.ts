/**
 * Workspace preflight (design spec 2026-07-27 §5.1 step 2, §5.2).
 *
 * The whole workspace is validated BEFORE anything is built, so a workspace that cannot ship
 * coherently never produces a single `.s2sp`. Two of §11's three preflight classes — a plugin
 * with no entry point, a plugin directory npm `workspaces` does not cover — are already raised
 * (aggregated) by `loadWorkspace`, because they are properties of the workspace's SHAPE. What is
 * left, and what lives here, is the range gate: a property of the workspace's CONTENT.
 *
 * The gate always runs over the entire workspace, even under `--filter`. A range violation is a
 * statement about the plugin set as a whole — B declaring `^2.0.0` against an A you are shipping
 * alongside it at 3.0.0 — and narrowing the build set does not make that any less wrong.
 */

import { checkSiblingRanges, formatRangeViolations } from "./graph.ts";
import type { RangeViolation } from "./graph.ts";
import type { Workspace } from "./workspace.ts";

/** Every preflight problem in `ws`, as report-ready text. Empty when the workspace is coherent. */
export function preflightProblems(ws: Workspace): string[] {
  const violations: RangeViolation[] = checkSiblingRanges(ws.plugins);
  return violations.length === 0 ? [] : [formatRangeViolations(violations)];
}

/**
 * Throw unless `ws` is coherent, naming EVERY violation at once (§5.2) rather than the first.
 * Call this before building anything.
 */
export function preflightWorkspace(ws: Workspace): void {
  const problems = preflightProblems(ws);
  if (problems.length > 0) throw new Error(problems.join("\n"));
}

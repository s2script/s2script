/**
 * `s2s deploy` in workspace mode (design spec 2026-07-27 §6): a per-plugin PUBLISH plan and its
 * upload, built on top of `build-all.ts`'s existing §5.1 machinery rather than a second copy of
 * it. §6.1 step 1 ("build everything first and require all green") IS `build-all.ts`'s
 * `buildWorkspace` — same filter/order/preflight/collect-all — reused verbatim so build and
 * deploy can never select or order a different plugin set from the same `--filter`. This module
 * owns only what is new here: the plan (§6.1 step 2, §6.2) and the upload (§6.1 step 4).
 */

import { RegistryClient, RegistryError } from "../registry/client.ts";
import { assembleDeployArchive } from "../registry/deploy.ts";
import type { DeployablePkgJson } from "../registry/deploy.ts";
import type { PluginBuildOutcome } from "./build-all.ts";
import type { Workspace, WorkspacePlugin } from "./workspace.ts";

// ---------------------------------------------------------------------------
// Recovering plugin objects from a build-all.ts result
// ---------------------------------------------------------------------------

export interface BuiltPlugin {
  plugin: WorkspacePlugin;
  outPath: string;
}

/**
 * `buildWorkspace`'s outcomes are already in dependency (build) order and already name-keyed to
 * `ws.plugins` — this recovers the full `WorkspacePlugin` (dir, pkg, private) that `outcomes`'
 * slimmer `PluginBuildOutcome` does not carry, without re-deriving the order or the filtered set
 * a second time. Call this only AFTER confirming every outcome succeeded (§6.1 step 1) — that is
 * a caller invariant, not something re-checked here.
 */
export function builtPluginsFromOutcomes(
  ws: Workspace,
  outcomes: readonly PluginBuildOutcome[],
): { ordered: WorkspacePlugin[]; built: Map<string, BuiltPlugin> } {
  const byName = new Map(ws.plugins.map((p) => [p.name, p]));
  const ordered: WorkspacePlugin[] = [];
  const built = new Map<string, BuiltPlugin>();
  for (const o of outcomes) {
    const plugin = byName.get(o.name);
    if (!plugin) continue; // defensive only — every outcome comes from this exact ws.plugins
    ordered.push(plugin);
    if (o.outPath !== undefined) built.set(o.name, { plugin, outPath: o.outPath });
  }
  return { ordered, built };
}

// ---------------------------------------------------------------------------
// Plan (§6.1 step 2, §6.2)
// ---------------------------------------------------------------------------

export type PlanReason = "skip (private)" | "skip (already published)" | "PUBLISH";

export interface PlanEntry {
  plugin: WorkspacePlugin;
  reason: PlanReason;
}

/**
 * True when `name@version` is already on the registry — a COURTESY check only, for a legible
 * plan (§6.2). The registry is the real authority: any failure to confirm here (network error,
 * a "not found" thrown as an error, the registry being unreachable) degrades to "assume
 * unpublished", which turns into a PUBLISH plan entry. If that guess was wrong, the server
 * rejects the upload as a duplicate and `uploadPlan` reports THAT as a skip, not a failure — so
 * a stale or absent check here can never turn into a broken deploy, only an unnecessary upload
 * attempt.
 */
export async function isAlreadyPublished(
  client: RegistryClient,
  name: string,
  version: string,
): Promise<boolean> {
  try {
    // `range` doubles as an exact-match filter here: semver treats a bare concrete version as a
    // range only it itself satisfies, so a hit for anything OTHER than `version` cannot happen.
    const r = await client.resolve(name, version);
    return r.version === version;
  } catch {
    return false;
  }
}

/**
 * The per-plugin plan (§6.1 step 2): `skip (private)` | `skip (already published)` | `PUBLISH`.
 *
 * `plugins` should already be in upload order (topological — see `builtPluginsFromOutcomes` /
 * `orderPlugins`); the plan preserves whatever order it is given, so printing it and then
 * uploading it need no re-sort in between.
 */
export async function computePlan(
  plugins: readonly WorkspacePlugin[],
  checkPublished: (name: string, version: string) => Promise<boolean>,
): Promise<PlanEntry[]> {
  const plan: PlanEntry[] = [];
  for (const plugin of plugins) {
    if (plugin.private) {
      plan.push({ plugin, reason: "skip (private)" });
      continue;
    }
    const already = await checkPublished(plugin.name, plugin.version);
    plan.push({ plugin, reason: already ? "skip (already published)" : "PUBLISH" });
  }
  return plan;
}

/** How many entries in `plan` will actually upload — what the confirm prompt counts (§6.1 step 3). */
export function publishCount(plan: readonly PlanEntry[]): number {
  return plan.filter((e) => e.reason === "PUBLISH").length;
}

/**
 * The plan block exactly as shown in design spec §6.1's worked example — name and version each
 * column-aligned to the widest entry, reason left as-is:
 *
 *     @me/shared-api 1.2.0   skip (already published)
 *     @me/shop       2.0.1   PUBLISH
 *
 * Each returned line is already indented by 4 spaces, matching the spec's own block.
 */
export function formatPlan(plan: readonly PlanEntry[]): string {
  const nameWidth = Math.max(...plan.map((e) => e.plugin.name.length), 0);
  const versionWidth = Math.max(...plan.map((e) => e.plugin.version.length), 0);
  return plan
    .map(
      (e) =>
        `    ${e.plugin.name.padEnd(nameWidth)} ${e.plugin.version.padEnd(versionWidth)}   ${e.reason}`,
    )
    .join("\n");
}

// ---------------------------------------------------------------------------
// Upload (§6.1 step 4, §6.2)
// ---------------------------------------------------------------------------

export interface DeployResult {
  plugin: WorkspacePlugin;
  status: "published" | "skipped" | "failed";
  detail: string;
}

/**
 * HTTP 409 is exactly the registry's duplicate-version rejection (the server's `deployPackage`
 * returns `{status: 409, error: "version <name>@<version> already exists"}`) — the one error a
 * mid-fan-out upload must treat as a `skip`, not a failure (§6.2), because re-running after a
 * partial failure has to stay safe by construction.
 */
function isDuplicateVersionRejection(e: unknown): boolean {
  return e instanceof RegistryError && e.status === 409;
}

/**
 * Upload every `PUBLISH` entry of `plan`, in the ORDER `plan` is already in (topological — a
 * consumer must never go live against an absent producer, §6.1 step 4). `built` maps a plugin's
 * package name to its already-built archive (from `builtPluginsFromOutcomes`); every `PUBLISH`
 * entry MUST have one, because §6.1 step 1 builds everything before any plan is computed — a
 * mismatch here is a caller bug, not a runtime condition, so it throws rather than silently
 * skipping.
 */
export async function uploadPlan(
  plan: readonly PlanEntry[],
  built: ReadonlyMap<string, BuiltPlugin>,
  client: RegistryClient,
): Promise<DeployResult[]> {
  const results: DeployResult[] = [];
  for (const entry of plan) {
    if (entry.reason !== "PUBLISH") {
      results.push({ plugin: entry.plugin, status: "skipped", detail: entry.reason });
      continue;
    }
    const b = built.get(entry.plugin.name);
    if (!b) {
      throw new Error(
        `internal error: ${entry.plugin.name} is planned to PUBLISH but was never built — ` +
          `uploadPlan must run only against a plan computed from a successful build`,
      );
    }
    try {
      const archive = assembleDeployArchive(b.plugin.dir, b.plugin.pkg as DeployablePkgJson, b.outPath);
      const res = await client.deploy(archive);
      results.push({ plugin: entry.plugin, status: "published", detail: res.reviewState });
    } catch (e) {
      if (isDuplicateVersionRejection(e)) {
        // The registry is the authority (§6.2): the client-side "already published" check that
        // built this plan was only a courtesy, and it can be stale — someone else published
        // between plan and upload, or the check itself degraded to "assume unpublished" because
        // the registry was unreachable at plan time. Either way this is exactly what re-running
        // after a partial failure looks like, and it must stay a skip, not a failure.
        results.push({ plugin: entry.plugin, status: "skipped", detail: "skip (already published)" });
        continue;
      }
      results.push({
        plugin: entry.plugin,
        status: "failed",
        detail: e instanceof Error ? e.message : String(e),
      });
    }
  }
  return results;
}

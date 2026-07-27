/**
 * `s2s deploy` — single-plugin mode (unchanged, now closing §6.3's private-package hole) and
 * workspace mode (design spec 2026-07-27 §6): build everything first and require all green,
 * print a per-plugin plan, confirm (or `--dry-run` / `--yes` / `--ci`), then upload in
 * dependency order.
 *
 * Which mode: `s2s deploy <one-plugin-dir>` stays single-plugin EVEN INSIDE a workspace (§4.2) —
 * only when `dir` IS ITSELF the marked workspace root does this run the workspace flow. That is
 * exactly `findWorkspaceRoot(dir) === dir`, since the walk-up checks `dir` before its parents.
 *
 * §6.1 step 1 ("build everything first and require all green") reuses `workspace/build-all.ts`'s
 * `buildWorkspace` verbatim — the same filter/order/preflight/collect-all machinery `s2s build`
 * uses — rather than a second implementation of it, so `s2s build --filter X` and
 * `s2s deploy --filter X` can never select or order a different plugin set.
 */
import { resolve } from "node:path";
import { deployPlugin } from "../registry/deploy.ts";
import { RegistryClient } from "../registry/client.ts";
import { loadCredentials, defaultRegistryUrl } from "../registry/credentials.ts";
import { resolvePackagesDir } from "../packages-resolve.ts";
import { parseFlag, hasFlag, positionals } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";
import { findWorkspaceRoot, loadWorkspace } from "../workspace/workspace.ts";
import { preflightWorkspace } from "../workspace/preflight.ts";
import { buildWorkspace, collectFilters, formatBuildFailures } from "../workspace/build-all.ts";
import type { StepRunner } from "../workspace/build-all.ts";
import {
  builtPluginsFromOutcomes,
  computePlan,
  isAlreadyPublished,
  publishCount,
  formatPlan,
  uploadPlan,
} from "../workspace/deploy-all.ts";

export async function run(argv: string[]): Promise<void> {
  const yes = hasFlag(argv, "--yes") || hasFlag(argv, "-y");
  const ci = hasFlag(argv, "--ci");
  const dryRun = hasFlag(argv, "--dry-run");
  const interactive = ui.isInteractive({ yes, ci });
  const filters = collectFilters(argv);
  const pos = positionals(argv, ["--packages-dir", "--registry", "--filter"]);
  const dir = pos[0] ?? ".";

  try {
    const absDir = resolve(dir);
    const root = findWorkspaceRoot(absDir);
    if (root !== null && resolve(root) === absDir) {
      await runWorkspaceDeploy({ root: absDir, argv, interactive, filters, dryRun, yes, ci });
      return;
    }

    if (filters.length > 0) {
      // §5.4/§6.1: --filter narrows a WORKSPACE build/publish set. `dir` resolved to
      // single-plugin mode, so silently ignoring --filter here would be a silent no-op — the
      // one thing this design refuses to do (§14).
      throw new Error(
        `--filter only applies at a workspace root — ${dir} resolves to single-plugin mode`,
      );
    }

    // Single-plugin mode, unchanged: deployPlugin itself now refuses a private package (§6.3).
    const packagesDir = resolvePackagesDir({
      explicit: parseFlag(argv, "--packages-dir"),
      pluginDir: absDir,
      fromCliUrl: import.meta.url,
    });
    const result = await ui.task(
      `Deploying ${dir}`,
      () =>
        deployPlugin({
          dir,
          packagesDir,
          registryUrl: parseFlag(argv, "--registry"),
          ci,
        }),
      { interactive, done: (r) => `Deployed ${r.name}@${r.version} (${r.reviewState})` },
    );
    if (interactive) {
      if (result.reviewState === "unreviewed") {
        ui.log.warn("Not reviewed by s2script — listed with a disclaimer until approved.");
      }
    } else {
      console.log(`deployed ${result.name}@${result.version} (${result.reviewState})`);
      if (result.reviewState === "unreviewed") {
        console.log("note: Not reviewed by s2script — listed with a disclaimer until approved.");
      }
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

/**
 * The workspace flow (§6.1): build everything targeted → plan → confirm/dry-run → upload in
 * dependency order. Thrown errors bubble to `run`'s catch, which prints and exits(1) exactly as
 * the single-plugin path does — a build failure, a range violation, or an upload failure all
 * read the same way to a caller.
 */
async function runWorkspaceDeploy(opts: {
  root: string;
  argv: string[];
  interactive: boolean;
  filters: string[];
  dryRun: boolean;
  yes: boolean;
  ci: boolean;
}): Promise<void> {
  const { root, argv, interactive, filters, dryRun, yes, ci } = opts;
  const label = (msg: string): void => (interactive ? ui.log.step(msg) : console.log(msg));

  const ws = loadWorkspace(root);
  // §5.2's range gate runs over the WHOLE workspace even under --filter (preflight.ts's own
  // doc). `buildWorkspace` below runs this same check again internally; doing it here too means
  // a broken workspace is refused before we even ask for credentials.
  preflightWorkspace(ws);

  const packagesDir = resolvePackagesDir({
    explicit: parseFlag(argv, "--packages-dir"),
    // npm hoists node_modules to the workspace ROOT (mirrors s2s build's own choice).
    pluginDir: root,
    fromCliUrl: import.meta.url,
  });

  // Credentials are NOT required yet — resolve (the plan's courtesy check) needs no auth, and
  // §9.1's all-private workspace (this repo's own plugins/ tree) must be able to run this whole
  // command, end to end, as a silent no-op with no token configured at all. The login gate below
  // fires only once the plan says there is actually something to upload.
  const creds = loadCredentials();
  const client = new RegistryClient({
    baseUrl: parseFlag(argv, "--registry") || creds?.registryUrl || defaultRegistryUrl(),
    token: creds?.token,
  });

  const step: StepRunner = interactive
    ? (stepLabel, fn, done) => ui.task(stepLabel, fn, { interactive: true, done })
    : async (_stepLabel, fn) => {
        const out = await fn();
        console.log(out);
        return out;
      };

  // §6.1 step 1: build EVERYTHING targeted first, collecting every failure (never fail-fast) —
  // any failure means publish nothing, so the plan below must never be computed against a
  // partially-built set.
  const build = await buildWorkspace({
    workspace: ws,
    packagesDir,
    filters,
    step,
    warn: interactive ? ui.log.warn : console.warn,
    onPlan: (plugins) => {
      label(`building ${plugins.length} plugin${plugins.length === 1 ? "" : "s"} (dependency order)`);
    },
  });
  if (build.failed.length > 0) {
    throw new Error(formatBuildFailures(build.failed, build.outcomes.length));
  }

  // Recover the full WorkspacePlugin objects (dir, pkg, private) the plan/upload need, in the
  // exact dependency order buildWorkspace already established — no second ordering pass.
  const { ordered, built } = builtPluginsFromOutcomes(ws, build.outcomes);

  const plan = await computePlan(ordered, (name, version) => isAlreadyPublished(client, name, version));
  label("plan:");
  console.log(formatPlan(plan));

  const toPublish = publishCount(plan);
  if (toPublish === 0) {
    // §9.1: the plan comes back all-skip and the command is a named no-op, not a surprise —
    // exactly what deploying this repo's own (all-private) plugins/ workspace looks like.
    label("nothing to publish");
    return;
  }

  if (dryRun) {
    label(`dry run — ${toPublish} plugin${toPublish === 1 ? "" : "s"} would publish, uploading nothing`);
    return;
  }

  // Only now — with something real to upload — does a token become required. Same message as
  // the single-plugin path above.
  if (!creds?.token) {
    throw new Error(
      ci ? "S2SCRIPT_TOKEN required in CI" : "not logged in — run `s2s login` or set S2SCRIPT_TOKEN",
    );
  }

  if (!(yes || ci)) {
    if (!interactive) {
      // Confirming needs a TTY; without one, and without an explicit --yes/--ci, the safe
      // default is refusal — never a silent publish because stdout happened to be piped.
      throw new Error(
        "refusing to publish without confirmation in a non-interactive shell — pass --yes or --ci",
      );
    }
    const proceed = await ui.confirm({
      message: `Publish ${toPublish} plugin${toPublish === 1 ? "" : "s"} to ${client.baseUrl}?`,
      initialValue: false,
    });
    if (!proceed) {
      ui.outro("Cancelled.");
      process.exit(130);
    }
  }

  const results = await uploadPlan(plan, built, client);
  let failures = 0;
  for (const r of results) {
    if (r.status === "failed") failures++;
    const line = `${r.plugin.name}@${r.plugin.version}: ${r.status}${r.detail ? ` (${r.detail})` : ""}`;
    if (interactive) {
      (r.status === "failed" ? ui.log.error : r.status === "published" ? ui.log.success : ui.log.info)(line);
    } else {
      console.log(line);
    }
  }
  if (failures > 0) {
    throw new Error(`${failures} plugin${failures === 1 ? "" : "s"} failed to publish`);
  }
}

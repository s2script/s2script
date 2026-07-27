import { resolve } from "node:path";
import { buildPlugin } from "../build.ts";
import { resolvePackagesDir } from "../packages-resolve.ts";
import { parseFlag, positionals } from "../cli/args.ts";
import {
  buildWorkspace,
  collectFilters,
  formatBuildFailures,
  formatStampResult,
} from "../workspace/build-all.ts";
import type { StepRunner } from "../workspace/build-all.ts";
import { findWorkspaceRoot, loadWorkspace } from "../workspace/workspace.ts";
import * as ui from "../ui/ui.ts";

/** Flags that consume the following token, so it is never mistaken for the plugin directory. */
const VALUE_FLAGS = ["--packages-dir", "--filter", "--stamp-version"];

export async function run(argv: string[]): Promise<void> {
  const interactive = ui.isInteractive();
  try {
    const filters = collectFilters(argv);
    const stampVersion = parseFlag(argv, "--stamp-version");
    let dir = positionals(argv, VALUE_FLAGS)[0];

    // Workspace discovery (design spec 2026-07-27 §4.2) — the backward-compatibility hinge: walk
    // UP for the `s2script.workspace` marker, and when there is none every path below is exactly
    // today's single-plugin build.
    const wsRoot = findWorkspaceRoot(dir === undefined ? process.cwd() : resolve(dir));
    // `s2s build <one-plugin-dir>` stays single-plugin mode even inside a workspace (§4.2), so
    // per-plugin builds keep working; naming the ROOT (or naming nothing) asks for the whole thing.
    const workspaceMode = wsRoot !== null && (dir === undefined || resolve(dir) === wsRoot);

    if (!workspaceMode && (filters.length > 0 || stampVersion !== undefined)) {
      const flag = filters.length > 0 ? "--filter" : "--stamp-version";
      throw new Error(
        wsRoot === null
          ? `${flag} is a workspace flag, but no workspace root was found above ` +
            `${dir === undefined ? process.cwd() : resolve(dir)} — a workspace root is a package.json ` +
            `carrying an "s2script": { "workspace": { "plugins": [...] } } block`
          : `${flag} narrows a WORKSPACE build, but ${dir} names a single plugin — drop the ` +
            `argument (or pass ${wsRoot}) to build the workspace`,
      );
    }

    if (wsRoot !== null && workspaceMode) {
      await runWorkspace(argv, wsRoot, interactive, filters, stampVersion);
      return;
    }

    // --- Single-plugin mode: unchanged. ---
    if (!dir && interactive) {
      dir = await ui.text({ message: "Plugin directory to build", defaultValue: ".", placeholder: "." });
    }
    if (!dir) {
      console.error(
        "Usage: s2s build <dir> [--packages-dir <path>]\n" +
          "       s2s build [workspace root] [--filter <pattern>]... [--stamp-version <v>]",
      );
      process.exit(1);
    }
    const packagesDir = resolvePackagesDir({
      explicit: parseFlag(argv, "--packages-dir"),
      pluginDir: resolve(dir),
      fromCliUrl: import.meta.url,
    });
    const out = await ui.task(`Building ${dir}`, () => buildPlugin(dir, packagesDir), {
      interactive,
      done: (p) => `Built ${p}`,
    });
    // Non-interactive: keep the machine-readable plain path on stdout (the scripting invariant).
    if (interactive) ui.outro(out);
    else console.log(out);
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

/**
 * Workspace mode (§5.1): build every plugin (or every `--filter`ed one), collect every failure,
 * exit non-zero if any failed.
 *
 * The non-interactive stdout contract is single-plugin mode's, one line per plugin: nothing but
 * artifact paths, so `s2s build > built.txt` keeps working. Progress, stamp reports, warnings and
 * failures all go to stderr.
 */
async function runWorkspace(
  argv: string[],
  root: string,
  interactive: boolean,
  filters: string[],
  stampVersion: string | undefined,
): Promise<void> {
  const ws = loadWorkspace(root);
  const packagesDir = resolvePackagesDir({
    explicit: parseFlag(argv, "--packages-dir"),
    // npm hoists node_modules to the workspace ROOT, so that — not an individual plugin
    // directory — is where a workspace's @s2script/* stubs actually live.
    pluginDir: ws.root,
    fromCliUrl: import.meta.url,
  });

  const step: StepRunner = interactive
    ? (label, fn, done) => ui.task(label, fn, { interactive: true, done })
    : async (_label, fn) => {
        const out = await fn();
        console.log(out);
        return out;
      };

  const result = await buildWorkspace({
    workspace: ws,
    packagesDir,
    filters,
    stampVersion,
    step,
    warn: interactive ? ui.log.warn : console.warn,
    onPlan: (plugins, stamp) => {
      const plan = `building ${plugins.length} plugin${plugins.length === 1 ? "" : "s"} (dependency order) in ${root}`;
      if (interactive) {
        if (stamp) ui.log.step(formatStampResult(stamp));
        ui.log.step(plan);
      } else {
        if (stamp) console.error(formatStampResult(stamp));
        console.error(plan);
      }
    },
  });

  if (result.failed.length > 0) {
    const summary = formatBuildFailures(result.failed, result.outcomes.length);
    if (interactive) {
      ui.log.error(summary);
      ui.outro(`Built ${result.built.length} of ${result.outcomes.length} plugins`);
    } else {
      console.error(summary);
    }
    process.exit(1);
  }
  if (interactive) ui.outro(`Built ${result.built.length} plugin${result.built.length === 1 ? "" : "s"}`);
}

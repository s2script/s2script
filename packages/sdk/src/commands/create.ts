/**
 * `s2s create` — two modes (design spec 2026-07-27 §8):
 *
 *   `s2s create --workspace <dir>`  scaffolds a WORKSPACE ROOT (create/workspace.ts).
 *   `s2s create [name]`             scaffolds a plugin — single-plugin mode, unchanged, UNLESS
 *                                   the invocation is inside an existing workspace, in which case
 *                                   `name` is a bare slug and the plugin always lands at
 *                                   `plugins/<name>/` (create.ts's `createPlugin` does the actual
 *                                   minimal-vs-standalone branching once told which root).
 *
 * Workspace discovery mirrors `build.ts`/`deploy.ts`: walk UP from the target looking for the
 * `s2script.workspace` marker. `createPlugin` itself never touches `process.cwd()` — this is the
 * one place that does, because it is the actual CLI entrypoint with a real ambient cwd. Every
 * other caller of `createPlugin` (tests, programmatic use) stays fully deterministic over its
 * explicit options.
 *
 * `--library` maps to `kind: "library"` in single-plugin mode — a build-time package other
 * plugins bundle into their own `.s2sp` rather than something the host loads. `createPlugin`
 * refuses the combination with workspace mode (a workspace library lives outside the `plugins/`
 * glob, which this codepath cannot express); everything else about the two modes above is
 * unaffected.
 */
import { resolve } from "node:path";
import { createPlugin } from "../create/create.ts";
import { createWorkspace } from "../create/workspace.ts";
import { findWorkspaceRoot } from "../workspace/workspace.ts";
import { parseFlag, hasFlag, positionals } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";

/** Flags that consume the following token, so it is never mistaken for the plugin name/path. */
const VALUE_FLAGS = ["--name", "--game", "--template", "--install", "--workspace"];

export async function run(argv: string[]): Promise<void> {
  const yes = hasFlag(argv, "--yes") || hasFlag(argv, "-y");
  const interactive = ui.isInteractive({ yes });
  // `parseFlag` returns undefined both when a flag is absent AND when `--workspace` is given with
  // no following value — distinguish "absent" (fall through to plugin mode) from "given, default
  // to cwd" via a plain presence check, same as `hasFlag` does for boolean flags.
  const workspacePresent = hasFlag(argv, "--workspace") || argv.some((a) => a.startsWith("--workspace="));

  try {
    if (workspacePresent) {
      const result = await createWorkspace({
        path: parseFlag(argv, "--workspace"),
        name: parseFlag(argv, "--name"),
        yes,
      });
      const next = `next: cd ${result.dir} && npm install`;
      if (interactive) {
        ui.log.success(`created workspace ${result.dir}`);
        ui.outro(next);
      } else {
        console.log(`created workspace ${result.dir}`);
        console.log(next);
      }
      return;
    }

    const pathArg = positionals(argv, VALUE_FLAGS)[0];
    // §4.2/§8: the same walk-up build.ts and deploy.ts use. Unlike those, ANY name here (not just
    // the root/no-arg case) means workspace mode — the thing being created does not exist yet, so
    // there is no "just this one plugin" path to keep separate from "add it to the workspace".
    const workspaceRoot = findWorkspaceRoot(pathArg === undefined ? process.cwd() : resolve(pathArg));

    // createPlugin owns the wizard (intro + prompts + install spinner) when interactive.
    const result = await createPlugin({
      path: pathArg,
      name: parseFlag(argv, "--name"),
      game: parseFlag(argv, "--game") as "cs2" | "none" | undefined,
      template: parseFlag(argv, "--template") as "minimal" | undefined,
      install: parseFlag(argv, "--install") as "npm" | "pnpm" | "yarn" | "bun" | "none" | undefined,
      noInstall: hasFlag(argv, "--no-install"),
      kind: hasFlag(argv, "--library") ? "library" : undefined,
      workspaceRoot: workspaceRoot ?? undefined,
      yes,
    });

    const next = result.workspaceRoot
      ? `next: npm install at the workspace root (${result.workspaceRoot}), then cd ${result.dir} && npm run build`
      : result.installed
        ? `dependencies installed (${result.packageManager})`
        : result.skippedInstall
          ? `next: cd ${result.dir} && npm run build`
          : `next: cd ${result.dir} && npm install && npm run build`;
    // A library has no `s2s deploy` consumer surface until it's published — point at the two
    // halves of that lifecycle right where the plugin path points at `npm run build`. Empty for a
    // plugin, so this changes nothing about the existing byte-stable non-interactive output.
    const libraryHint =
      result.kind === "library" ? " (publish with `s2s deploy`; consumers get it with `s2s add`)" : "";

    if (interactive) {
      ui.log.success(`created ${result.dir}`);
      ui.outro(next + libraryHint);
    } else {
      console.log(`created ${result.dir}`);
      console.log(next + libraryHint);
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

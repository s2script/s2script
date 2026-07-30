import { resolve } from "node:path";
import { addPackage } from "../registry/add.ts";
import { parseFlag, positionals } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";
import { findWorkspaceRoot } from "../workspace/workspace.ts";

export async function run(argv: string[]): Promise<void> {
  const interactive = ui.isInteractive();
  const pos = positionals(argv, ["--dir", "--registry"]);
  let spec = pos[0];
  if (!spec && interactive) {
    spec = await ui.text({ message: "Package to add (name[@range])" });
  }
  if (!spec) {
    console.error("Usage: s2s add <pkg>[@range] [--dir <plugin>] [--registry <url>]");
    process.exit(1);
  }
  try {
    // Design spec §14: `add` keeps its exact single-plugin semantics everywhere — including at a
    // specific plugin INSIDE a workspace (--dir plugins/x, unaffected below) — but it is one of
    // the five commands that must refuse by name when run AT a workspace root, where there is no
    // plugin directory to act on. Without this, `s2s add @foo/bar` at a workspace root wrote
    // .s2script/types/@foo/bar/ and injected s2script.pluginDependencies into the ROOT's
    // package.json — the workspace MARKER FILE itself — silently declaring a dependency the root
    // can never build.
    const pluginDir = parseFlag(argv, "--dir") || ".";
    const absDir = resolve(pluginDir);
    const root = findWorkspaceRoot(absDir);
    if (root !== null && resolve(root) === absDir) {
      throw new Error(
        `${absDir} is a workspace root (design spec 2026-07-27 §4.1), not a plugin — ` +
          `\`s2s add\` needs a plugin directory: pass --dir <plugin> (e.g. --dir plugins/shop)`,
      );
    }

    const result = await ui.task(
      `Adding ${spec}`,
      () =>
        addPackage({
          pluginDir,
          spec: spec!,
          registryUrl: parseFlag(argv, "--registry"),
        }),
      {
        interactive,
        done: (r) =>
          r.kind === "library"
            ? `Added library ${r.name}@${r.version} → ${r.libDir} (commit this directory)`
            : `Added ${r.name}@${r.version} → ${r.typesDir}`,
      },
    );
    if (interactive) {
      // A library never gets an .npmrc line (npmrcLine is always null for one), so this
      // branch only ever fires on the plugin path — nothing extra to gate on result.kind.
      if (result.npmrcLine) {
        ui.log.info(`npmrc: ${result.npmrcLine}  (types-only; npm install ${result.name} also works)`);
      }
      if (result.reviewState === "unreviewed") {
        ui.log.warn("Not reviewed by s2script — types pulled; use at your own risk.");
      }
    } else if (result.kind === "library") {
      console.log(`added library ${result.name}@${result.version} → ${result.libDir}`);
      if (result.reviewState === "unreviewed") {
        console.log("note: Not reviewed by s2script — library pulled; use at your own risk.");
      }
    } else {
      console.log(`added ${result.name}@${result.version} → ${result.typesDir}`);
      if (result.npmrcLine) {
        console.log(`npmrc: ${result.npmrcLine}  (types-only; npm install ${result.name} also works)`);
      }
      if (result.reviewState === "unreviewed") {
        console.log("note: Not reviewed by s2script — types pulled; use at your own risk.");
      }
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

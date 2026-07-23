import { resolve } from "node:path";
import { deployPlugin } from "../registry/deploy.ts";
import { resolvePackagesDir } from "../packages-resolve.ts";
import { parseFlag, hasFlag, positionals } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";

export async function run(argv: string[]): Promise<void> {
  const interactive = ui.isInteractive();
  const pos = positionals(argv, ["--packages-dir", "--registry"]);
  const dir = pos[0] ?? ".";
  try {
    const packagesDir = resolvePackagesDir({
      explicit: parseFlag(argv, "--packages-dir"),
      pluginDir: resolve(dir),
      fromCliUrl: import.meta.url,
    });
    const result = await ui.task(
      `Deploying ${dir}`,
      () =>
        deployPlugin({
          dir,
          packagesDir,
          registryUrl: parseFlag(argv, "--registry"),
          ci: hasFlag(argv, "--ci"),
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

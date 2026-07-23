import { resolve } from "node:path";
import { buildPlugin } from "../build.ts";
import { resolvePackagesDir } from "../packages-resolve.ts";
import { parseFlag } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";

export async function run(argv: string[]): Promise<void> {
  const interactive = ui.isInteractive();
  let dir = argv.find((a) => !a.startsWith("-"));
  if (!dir && interactive) {
    dir = await ui.text({ message: "Plugin directory to build", defaultValue: ".", placeholder: "." });
  }
  if (!dir) {
    console.error("Usage: s2s build <dir> [--packages-dir <path>]");
    process.exit(1);
  }
  try {
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

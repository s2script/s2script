import { createPlugin } from "../create/create.ts";
import { parseFlag, hasFlag } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";

export async function run(argv: string[]): Promise<void> {
  const yes = hasFlag(argv, "--yes") || hasFlag(argv, "-y");
  const interactive = ui.isInteractive({ yes });
  const pathArg = argv.find((a) => !a.startsWith("-"));
  try {
    // createPlugin owns the wizard (intro + prompts + install spinner) when interactive.
    const result = await createPlugin({
      path: pathArg,
      name: parseFlag(argv, "--name"),
      game: parseFlag(argv, "--game") as "cs2" | "none" | undefined,
      template: parseFlag(argv, "--template") as "minimal" | undefined,
      install: parseFlag(argv, "--install") as "npm" | "pnpm" | "yarn" | "bun" | "none" | undefined,
      noInstall: hasFlag(argv, "--no-install"),
      yes,
    });
    const next = result.installed
      ? `dependencies installed (${result.packageManager})`
      : result.skippedInstall
        ? `next: cd ${result.dir} && npm run build`
        : `next: cd ${result.dir} && npm install && npm run build`;
    if (interactive) {
      ui.log.success(`created ${result.dir}`);
      ui.outro(next);
    } else {
      console.log(`created ${result.dir}`);
      console.log(next);
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

import { find } from "./commands/index.ts";
import * as menu from "./commands/menu.ts";
import * as ui from "./ui/ui.ts";

const argv = process.argv.slice(2);
const command = argv[0];

function usage(): void {
  console.error(
    "Usage:\n" +
      "  s2s create [path] [--game cs2|none] [--name <pkg>] [--template minimal]\n" +
      "             [--install npm|pnpm|yarn|bun|none] [--no-install] [-y]\n" +
      "  s2s build <dir> [--packages-dir <path>]\n" +
      "  s2s login [--token s2s_…] [--registry <url>]\n" +
      "  s2s deploy [dir] [--ci] [--registry <url>] [--packages-dir <path>]\n" +
      "  s2s add <pkg>[@range] [--dir <plugin>] [--registry <url>]\n" +
      "  s2s config gen <plugin.s2sp...> --out <dir>\n" +
      "  s2s gen-schema [--check]\n" +
      "  s2s gen-events [--check]\n" +
      "  s2s gen-nav [--check]\n" +
      "\n" +
      "Env: S2SCRIPT_REGISTRY_URL  S2SCRIPT_TOKEN (CI deploy)",
  );
}

if (!command) {
  // Bare `s2s`: an arrow-key menu in a terminal, the usage text otherwise (unchanged for scripts/CI).
  if (ui.isInteractive()) {
    await menu.run();
  } else {
    usage();
    process.exit(1);
  }
} else {
  const cmd = find(command);
  if (!cmd) {
    usage();
    process.exit(1);
  }
  await cmd.run(argv.slice(1));
}

import { runConfigGen } from "../config/gen.ts";
import { parseFlag, positionals } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";

export async function run(argv: string[]): Promise<void> {
  // The only subcommand is `gen`; accept `config gen …` and (from the menu) a bare `config`.
  const rest = argv[0] === "gen" ? argv.slice(1) : argv;
  const interactive = ui.isInteractive();
  const outDir = parseFlag(rest, "--out") ?? process.cwd();
  let s2sps = positionals(rest, ["--out"]);
  if (s2sps.length === 0 && interactive) {
    const p = (await ui.text({ message: "Path to a .s2sp file" })).trim();
    if (p) s2sps = [p];
  }
  if (s2sps.length === 0) {
    console.error("Usage: s2s config gen <plugin.s2sp...> --out <dir>");
    process.exit(1);
  }
  try {
    const { written, skipped } = runConfigGen(s2sps, outDir);
    for (const w of written) {
      if (interactive) ui.log.success(`wrote ${w}`);
      else console.log(`config gen: wrote ${w}`);
    }
    for (const s of skipped) {
      if (interactive) ui.log.info(`${s} declares no config — skipped`);
      else console.log(`config gen: ${s} declares no config — skipped`);
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

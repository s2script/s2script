/**
 * `s2s enable <name>...` / `s2s disable <name>...` — move installed plugins out of / into
 * plugins/disabled/ (where the loader never looks) and record it in plugins.json, so the startup
 * update keeps them there. No download: the installed file moves.
 */
import { hasFlag, parseFlag, positionals } from "../cli/args.ts";
import { findServerRoot } from "../server/manifest.ts";
import { serverSetEnabled } from "../server/commands.ts";
import { runServer, serverClient } from "./install.ts";

async function setEnabled(argv: string[], enabled: boolean): Promise<void> {
  const names = positionals(argv, ["--registry", "--server"]);
  const dryRun = hasFlag(argv, "--dry-run");
  const root = findServerRoot(parseFlag(argv, "--server") ?? ".");
  if (!root) {
    console.error("no plugins.json here (or in ./addons/s2script) — install a plugin with `s2s install <name>` first");
    process.exit(1);
  }
  await runServer(
    () => serverSetEnabled({ root, client: serverClient(argv), names, enabled, dryRun, log: console.log }),
    dryRun,
  );
}

export const enable = (argv: string[]): Promise<void> => setEnabled(argv, true);
export const disable = (argv: string[]): Promise<void> => setEnabled(argv, false);

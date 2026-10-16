/**
 * `s2s update [name...]` — npm update for a server: move each plugin (or just the named ones) to
 * the newest version its plugins.json range allows, then rewrite plugins-lock.json. The runtime
 * does the same at every startup unless `autoUpdate` is false.
 */
import { hasFlag, parseFlag, positionals } from "../cli/args.ts";
import { findServerRoot } from "../server/manifest.ts";
import { serverUpdate } from "../server/commands.ts";
import { runServer, serverClient } from "./install.ts";

export async function run(argv: string[]): Promise<void> {
  const names = positionals(argv, ["--registry", "--server"]);
  const dryRun = hasFlag(argv, "--dry-run");
  const root = findServerRoot(parseFlag(argv, "--server") ?? ".");
  if (!root) {
    console.error("no plugins.json here (or in ./addons/s2script) — run `s2s server init` first");
    process.exit(1);
  }
  await runServer(
    () => serverUpdate({ root, client: serverClient(argv), names, dryRun, log: console.log }),
    dryRun,
  );
}

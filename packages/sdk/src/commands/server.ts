/**
 * `s2s server init [dir]` — write a server's plugins.json + plugins-lock.json from the plugins it
 * already has, so `s2s install` / `s2s update` and the runtime's startup update can manage them.
 *
 * [dir] is the addon root (addons/s2script); run from the game directory it is found for you.
 */
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { hasFlag, parseFlag, positionals } from "../cli/args.ts";
import { serverInit } from "../server/commands.ts";
import { findAddonRoot } from "../server/manifest.ts";
import { serverClient } from "./install.ts";


export async function run(argv: string[]): Promise<void> {
  const [sub, dir] = positionals(argv, ["--registry", "--name"]);
  if (sub !== "init") {
    console.error("Usage: s2s server init [addons/s2script] [--name <server>] [--force] [--registry <url>]");
    process.exit(1);
  }
  const root = dir ? resolve(dir) : (findAddonRoot(".") ?? resolve("addons", "s2script"));
  try {
    if (!existsSync(join(root, "plugins"))) {
      throw new Error(`${root} has no plugins/ directory — pass your addons/s2script directory`);
    }
    const report = await serverInit({
      root,
      client: serverClient(argv),
      name: parseFlag(argv, "--name"),
      force: hasFlag(argv, "--force"),
    });
    for (const m of report.managed) console.log(`managed   ${m.name}@${m.version} (${m.filename})`);
    for (const u of report.unmanaged) console.log(`unmanaged ${u.filename}: ${u.reason}`);
    console.log(
      `wrote ${join(root, "plugins.json")} and plugins-lock.json — ${report.managed.length} managed, ` +
        `${report.unmanaged.length} left as they are`,
    );
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

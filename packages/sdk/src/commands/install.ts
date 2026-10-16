/**
 * `s2s install [name[@range]...]` — npm install for a server's plugins.json (addons/s2script/).
 * Without names it installs exactly what plugins-lock.json records (re-resolving only when
 * plugins.json changed); with names it adds them, recording ^latest for a bare name. Like npm, a
 * server with no plugins.json yet gets one.
 */
import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { RegistryClient } from "../registry/client.ts";
import { loadCredentials, defaultRegistryUrl } from "../registry/credentials.ts";
import { parseFlag, hasFlag, positionals } from "../cli/args.ts";
import { SERVER_MANIFEST, findAddonRoot, newServerManifestJson } from "../server/manifest.ts";
import { serverInstall } from "../server/commands.ts";

/**
 * A registry client for server commands: --registry, else the login's. The login's token rides
 * along, so org members can install restricted packages (and a server can use a read-only
 * S2SCRIPT_TOKEN); public downloads redirect to the CDN, where fetch drops the header.
 */
export function serverClient(argv: string[]): RegistryClient {
  const creds = loadCredentials();
  return new RegistryClient({
    baseUrl: parseFlag(argv, "--registry") || creds?.registryUrl || defaultRegistryUrl(),
    token: creds?.token,
  });
}

/** Run a server workflow, printing its summary or its error (exit 1). */
export async function runServer(fn: () => Promise<string>, dryRun: boolean): Promise<void> {
  try {
    const summary = await fn();
    console.log(dryRun ? `dry run — ${summary}; nothing written` : `done: ${summary}`);
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

/** Every `--with <member>` / `--with=<member>` (repeatable). */
function collectWith(argv: readonly string[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    if (arg.startsWith("--with=")) {
      if (arg.length > "--with=".length) out.push(arg.slice("--with=".length));
      continue;
    }
    if (arg !== "--with") continue;
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("-")) {
      throw new Error("--with requires an optional member name (e.g. --with @edge/jailbreak-gangperks)");
    }
    out.push(value);
  }
  return out;
}

/** Flags of the retired s2script-plugins.json installer, and what replaced each. */
const REMOVED_FLAGS: Record<string, string> = {
  "--dir": "run from your game directory, or pass --server <addons/s2script>",
  "--file": "list plugins in addons/s2script/plugins.json (`s2s install <name>` adds them)",
  "--reviewed-only": 'set "reviewedOnly": true in addons/s2script/plugins.json',
};

export async function run(argv: string[]): Promise<void> {
  for (const [flag, instead] of Object.entries(REMOVED_FLAGS)) {
    if (argv.some((a) => a === flag || a.startsWith(`${flag}=`))) {
      console.error(`s2s install ${flag} was removed with s2script-plugins.json — ${instead}`);
      process.exit(1);
    }
  }
  const names = positionals(argv, ["--registry", "--with", "--server"]);
  const dryRun = hasFlag(argv, "--dry-run");
  const root = findAddonRoot(parseFlag(argv, "--server") ?? ".");
  if (!root) {
    console.error(
      "Usage: s2s install [name[@range]...] [--with <member>] [--disabled] [--server <addons/s2script>] [--dry-run]\n" +
        "  Run it from your game directory (csgo/) or addons/s2script, or pass --server.",
    );
    process.exit(1);
  }
  await runServer(async () => {
    const withMembers = collectWith(argv);
    if (!existsSync(join(root, SERVER_MANIFEST))) {
      if (dryRun) throw new Error(`no ${SERVER_MANIFEST} in ${root} yet — run without --dry-run to create one`);
      const fresh = newServerManifestJson("s2script-server", {});
      writeFileSync(join(root, SERVER_MANIFEST), JSON.stringify(fresh, null, 2) + "\n");
      console.log(`created ${join(root, SERVER_MANIFEST)}`);
    }
    return serverInstall({
      root,
      client: serverClient(argv),
      specs: names,
      with: withMembers,
      disabled: hasFlag(argv, "--disabled"),
      dryRun,
      log: console.log,
    });
  }, dryRun);
}

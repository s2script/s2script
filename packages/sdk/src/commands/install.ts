import { resolve } from "node:path";
import { existsSync } from "node:fs";
import { RegistryClient } from "../registry/client.ts";
import { loadCredentials, defaultRegistryUrl } from "../registry/credentials.ts";
import { loadManifestFile, mergeSpecs, installPlan, parseSpec } from "../registry/install.ts";
import { parseFlag, hasFlag, positionals } from "../cli/args.ts";
import { findServerRoot } from "../server/manifest.ts";
import { serverInstall } from "../server/commands.ts";

/** A registry client for server commands: --registry, else the login's, with the login's token. */
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

const AUTODETECT = "addons/s2script/plugins";

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

export async function run(argv: string[]): Promise<void> {
  const args = positionals(argv, ["--dir", "--file", "--registry", "--with", "--server"]);
  const file = parseFlag(argv, "--file") ?? "s2script-plugins.json";
  const registry =
    parseFlag(argv, "--registry") || loadCredentials()?.registryUrl || defaultRegistryUrl();
  const dryRun = hasFlag(argv, "--dry-run");
  const reviewedOnly = hasFlag(argv, "--reviewed-only");

  // A server with a plugins.json installs from it and keeps plugins-lock.json —
  // unless the caller asked for the older --dir / --file manifest flow explicitly.
  if (parseFlag(argv, "--dir") === undefined && parseFlag(argv, "--file") === undefined) {
    const root = findServerRoot(parseFlag(argv, "--server") ?? ".");
    if (root) {
      await runServer(() => {
        if (collectWith(argv).length > 0) {
          throw new Error(`--with: add the member to "with" in ${root}/plugins.json instead`);
        }
        if (reviewedOnly) {
          throw new Error(`--reviewed-only: set "reviewedOnly": true in ${root}/plugins.json instead`);
        }
        return serverInstall({ root, client: serverClient(argv), specs: args, dryRun, log: console.log });
      }, dryRun);
      return;
    }
    if (parseFlag(argv, "--server") !== undefined) {
      console.error(`no plugins.json found — run \`s2s server init\` there first`);
      process.exit(1);
    }
  }

  let dir = parseFlag(argv, "--dir");
  if (!dir) {
    if (existsSync(resolve(AUTODETECT))) dir = resolve(AUTODETECT);
    else {
      console.error(
        `Usage: s2s install [name[@range]...] --dir <plugins dir>\n` +
          `  No --dir given and ./${AUTODETECT} not found. Point --dir at your addons/s2script/plugins directory.`
      );
      process.exit(1);
    }
  }

  // Your login rides along, so org members can install restricted packages (and a server can use
  // a read-only S2SCRIPT_TOKEN). Public downloads redirect to the CDN, where fetch drops the header.
  const client = new RegistryClient({ baseUrl: registry, token: loadCredentials()?.token });
  try {
    const withArgs = collectWith(argv);
    const manifest = loadManifestFile(resolve(file));
    const specs = mergeSpecs(manifest, args);
    const withMembers = { ...(manifest.with ?? {}) };
    if (withArgs.length > 0) {
      // --with names optional members of ONE bundle on the command line; with several roots it
      // would be a guess which bundle each member belongs to.
      if (args.length !== 1) {
        throw new Error(
          `--with applies to exactly one bundle named on the command line (got ${args.length}) — ` +
            `for several bundles, use { "range", "with" } entries in ${file}`,
        );
      }
      withMembers[parseSpec(args[0]!).name] = withArgs;
    }
    if (Object.keys(specs).length === 0) {
      console.error(
        `Nothing to install. Add plugins to ${file} or pass names: s2s install rtv@^1.0.0`
      );
      process.exit(1);
    }

    const res = await installPlan({
      client,
      specs,
      with: withMembers,
      dir,
      dryRun,
      reviewedOnly,
      log: (m) => console.log(m),
    });
    if (dryRun) {
      console.log("dry run — nothing written");
    } else {
      console.log(
        `done: ${res.written.length} installed, ${res.skipped.length} up to date, in ${dir}`
      );
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

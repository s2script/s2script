import { resolve } from "node:path";
import { existsSync } from "node:fs";
import { RegistryClient } from "../registry/client.ts";
import { loadCredentials, defaultRegistryUrl } from "../registry/credentials.ts";
import { loadManifestFile, mergeSpecs, installPlan } from "../registry/install.ts";
import { parseFlag, hasFlag, positionals } from "../cli/args.ts";

const AUTODETECT = "addons/s2script/plugins";

export async function run(argv: string[]): Promise<void> {
  const args = positionals(argv, ["--dir", "--file", "--registry"]);
  const file = parseFlag(argv, "--file") ?? "s2script-plugins.json";
  const registry =
    parseFlag(argv, "--registry") || loadCredentials()?.registryUrl || defaultRegistryUrl();
  const dryRun = hasFlag(argv, "--dry-run");
  const reviewedOnly = hasFlag(argv, "--reviewed-only");

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

  const client = new RegistryClient({ baseUrl: registry });
  try {
    const manifest = loadManifestFile(resolve(file));
    const specs = mergeSpecs(manifest, args);
    if (Object.keys(specs).length === 0) {
      console.error(
        `Nothing to install. Add plugins to ${file} or pass names: s2s install rtv@^1.0.0`
      );
      process.exit(1);
    }

    const res = await installPlan({
      client,
      specs,
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

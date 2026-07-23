import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { unzipSync } from "fflate";
import { buildPlugin } from "../build.ts";
import { assertPublishesTypes, hasPublishes } from "../publish-gate.ts";
import { packTypesTarball } from "../types-pack.ts";
import { loadCredentials, defaultRegistryUrl } from "./credentials.ts";
import { RegistryClient } from "./client.ts";

export async function deployPlugin(opts: {
  dir: string;
  packagesDir?: string;
  registryUrl?: string;
  ci?: boolean;
}): Promise<{ name: string; version: string; reviewState: string; disclaimer?: string }> {
  const absDir = resolve(opts.dir);
  const pkg = JSON.parse(readFileSync(resolve(absDir, "package.json"), "utf8"));
  const gate = assertPublishesTypes(pkg, absDir);
  if (!gate.ok) throw new Error(`publish gate failed: ${gate.error}`);

  const creds = loadCredentials();
  if (!creds?.token) {
    throw new Error(
      opts.ci
        ? "S2SCRIPT_TOKEN required in CI"
        : "not logged in — run `s2s login` or set S2SCRIPT_TOKEN"
    );
  }

  // buildPlugin derives the authoritative manifest (stamped apiVersion, derived publishes,
  // compiledAgainst). Deploy that — do not reconstruct from package.json.
  const outPath = await buildPlugin(absDir, opts.packagesDir);
  const s2sp = readFileSync(outPath);
  const manifestEntry = unzipSync(s2sp)["manifest.json"];
  if (!manifestEntry) {
    throw new Error(`built archive missing manifest.json: ${outPath}`);
  }
  const manifest = JSON.parse(Buffer.from(manifestEntry).toString("utf8")) as {
    id: string;
    version: string;
    apiVersion: string;
    pluginDependencies?: Record<string, string>;
    publishes?: Record<string, unknown> | string | null;
    [key: string]: unknown;
  };

  let types: Buffer | null = null;
  if (gate.typesPath) {
    types = packTypesTarball({
      name: typeof manifest.id === "string" ? manifest.id : pkg.name,
      version: typeof manifest.version === "string" ? manifest.version : pkg.version,
      typesPath: gate.typesPath,
      publishes: hasPublishes(manifest.publishes)
        ? (manifest.publishes as Record<string, unknown>)
        : undefined,
    });
  }

  const client = new RegistryClient({
    baseUrl: opts.registryUrl || creds.registryUrl || defaultRegistryUrl(),
    token: creds.token,
  });

  return client.deploy({ manifest, s2sp, types });
}

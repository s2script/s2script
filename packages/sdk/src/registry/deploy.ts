import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { unzipSync } from "fflate";
import { buildPlugin } from "../build.ts";
import { packageKind, buildLibrary } from "../build-library.ts";
import { assertPublishesTypes, hasPublishes } from "../publish-gate.ts";
import { packTypesTarball } from "../types-pack.ts";
import { loadCredentials, defaultRegistryUrl } from "./credentials.ts";
import { RegistryClient } from "./client.ts";

/**
 * The package.json fields deploy cares about. Everything else passes through untouched — this is
 * the same narrow-read/wide-pass-through shape as `workspace/workspace.ts`'s
 * `WorkspacePackageJson`, which is what a `WorkspacePlugin.pkg` actually is when
 * workspace/deploy-all.ts calls into `assembleDeployArchive`/`assertDeployable` below.
 */
export interface DeployablePkgJson {
  name?: string;
  version?: string;
  types?: string;
  typings?: string;
  private?: boolean;
  s2script?: {
    kind?: string;
    publishes?: Record<string, unknown> | string | null;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

/**
 * §6.3 / design spec finding 3.4: `private: true` means "built, never published" (§9.1's own
 * npm/changesets semantics), and every one of the repo's 18 base plugins already carries it.
 * `registry/deploy.ts` never checked `private` — so `s2s deploy plugins/basechat`, a SINGLE
 * plugin, would publish a private package today. This is the named gate that closes that hole,
 * shared by both the single-plugin path below and workspace/deploy-all.ts's plan (§6.1 step 2),
 * so a private package can never reach `client.deploy` by either route.
 */
export function assertDeployable(pkg: DeployablePkgJson, label: string): void {
  if (pkg.private === true) {
    throw new Error(
      `${pkg.name ?? label} is private ("private": true in package.json) — private packages are ` +
        `never published to the registry. Remove "private" to publish it, or leave it out of ` +
        `this deploy (design spec 2026-07-27 §6.3).`,
    );
  }
}

export interface DeployArchive {
  manifest: Record<string, unknown>;
  // Exactly one of s2sp/lib is set — which one is `packageKind(pkg)`'s call, not a caller choice.
  s2sp: Buffer | null;
  lib: Buffer | null;
  types: Buffer | null;
}

/**
 * Assemble the upload payload for a package ALREADY built to `outPath` (buildPlugin's or
 * buildLibrary's own output).
 *
 * Split out of `deployPlugin` so workspace-mode deploy can reuse it: §6.1 step 1 builds every
 * targeted plugin FIRST, as its own phase, and only afterwards uploads the ones the plan says to
 * — that two-phase split is what makes "any build failure means publish nothing" true by
 * construction, and it means the upload phase must not re-invoke `buildPlugin`/`buildLibrary` per
 * plugin.
 */
export function assembleDeployArchive(
  pluginDir: string,
  pkg: DeployablePkgJson,
  outPath: string,
): DeployArchive {
  const absDir = resolve(pluginDir);

  if (packageKind(pkg) === "library") {
    // A library's types ride INSIDE the .s2lib (build-library.ts requires `types` to build one at
    // all) — there is no separate types tarball to pack, and `assertPublishesTypes` gates a
    // plugin-only concept (`s2script.publishes` + a standalone types artifact) that a library is
    // refused from declaring in the first place, so it never runs for this branch.
    const lib = readFileSync(outPath);
    const manifestEntry = unzipSync(lib)["manifest.json"];
    if (!manifestEntry) {
      throw new Error(`built archive missing manifest.json: ${outPath}`);
    }
    const manifest = JSON.parse(Buffer.from(manifestEntry).toString("utf8")) as Record<string, unknown>;
    return { manifest, s2sp: null, lib, types: null };
  }

  const gate = assertPublishesTypes(pkg, absDir);
  if (!gate.ok) throw new Error(`publish gate failed: ${gate.error}`);

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
      name: typeof manifest.id === "string" ? manifest.id : (pkg.name as string),
      version: typeof manifest.version === "string" ? manifest.version : (pkg.version as string),
      typesPath: gate.typesPath,
      publishes: hasPublishes(manifest.publishes)
        ? (manifest.publishes as Record<string, unknown>)
        : undefined,
    });
  }

  return { manifest, s2sp, lib: null, types };
}

export async function deployPlugin(opts: {
  dir: string;
  packagesDir?: string;
  registryUrl?: string;
  ci?: boolean;
  /** Injectable for tests — mirrors RegistryClientOpts.fetch / addPackage's own opts.fetch. */
  fetch?: typeof fetch;
}): Promise<{ name: string; version: string; reviewState: string; disclaimer?: string }> {
  const absDir = resolve(opts.dir);
  const pkg = JSON.parse(readFileSync(resolve(absDir, "package.json"), "utf8")) as DeployablePkgJson;

  // §6.3: checked before anything is built or a token is even required — a private package is
  // refused as fast as a malformed one, library or plugin alike.
  assertDeployable(pkg, absDir);

  const creds = loadCredentials();
  if (!creds?.token) {
    throw new Error(
      opts.ci
        ? "S2SCRIPT_TOKEN required in CI"
        : "not logged in — run `s2s login` or set S2SCRIPT_TOKEN"
    );
  }

  // buildPlugin/buildLibrary derives the authoritative manifest (stamped apiVersion, derived
  // publishes, compiledAgainst). Deploy that — do not reconstruct from package.json. Which one to
  // call is `packageKind`'s call, not a flag: `assembleDeployArchive` below reads the SAME `pkg`
  // to decide how to unpack whatever `outPath` turns out to be, so the two must never disagree.
  const outPath =
    packageKind(pkg) === "library"
      ? await buildLibrary(absDir, opts.packagesDir)
      : await buildPlugin(absDir, opts.packagesDir);
  const { manifest, s2sp, lib, types } = assembleDeployArchive(absDir, pkg, outPath);

  const client = new RegistryClient({
    baseUrl: opts.registryUrl || creds.registryUrl || defaultRegistryUrl(),
    token: creds.token,
    fetch: opts.fetch,
  });

  return client.deploy({ manifest, s2sp, lib, types });
}

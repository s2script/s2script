/**
 * s2script add — download types-only artifact into .s2script/types/<pkg>/
 */

import {
  mkdirSync,
  writeFileSync,
  readFileSync,
  existsSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { gunzipSync } from "node:zlib";
import { loadCredentials, defaultRegistryUrl } from "./credentials.ts";
import { RegistryClient } from "./client.ts";
import { ensureScopeNpmrc } from "./npmrc.ts";
import { extractLibArchive } from "./lib-extract.ts";

function parseSpec(spec: string): { name: string; range: string } {
  // @scope/name@version or @scope/name@^1 or name@1.0.0
  if (spec.startsWith("@")) {
    const at = spec.indexOf("@", 1);
    if (at < 0) return { name: spec, range: "*" };
    return { name: spec.slice(0, at), range: spec.slice(at + 1) || "*" };
  }
  const at = spec.lastIndexOf("@");
  if (at <= 0) return { name: spec, range: "*" };
  return { name: spec.slice(0, at), range: spec.slice(at + 1) || "*" };
}

/**
 * Minimal ustar extract: pull package/api.d.ts (+ package.json) into outDir.
 * Local layout matches SDK `localContractPath`: `.s2script/types/<pkg>/index.d.ts`.
 */
export function extractTypesTarball(tgz: Buffer, outDir: string): void {
  const tar = gunzipSync(tgz);
  mkdirSync(outDir, { recursive: true });
  let offset = 0;
  let wroteContract = false;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    offset += 512;
    if (header.every((b) => b === 0)) break;
    const name = header.subarray(0, 100).toString("utf8").replace(/\0.*$/, "");
    const sizeOct = header.subarray(124, 135).toString("utf8").replace(/\0.*$/, "").trim();
    const size = parseInt(sizeOct, 8) || 0;
    const content = tar.subarray(offset, offset + size);
    offset += size + ((512 - (size % 512)) % 512);
    if (!name || name.endsWith("/")) continue;
    // Prefer files under package/
    const base = name.replace(/^package\//, "");
    if (base === "package.json") {
      writeFileSync(join(outDir, "package.json"), content);
    } else if (base === "api.d.ts" || base === "index.d.ts") {
      // npm-shaped tarball ships api.d.ts; local verified-copy path is index.d.ts
      writeFileSync(join(outDir, "index.d.ts"), content);
      wroteContract = true;
    }
  }
  if (!wroteContract && !existsSync(join(outDir, "index.d.ts"))) {
    throw new Error("types tarball missing package/api.d.ts (or index.d.ts)");
  }
}

/**
 * Pin a declared range to the resolved version, the same way for both the plugin
 * (pluginDependencies) and library (s2script.libraries) branches — the two callers
 * must agree so `s2s add pkg` and `s2s add lib` read the same at a glance.
 */
function rangeForResolved(range: string, version: string): string {
  const major = version.split(".")[0] ?? "0";
  return range.startsWith("^") || range === "*"
    ? `^${major}.0.0`
    : range.includes(".")
      ? range
      : `^${version}`;
}

export type AddedPackage =
  | {
      kind: "plugin";
      name: string;
      version: string;
      reviewState: string;
      typesDir: string;
      libDir: null;
      npmrcLine: string | null;
    }
  | {
      kind: "library";
      name: string;
      version: string;
      reviewState: string;
      typesDir: string;
      libDir: string;
      npmrcLine: null;
    };

export async function addPackage(opts: {
  pluginDir: string;
  spec: string;
  registryUrl?: string;
}): Promise<AddedPackage> {
  const absDir = resolve(opts.pluginDir);
  const { name, range } = parseSpec(opts.spec);
  const creds = loadCredentials();
  const client = new RegistryClient({
    baseUrl: opts.registryUrl || creds?.registryUrl || defaultRegistryUrl(),
    token: creds?.token,
  });

  const resolved = await client.resolve(name, range);

  if (resolved.kind === "library") {
    // A library's types live INSIDE the .s2lib (build-library.ts), never in the
    // separate types tarball, so `hasTypes` (which describes that tarball) is not
    // consulted here at all — it is always false for a library and that is correct.
    const s2lib = await client.downloadLib(name, resolved.version);
    const libDir = join(absDir, ".s2script", "libs", ...name.split("/"));
    extractLibArchive(s2lib, libDir, { name, version: resolved.version });

    const pkgPath = join(absDir, "package.json");
    const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
    pkg.s2script = pkg.s2script ?? {};
    pkg.s2script.libraries = pkg.s2script.libraries ?? {};
    pkg.s2script.libraries[name] = rangeForResolved(range, resolved.version);
    writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");

    return {
      kind: "library",
      name: resolved.name,
      version: resolved.version,
      reviewState: resolved.reviewState,
      typesDir: libDir,
      libDir,
      // Deliberately NO .npmrc line: nothing about a library may look like an npm
      // package — that is the exact confusion this feature exists to prevent.
      npmrcLine: null,
    };
  }

  if (!resolved.hasTypes) {
    throw new Error(`${name}@${resolved.version} has no types artifact (runtime-only package?)`);
  }
  const tgz = await client.downloadTypes(name, resolved.version);
  const typesDir = join(absDir, ".s2script", "types", ...name.split("/"));
  extractTypesTarball(tgz, typesDir);

  // Merge pluginDependencies
  const pkgPath = join(absDir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  pkg.s2script = pkg.s2script ?? {};
  pkg.s2script.pluginDependencies = pkg.s2script.pluginDependencies ?? {};
  pkg.s2script.pluginDependencies[name] = rangeForResolved(range, resolved.version);
  writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");

  const registryUrl = opts.registryUrl || creds?.registryUrl || defaultRegistryUrl();
  const npmrcLine = ensureScopeNpmrc(absDir, name, registryUrl);

  return {
    kind: "plugin",
    name: resolved.name,
    version: resolved.version,
    reviewState: resolved.reviewState,
    typesDir,
    libDir: null,
    npmrcLine,
  };
}

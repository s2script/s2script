/**
 * `s2s pack` — `npm pack` for an s2script package: build the source tarball `s2s deploy` uploads,
 * and write it (or, with --dry-run, only list it). The file selection is source-pack.ts's, the same
 * code deploy runs, so what this prints is exactly what a deploy sends.
 */
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { hasFlag, parseFlag, positionals } from "../cli/args.ts";
import {
  assertSourceSize,
  packSourceTarball,
  tarballFilename,
  type PackagePkgJson,
} from "../source-pack.ts";

/** npm's byte formatting: 1.2kB, 3.4MB (decimal units). */
export function formatBytes(n: number): string {
  if (n < 1000) return `${n}B`;
  if (n < 1000 * 1000) return `${(n / 1000).toFixed(1)}kB`;
  return `${(n / 1000 / 1000).toFixed(1)}MB`;
}

export interface PackResult {
  id: string;
  name: string;
  version: string;
  filename: string;
  size: number;
  unpackedSize: number;
  shasum: string;
  integrity: string;
  files: { path: string; size: number }[];
  entryCount: number;
}

export function packPackage(dir: string): { result: PackResult; tarball: Buffer } {
  const absDir = resolve(dir);
  const pkg = JSON.parse(readFileSync(join(absDir, "package.json"), "utf8")) as PackagePkgJson;
  if (typeof pkg.name !== "string" || typeof pkg.version !== "string") {
    throw new Error(`${join(absDir, "package.json")}: "name" and "version" are required to pack`);
  }
  const pack = packSourceTarball(absDir, pkg);
  return {
    tarball: pack.tarball,
    result: {
      id: `${pkg.name}@${pkg.version}`,
      name: pkg.name,
      version: pkg.version,
      filename: tarballFilename(pkg.name, pkg.version),
      size: pack.tarball.length,
      unpackedSize: pack.unpackedSize,
      shasum: createHash("sha1").update(pack.tarball).digest("hex"),
      integrity: `sha512-${createHash("sha512").update(pack.tarball).digest("base64")}`,
      files: pack.files,
      entryCount: pack.files.length,
    },
  };
}

/** The `npm notice` report npm prints for pack and publish, minus the prefix. */
export function formatPackReport(r: PackResult): string {
  const width = Math.max(...r.files.map((f) => formatBytes(f.size).length), 0);
  return [
    `📦  ${r.id}`,
    "Tarball Contents",
    ...r.files.map((f) => `${formatBytes(f.size).padEnd(width)} ${f.path}`),
    "Tarball Details",
    `name:          ${r.name}`,
    `version:       ${r.version}`,
    `filename:      ${r.filename}`,
    `package size:  ${formatBytes(r.size)}`,
    `unpacked size: ${formatBytes(r.unpackedSize)}`,
    `shasum:        ${r.shasum}`,
    `integrity:     ${r.integrity.slice(0, 20)}[...]${r.integrity.slice(-15)}`,
    `total files:   ${r.entryCount}`,
  ].join("\n");
}

export async function run(argv: string[]): Promise<void> {
  const dryRun = hasFlag(argv, "--dry-run");
  const json = hasFlag(argv, "--json");
  const dest = parseFlag(argv, "--pack-destination") ?? ".";
  const dir = positionals(argv, ["--pack-destination"])[0] ?? ".";
  try {
    const { result, tarball } = packPackage(dir);
    assertSourceSize({ tarball, files: result.files, unpackedSize: result.unpackedSize }, result.id);
    if (!dryRun) writeFileSync(resolve(dest, result.filename), tarball);
    if (json) {
      console.log(JSON.stringify([result], null, 2));
    } else {
      // npm prints the report to stderr and only the filename to stdout, so scripts can capture it.
      console.error(formatPackReport(result));
      console.log(result.filename);
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}

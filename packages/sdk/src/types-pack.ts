/**
 * Pack a types-only npm-shaped tarball (gzip+ustar) for registry upload /
 * local inspection. Contains package/package.json + package/api.d.ts.
 */

import { readFileSync } from "node:fs";
import { packTar } from "./tar.ts";

export interface TypesPackInput {
  name: string;
  version: string;
  /** Absolute path to the producer's .d.ts */
  typesPath: string;
  publishes?: Record<string, unknown>;
}

/** Build gzipped ustar buffer (npm-compatible layout under package/). */
export function packTypesTarball(input: TypesPackInput): Buffer {
  const dts = readFileSync(input.typesPath);
  const pkgJson = Buffer.from(
    JSON.stringify(
      {
        name: input.name,
        version: input.version,
        types: "api.d.ts",
        s2script: {
          kind: "interface",
          ...(input.publishes ? { publishes: input.publishes } : {}),
        },
      },
      null,
      2
    ) + "\n",
    "utf8"
  );

  return packTar([
    { path: "package/package.json", content: pkgJson },
    { path: "package/api.d.ts", content: dts },
  ]);
}

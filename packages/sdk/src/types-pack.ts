/**
 * Pack a types-only npm-shaped tarball (gzip+ustar) for registry upload /
 * local inspection. Contains package/package.json + package/api.d.ts.
 */

import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

export interface TypesPackInput {
  name: string;
  version: string;
  /** Absolute path to the producer's .d.ts */
  typesPath: string;
  publishes?: Record<string, unknown>;
}

function pad(buf: Buffer, size: number): Buffer {
  if (buf.length > size) throw new Error(`tar field overflow (${buf.length}>${size})`);
  const out = Buffer.alloc(size, 0);
  buf.copy(out);
  return out;
}

function tarHeader(name: string, size: number, type: string): Buffer {
  const header = Buffer.alloc(512, 0);
  pad(Buffer.from(name, "utf8"), 100).copy(header, 0);
  Buffer.from("0000644\0", "utf8").copy(header, 100); // mode
  Buffer.from("0000000\0", "utf8").copy(header, 108); // uid
  Buffer.from("0000000\0", "utf8").copy(header, 116); // gid
  const sizeOct = size.toString(8).padStart(11, "0") + "\0";
  Buffer.from(sizeOct, "utf8").copy(header, 124);
  const mtime = Math.floor(Date.now() / 1000)
    .toString(8)
    .padStart(11, "0") + "\0";
  Buffer.from(mtime, "utf8").copy(header, 136);
  Buffer.from("        ", "utf8").copy(header, 148); // checksum placeholder
  header[156] = type.charCodeAt(0); // '0' file
  Buffer.from("ustar\0", "utf8").copy(header, 257);
  Buffer.from("00", "utf8").copy(header, 263);

  let sum = 0;
  for (let i = 0; i < 512; i++) sum += header[i]!;
  const chk = sum.toString(8).padStart(6, "0") + "\0 ";
  Buffer.from(chk, "utf8").copy(header, 148);
  return header;
}

function tarFile(name: string, content: Buffer): Buffer {
  const header = tarHeader(name, content.length, "0");
  const padLen = (512 - (content.length % 512)) % 512;
  return Buffer.concat([header, content, Buffer.alloc(padLen, 0)]);
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

  const tar = Buffer.concat([
    tarFile("package/package.json", pkgJson),
    tarFile("package/api.d.ts", dts),
    Buffer.alloc(1024, 0), // two zero blocks = EOF
  ]);
  return gzipSync(tar);
}

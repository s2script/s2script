/**
 * A minimal gzip+ustar writer for the npm-shaped tarballs the SDK packs (types.tgz, source.tgz).
 * Regular files only, no directory entries — the same shape `npm pack` emits.
 */

import { gzipSync } from "node:zlib";

export interface TarFile {
  /** Path inside the archive, `/`-separated, e.g. `package/src/plugin.ts`. */
  path: string;
  content: Buffer;
  /** Unix permission bits. Default 0o644. */
  mode?: number;
}

/** `npm pack`'s fixed mtime (1985-10-26T08:15:00Z), so the same files always give the same bytes. */
export const NPM_PACK_MTIME = 499162500;

function writeString(header: Buffer, value: string, offset: number, size: number): void {
  const bytes = Buffer.from(value, "utf8");
  if (bytes.length > size) throw new Error(`tar field overflow (${bytes.length}>${size})`);
  bytes.copy(header, offset);
}

function writeOctal(header: Buffer, value: number, offset: number, size: number): void {
  writeString(header, value.toString(8).padStart(size - 1, "0") + "\0", offset, size);
}

/**
 * ustar splits a long path into a prefix (≤155 bytes) and a name (≤100 bytes) at a `/`.
 * A path that fits neither way is refused: silently truncating it would corrupt the archive.
 */
export function splitUstarPath(path: string): { name: string; prefix: string } {
  if (Buffer.byteLength(path, "utf8") <= 100) return { name: path, prefix: "" };
  for (let i = path.indexOf("/"); i !== -1; i = path.indexOf("/", i + 1)) {
    const prefix = path.slice(0, i);
    const name = path.slice(i + 1);
    if (Buffer.byteLength(prefix, "utf8") > 155) break;
    if (name && Buffer.byteLength(name, "utf8") <= 100) return { name, prefix };
  }
  throw new Error(`path too long for a tar archive (max 255 bytes, split at a "/"): ${path}`);
}

function tarHeader(file: TarFile, mtime: number): Buffer {
  const header = Buffer.alloc(512, 0);
  const { name, prefix } = splitUstarPath(file.path);
  writeString(header, name, 0, 100);
  writeOctal(header, file.mode ?? 0o644, 100, 8);
  writeOctal(header, 0, 108, 8); // uid
  writeOctal(header, 0, 116, 8); // gid
  writeOctal(header, file.content.length, 124, 12);
  writeOctal(header, mtime, 136, 12);
  header.fill(0x20, 148, 156); // checksum placeholder: eight spaces
  header[156] = "0".charCodeAt(0); // regular file
  writeString(header, "ustar\0", 257, 6);
  writeString(header, "00", 263, 2);
  writeString(header, prefix, 345, 155);

  let sum = 0;
  for (let i = 0; i < 512; i++) sum += header[i]!;
  writeString(header, sum.toString(8).padStart(6, "0") + "\0 ", 148, 8);
  return header;
}

/** Gzipped ustar archive of `files`, in the order given. */
export function packTar(files: TarFile[], opts: { mtime?: number } = {}): Buffer {
  const mtime = opts.mtime ?? Math.floor(Date.now() / 1000);
  const parts: Buffer[] = [];
  for (const file of files) {
    parts.push(tarHeader(file, mtime), file.content);
    const padLen = (512 - (file.content.length % 512)) % 512;
    if (padLen) parts.push(Buffer.alloc(padLen, 0));
  }
  parts.push(Buffer.alloc(1024, 0)); // two zero blocks = end of archive
  return gzipSync(Buffer.concat(parts));
}

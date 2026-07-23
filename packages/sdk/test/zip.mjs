/**
 * Minimal zip read/write helpers for the test suite, backed by fflate.
 * Replaces adm-zip (dropped for a high-severity advisory) — provides just the
 * slice of its API the tests call: openZip(path).{readAsText,readFile,getEntries}
 * and writeZip(path, { name: Buffer|Uint8Array }).
 */
import { readFileSync, writeFileSync } from "node:fs";
import { unzipSync, zipSync, strFromU8 } from "fflate";

/** Open a .s2sp/.zip from a file path. */
export function openZip(path) {
  const files = unzipSync(readFileSync(path));
  return {
    readAsText: (name) => (files[name] ? strFromU8(files[name]) : null),
    readFile: (name) => (files[name] ? Buffer.from(files[name]) : null),
    getEntries: () => Object.keys(files).map((entryName) => ({ entryName })),
  };
}

/** Write a zip from { name: Buffer|Uint8Array } to `path`. */
export function writeZip(path, files) {
  const out = {};
  for (const [name, buf] of Object.entries(files)) out[name] = new Uint8Array(buf);
  writeFileSync(path, zipSync(out));
}

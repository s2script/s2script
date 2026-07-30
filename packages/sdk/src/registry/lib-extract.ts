/**
 * .s2lib -> .s2script/libs/<name>/ (the committed, vendored tree).
 *
 * Only the three known basenames are ever written, and only directly inside outDir.
 * Entry names arrive from the registry, so they are treated as untrusted data, never
 * as paths — same posture as install.ts's safeFilename.
 */

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { unzipSync } from "fflate";

const WANTED = new Set(["index.js", "index.d.ts"]);

export function extractLibArchive(
  s2lib: Buffer,
  outDir: string,
  meta: { name: string; version: string },
): void {
  const entries = unzipSync(new Uint8Array(s2lib));
  if (!entries["index.js"]?.length) throw new Error("library archive missing index.js");
  if (!entries["index.d.ts"]?.length) throw new Error("library archive missing index.d.ts");

  mkdirSync(outDir, { recursive: true });
  for (const [name, bytes] of Object.entries(entries)) {
    if (!WANTED.has(name)) continue;
    writeFileSync(join(outDir, name), Buffer.from(bytes));
  }

  // A local package.json makes esbuild's nodePaths resolution explicit rather than
  // relying on directory-index fallback, and records what version is vendored so a
  // reviewer reading the diff can see it.
  let apiVersion = "1.x";
  try {
    const m = JSON.parse(Buffer.from(entries["manifest.json"] ?? new Uint8Array()).toString("utf8"));
    if (typeof m.apiVersion === "string") apiVersion = m.apiVersion;
  } catch {
    // A manifest we cannot read costs only the apiVersion gate's input; the code is still usable.
  }
  writeFileSync(
    join(outDir, "package.json"),
    JSON.stringify(
      { name: meta.name, version: meta.version, main: "index.js", types: "index.d.ts", s2script: { kind: "library", apiVersion } },
      null,
      2,
    ) + "\n",
  );
}

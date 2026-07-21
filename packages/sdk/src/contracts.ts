/**
 * The verified-copy convention (design spec 2026-07-15 §4.6, landed in B1): a consumer keeps a
 * BYTE-copy of a producer's published contract at `.s2script/types/<interface>/index.d.ts`.
 * The typecheck gate paths-maps the interface module to it (real types, not an `any` stub) and
 * `s2s build` hashes the same bytes into `manifest.compiledAgainst[<interface>]`, which the
 * loader verifies against the producer's published `typesSha256` (fail-fast + per-call).
 */

import { existsSync } from "node:fs";
import { join } from "node:path";

/** Absolute path of the plugin's verified copy for `dep`, or null (absent / traversal-unsafe). */
export function localContractPath(pluginDir: string, dep: string): string | null {
  const segs = dep.split("/");
  if (segs.some((s) => s === "" || s === "." || s === "..")) return null;
  const p = join(pluginDir, ".s2script", "types", ...segs, "index.d.ts");
  return existsSync(p) ? p : null;
}

/**
 * The `s2script.publishes` grammar (design spec 2026-07-15 §4.2).
 *
 * AUTHORED (package.json):  "self"  |  { "<interface>": "<range>" }
 * DERIVED  (manifest.json): { "<interface>": { version, typesSha256 } }
 *
 * The interface NAME is decoupled from the package name: @edge/mce@3.1.0 may
 * publish @community/mapchooser@1.2.0. "self" is sugar for the dominant case
 * (name = package name, version = package version) and does NOT compose — a
 * plugin publishing its own contract AND implementing another's uses the map form.
 */

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

/** One derived manifest entry: the resolved contract version + its content hash. */
export interface PublishDecl {
  version: string;
  typesSha256: string;
}

/** The authored form, straight off package.json. */
export type PublishesAuthored = string | Record<string, string> | undefined;

/** Expand the authored form to `{interface: range}`. Throws on a malformed grammar. */
export function expandPublishes(
  authored: PublishesAuthored,
  pkgName: string,
  pkgVersion: string,
): Record<string, string> {
  if (authored === undefined || authored === null) return {};
  if (typeof authored === "string") {
    if (authored.trim() !== "self") {
      throw new Error(
        `publishes: the only valid string form is "self" (got ${JSON.stringify(authored)}); ` +
          `use the map form to publish a differently-named contract`,
      );
    }
    return { [pkgName]: pkgVersion };
  }
  if (typeof authored !== "object") {
    throw new Error(`publishes must be "self" or an object (got ${typeof authored})`);
  }
  const out: Record<string, string> = {};
  for (const [iface, range] of Object.entries(authored)) {
    if (typeof range !== "string") {
      throw new Error(`publishes[${JSON.stringify(iface)}] must be a version range string`);
    }
    out[iface] = range;
  }
  return out;
}

/** sha256 hex of the contract's RAW bytes. No normalization — any canonicalization
 *  step would be a second source of truth (spec §4.2). */
export function hashContract(typesPath: string): string {
  return createHash("sha256").update(readFileSync(typesPath)).digest("hex");
}

/** Expand + hash → the manifest `publishes` block. */
export function derivePublishes(
  authored: PublishesAuthored,
  pkgName: string,
  pkgVersion: string,
  typesPath: string | null,
): Record<string, PublishDecl> {
  const expanded = expandPublishes(authored, pkgName, pkgVersion);
  const names = Object.keys(expanded);
  if (names.length === 0) return {};
  if (typesPath === null) {
    throw new Error(
      `publishes is set but no contract .d.ts was resolved — set "types": "api.d.ts" in package.json`,
    );
  }
  const typesSha256 = hashContract(typesPath);
  const out: Record<string, PublishDecl> = {};
  for (const name of names) {
    out[name] = { version: expanded[name], typesSha256 };
  }
  return out;
}

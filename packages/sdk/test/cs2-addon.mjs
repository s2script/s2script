/**
 * The CS2 addon JS bundle for offline-VM tests — the same game-package files, in the same order,
 * that game-package.jsonc names for dist's js/pawn.js at package time
 * (schema.generated.js → nav.generated.js → activity.js → csitem.generated.js → weapon.js → pawn.js).
 *
 * The order is DERIVED from game-package.jsonc's bootstrapInputs rather than
 * hardcoded, so these tests can never drift from the real bundle — that drift (pawn.js gained a
 * `globalThis.__s2pkg_cs2.Weapon` read from weapon.js while the harness still fed it schema+pawn only)
 * is exactly what broke them.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { stripJsonComments } from "../src/gamedata/jsonc.ts";
import { hashCanonical } from "../src/engine-functions/canonical-json.ts";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");

/** Read the source manifest's ordered bootstrap file list. */
function bundleFiles() {
  const source = JSON.parse(stripJsonComments(readFileSync(join(repo, "games/cs2/game-package.jsonc"), "utf8")));
  const files = source.bootstrapInputs;
  if (!Array.isArray(files) || !files.length || files.some(f => !/^js\/[\w.-]+\.js$/.test(f)))
    throw new Error("cs2-addon: invalid bootstrapInputs in games/cs2/game-package.jsonc");
  return files.map(f => `games/cs2/${f}`);
}

/** Engine-function adapter sources in the builder's order (sorted by adapter id), prepended. */
function adapterEntries() {
  const source = JSON.parse(stripJsonComments(readFileSync(join(repo, "games/cs2/game-package.jsonc"), "utf8")));
  return Object.keys(source.adapters ?? {}).sort().map(id => [id, source.adapters[id]]);
}

/**
 * id -> locked contract hash, as the builder's `__s2_adapter_contracts` prelude publishes it.
 * A test host that models the S2 adapter natives installs this global itself.
 */
export const cs2AdapterContracts = Object.freeze(Object.fromEntries(adapterEntries().map(([id, entry]) =>
  [id, hashCanonical(JSON.parse(readFileSync(join(repo, "games/cs2", entry.contract), "utf8")))])));

/** The concatenated CS2 addon bundle, ready to hand to vm.runInContext. */
export const cs2AddonBundle = [...adapterEntries().map(([, entry]) => `games/cs2/${entry.source}`), ...bundleFiles()]
  .map((f) => readFileSync(join(repo, f), "utf8"))
  .join("\n");

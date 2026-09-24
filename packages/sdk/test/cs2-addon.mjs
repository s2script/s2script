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

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");

/** Read the source manifest's ordered bootstrap file list. */
function bundleFiles() {
  const source = JSON.parse(stripJsonComments(readFileSync(join(repo, "games/cs2/game-package.jsonc"), "utf8")));
  const files = source.bootstrapInputs;
  if (!Array.isArray(files) || !files.length || files.some(f => !/^js\/[\w.-]+\.js$/.test(f)))
    throw new Error("cs2-addon: invalid bootstrapInputs in games/cs2/game-package.jsonc");
  return files.map(f => `games/cs2/${f}`);
}

/** The concatenated CS2 addon bundle, ready to hand to vm.runInContext. */
export const cs2AddonBundle = bundleFiles()
  .map((f) => readFileSync(join(repo, f), "utf8"))
  .join("\n");

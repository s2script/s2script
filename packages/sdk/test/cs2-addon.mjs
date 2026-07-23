/**
 * The CS2 addon JS bundle for offline-VM tests — the same game-package files, in the same order,
 * that scripts/package-addon.sh concatenates into dist's js/pawn.js at package time
 * (schema.generated.js → nav.generated.js → activity.js → csitem.generated.js → weapon.js → pawn.js).
 *
 * The order is DERIVED from package-addon.sh's `cat games/cs2/js/… > …/pawn.js` line rather than
 * hardcoded, so these tests can never drift from the real bundle — that drift (pawn.js gained a
 * `globalThis.__s2pkg_cs2.Weapon` read from weapon.js while the harness still fed it schema+pawn only)
 * is exactly what broke them.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");

/** Parse package-addon.sh's `cat games/cs2/js/… > …/pawn.js` line into its ordered file list. */
function bundleFiles() {
  const sh = readFileSync(join(repo, "scripts/package-addon.sh"), "utf8");
  const line = sh.split("\n").find((l) => /^\s*cat\s+games\/cs2\/js\/.*\bpawn\.js\b\s*>/.test(l));
  if (!line) throw new Error("cs2-addon: no `cat games/cs2/js/… > …/pawn.js` line in scripts/package-addon.sh");
  const files = line.slice(0, line.indexOf(">")).match(/games\/cs2\/js\/[\w.-]+\.js/g) || [];
  if (!files.length) throw new Error("cs2-addon: parsed no game js files from package-addon.sh");
  return files;
}

/** The concatenated CS2 addon bundle, ready to hand to vm.runInContext. */
export const cs2AddonBundle = bundleFiles()
  .map((f) => readFileSync(join(repo, f), "utf8"))
  .join("\n");

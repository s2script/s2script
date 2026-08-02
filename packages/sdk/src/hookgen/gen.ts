import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildHookModel, type GamedataHooks } from "./model.ts";
import { emitHookDts } from "./emit-dts.ts";
import { stripJsonComments } from "../gamedata/jsonc.ts";

// The cs2 game package's own gamedata (JSONC — comments are load-bearing documentation there, see
// the file's own header). `hooks` is Task 5's descriptor section; this generator reads it directly
// rather than re-deriving names, so a ninth hook needs no codegen edit, only a gamedata entry.
const GAMEDATA_PATH = "gamedata/cs2/game.cs2.jsonc";
const DTS_OUT = "packages/cs2/hooks.generated.d.ts";

/** Generate the `ctx` hook-augmentation `.d.ts` artifact from the cs2 game package's declared
 *  `hooks`. `check:true` compares against the committed file (no write) and reports drift. */
export function runGenHooks(repoRoot: string, opts: { check: boolean }): { hooks: number; drift: string[] } {
  const raw = readFileSync(join(repoRoot, GAMEDATA_PATH), "utf8");
  const gd = JSON.parse(stripJsonComments(raw));
  const hooks: GamedataHooks = gd.hooks ?? {};
  const model = buildHookModel(hooks);
  const dts = emitHookDts(model);
  const drift: string[] = [];

  const abs = join(repoRoot, DTS_OUT);
  if (opts.check) {
    let cur = "";
    try { cur = readFileSync(abs, "utf8"); } catch { /* missing */ }
    if (cur !== dts) drift.push(DTS_OUT);
  } else {
    writeFileSync(abs, dts);
  }

  const hookCount = model.reduce((n, ns) => n + ns.hooks.length, 0);
  return { hooks: hookCount, drift };
}

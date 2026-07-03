import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildEventModel, type EventCatalog } from "./model.ts";
import { emitEventDts } from "./emit-dts.ts";

const CATALOG_PATH = "games/cs2/gamedata/event-catalog.json";
const DTS_OUT = "packages/cs2/events.generated.d.ts";

/** Generate the event-catalog .d.ts artifact. `check:true` compares against the committed file (no write) and reports drift. */
export function runGenEvents(repoRoot: string, opts: { check: boolean }): { events: number; drift: string[] } {
  const catalog: EventCatalog = JSON.parse(readFileSync(join(repoRoot, CATALOG_PATH), "utf8"));
  const model = buildEventModel(catalog);
  const dts = emitEventDts(model);
  const drift: string[] = [];

  const abs = join(repoRoot, DTS_OUT);
  if (opts.check) {
    let cur = "";
    try { cur = readFileSync(abs, "utf8"); } catch { /* missing */ }
    if (cur !== dts) drift.push(DTS_OUT);
  } else {
    writeFileSync(abs, dts);
  }

  return { events: model.length, drift };
}

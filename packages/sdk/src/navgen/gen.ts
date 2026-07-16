import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildNavModel, type NavConfigEntry } from "./model.ts";
import { emitNavJs } from "./emit-js.ts";
import { emitNavDts } from "./emit-dts.ts";
import type { Catalog } from "../schemagen/model.ts";

const TARGETS_PATH = "games/cs2/nav-targets.json";
const CATALOG_PATH = "games/cs2/gamedata/schema-catalog.json";
const JS_OUT = "games/cs2/js/nav.generated.js";
const DTS_OUT = "packages/cs2/nav.generated.d.ts";

/** Generate the two nav artifacts. `check:true` compares against the committed files (no write) and reports drift. */
export function runGenNav(repoRoot: string, opts: { check: boolean }): { wrappers: number; fields: number; drift: string[] } {
  const config: NavConfigEntry[] = JSON.parse(readFileSync(join(repoRoot, TARGETS_PATH), "utf8"));
  const catalog: Catalog = JSON.parse(readFileSync(join(repoRoot, CATALOG_PATH), "utf8"));
  const model = buildNavModel(config, catalog);
  const js = emitNavJs(model);
  const dts = emitNavDts(model);

  const fields = model.wrappers.reduce((n, w) => n + w.fields.length, 0);
  const drift: string[] = [];

  const files: [string, string][] = [[JS_OUT, js], [DTS_OUT, dts]];
  for (const [rel, content] of files) {
    const abs = join(repoRoot, rel);
    if (opts.check) {
      let cur = "";
      try { cur = readFileSync(abs, "utf8"); } catch { /* missing */ }
      if (cur !== content) drift.push(rel);
    } else {
      writeFileSync(abs, content);
    }
  }

  return { wrappers: model.wrappers.length, fields, drift };
}

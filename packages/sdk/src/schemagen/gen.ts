import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildModel, type Catalog } from "./model.ts";
import { emitDts } from "./emit-dts.ts";
import { emitJs } from "./emit-js.ts";

const CATALOG_PATH = "games/cs2/gamedata/schema-catalog.json";
/** Enum widths + enumerators. Its own file: the catalog serializes as a bare class map. */
const ENUMS_PATH = "games/cs2/gamedata/schema-enums.json";
const LIST_PATH = "games/cs2/codegen-classes.json";
/** Structs embedded inline in a generated class that should be exposed as nested accessors. */
const EMBED_PATH = "games/cs2/codegen-embedded.json";
const JS_OUT = "games/cs2/js/schema.generated.js";
const DTS_OUT = "packages/cs2/schema.generated.d.ts";

/** Generate the two artifacts. `check:true` compares against the committed files (no write) and reports drift. */
export function runGenSchema(repoRoot: string, opts: { check: boolean }): { classes: number; fields: number; skipped: number; drift: string[] } {
  const catalog: Catalog = JSON.parse(readFileSync(join(repoRoot, CATALOG_PATH), "utf8"));
  const list: string[] = JSON.parse(readFileSync(join(repoRoot, LIST_PATH), "utf8"));
  // Optional: a repo without the file simply generates no embedded structs.
  let embed: string[] = [];
  try { embed = JSON.parse(readFileSync(join(repoRoot, EMBED_PATH), "utf8")); } catch { /* absent */ }
  // Absent enum table = enum fields stay plain integers, exactly as before it existed.
  let enumCatalog = {};
  try { enumCatalog = JSON.parse(readFileSync(join(repoRoot, ENUMS_PATH), "utf8")); } catch { /* absent */ }
  const model = buildModel(catalog, list, embed, enumCatalog);
  const dts = emitDts(model);
  const js = emitJs(model);

  const fields = model.classes.reduce((n, c) => n + c.ownFields.length, 0);
  const skipped = model.classes.reduce((n, c) => n + c.skipped.length, 0);
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
  // Report collisions + a per-class skip summary to stderr (auditable coverage).
  if (model.collisions.length) console.error(`gen-schema: ${model.collisions.length} name collision(s) → raw fallback:\n  ` + model.collisions.join("\n  "));
  for (const c of model.classes) if (c.skipped.length) console.error(`gen-schema: ${c.className}: skipped ${c.skipped.length} field(s)`);
  return { classes: model.classes.length, fields, skipped, drift };
}

import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { buildModel } from "../src/schemagen/model.ts";
import { emitDts } from "../src/schemagen/emit-dts.ts";
import { emitJs } from "../src/schemagen/emit-js.ts";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const catalog = JSON.parse(readFileSync(join(repo, "games/cs2/gamedata/schema-catalog.json"), "utf8"));
const list = JSON.parse(readFileSync(join(repo, "games/cs2/codegen-classes.json"), "utf8"));

test("generation is deterministic (byte-identical across runs)", () => {
  const a = buildModel(catalog, list);
  const b = buildModel(catalog, list);
  assert.equal(emitDts(a), emitDts(b));
  assert.equal(emitJs(a), emitJs(b));
});

test("real catalog: CCSPlayerPawn resolves health via CBaseEntity, friction present, chain intact", () => {
  const dts = emitDts(buildModel(catalog, list));
  assert.match(dts, /export interface CCSPlayerPawn extends CCSPlayerPawnBase \{/);
  const js = emitJs(buildModel(catalog, list));
  assert.match(js, /readInt32\(off\("CBaseEntity","m_iHealth"\)\)/);      // health inherited from CBaseEntity
  assert.match(js, /readFloat32\(off\("CBaseEntity","m_flFriction"\)\)/);
});

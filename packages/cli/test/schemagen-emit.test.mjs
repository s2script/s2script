import { test } from "node:test";
import assert from "node:assert";
import { buildModel } from "../src/schemagen/model.ts";
import { emitDts } from "../src/schemagen/emit-dts.ts";

const CATALOG = {
  Base: { parent: null, fields: [
    { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
    { name: "m_flFriction", offset: 12, type: { kind: "atomic", name: "float32" } },
    { name: "m_vecOrigin", offset: 16, type: { kind: "atomic", name: "Vector" } },   // skipped
  ] },
  Leaf: { parent: "Base", fields: [
    { name: "m_hController", offset: 24, type: { kind: "handle", inner: "Base" } },
    { name: "m_bScoped", offset: 28, type: { kind: "atomic", name: "bool" } },
  ] },
};

test("emitDts: extends chain, own fields only, writable vs readonly, skipped absent", () => {
  const dts = emitDts(buildModel(CATALOG, ["Leaf"]));
  assert.match(dts, /import type \{ EntityRef \} from "@s2script\/std";/);
  assert.match(dts, /export interface Base \{/);
  assert.match(dts, /health: number \| null;/);       // writable → mutable
  assert.match(dts, /friction: number \| null;/);
  assert.match(dts, /export interface Leaf extends Base \{/);
  assert.match(dts, /readonly controller: EntityRef \| null;/);  // handle → readonly
  assert.match(dts, /scoped: boolean \| null;/);
  assert.doesNotMatch(dts, /origin/);                  // Vector skipped
  assert.doesNotMatch(dts, /m_vecOrigin/);
});

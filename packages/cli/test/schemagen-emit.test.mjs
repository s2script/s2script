import { test } from "node:test";
import assert from "node:assert";
import { buildModel } from "../src/schemagen/model.ts";
import { emitDts } from "../src/schemagen/emit-dts.ts";
import { emitJs } from "../src/schemagen/emit-js.ts";
import vm from "node:vm";

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

test("emitJs: flattened getters/setters, live off() resolve, notifyStateChanged on write", () => {
  const js = emitJs(buildModel(CATALOG, ["Leaf"]));
  // getter reads via the declaring class + raw name, resolved live:
  assert.match(js, /readInt32\(off\("Base","m_iHealth"\)\)/);
  assert.match(js, /readFloat32\(off\("Base","m_flFriction"\)\)/);
  assert.match(js, /readHandle\(off\("Leaf","m_hController"\)\)/);
  assert.doesNotMatch(js, /m_vecOrigin/);   // skipped field absent
  // NO baked offset numbers (layout-is-data): the reference offsets 8/12/24/28 must not appear as read args
  assert.doesNotMatch(js, /readInt32\(\s*8\s*\)/);

  // Evaluate in a sandbox with stub natives; assert the accessors work + writes notify.
  const reads = [];
  const writes = [];
  const notified = [];
  const ctx = {
    globalThis: {},
    __s2_schema_offset: (cls, field) => 100,   // any non-negative offset
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(js, ctx);
  const schema = ctx.__s2pkg_cs2_schema;
  assert.equal(typeof schema.applyAccessors, "function");
  const ref = {
    readInt32: (o) => { reads.push(o); return 100; },
    readFloat32: () => 0.25,
    readBool: () => true,
    readHandle: () => ({ index: 1, serial: 7 }),
    writeInt32: (o, v) => { writes.push([o, v]); return true; },
    writeFloat32: () => true, writeBool: () => true,
    notifyStateChanged: (o) => { notified.push(o); },
  };
  const pawn = schema.wrap("Leaf", ref);
  assert.equal(pawn.health, 100);            // getter, flattened from Base
  assert.equal(pawn.friction, 0.25);
  assert.deepEqual(pawn.controller, { index: 1, serial: 7 });
  pawn.health = 55;                          // writable → write + notify
  assert.deepEqual(writes, [[100, 55]]);
  assert.deepEqual(notified, [100]);
});

test("emitDts: extends chain, own fields only, writable vs readonly, skipped absent", () => {
  const dts = emitDts(buildModel(CATALOG, ["Leaf"]));
  assert.match(dts, /import type \{ EntityRef \} from "@s2script\/entity";/);
  assert.match(dts, /export interface Base \{/);
  assert.match(dts, /health: number \| null;/);       // writable → mutable
  assert.match(dts, /friction: number \| null;/);
  assert.match(dts, /export interface Leaf extends Base \{/);
  assert.match(dts, /readonly controller: EntityRef \| null;/);  // handle → readonly
  assert.match(dts, /scoped: boolean \| null;/);
  assert.doesNotMatch(dts, /origin/);                  // Vector skipped
  assert.doesNotMatch(dts, /m_vecOrigin/);
});

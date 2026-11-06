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
    { name: "m_vecOrigin", offset: 16, type: { kind: "atomic", name: "Vector2D" } },   // skipped (Vector2D deferred)
  ] },
  Leaf: { parent: "Base", fields: [
    { name: "m_hController", offset: 24, type: { kind: "handle", inner: "Base" } },
    { name: "m_bScoped", offset: 28, type: { kind: "atomic", name: "bool" } },
  ] },
};

test("emitJs: flattened getters/setters, live off() resolve, notifyStateChanged on write", () => {
  const js = emitJs(buildModel(CATALOG, ["Leaf"]));
  // getter reads via the declaring class + raw name, resolved live:
  assert.match(js, /readInt32\(\(O\[\d+\] >= 0 \? O\[\d+\] : \(O\[\d+\] = off\("Base","m_iHealth"\)\)\)\)/);
  assert.match(js, /readFloat32\(\(O\[\d+\] >= 0 \? O\[\d+\] : \(O\[\d+\] = off\("Base","m_flFriction"\)\)\)\)/);
  assert.match(js, /readHandle\(\(O\[\d+\] >= 0 \? O\[\d+\] : \(O\[\d+\] = off\("Leaf","m_hController"\)\)\)\)/);
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
  assert.match(dts, /import type \{ EntityRef \} from "@s2script\/sdk\/entity";/);
  assert.match(dts, /export interface Base \{/);
  assert.match(dts, /health: number \| null;/);       // writable → mutable
  assert.match(dts, /friction: number \| null;/);
  assert.match(dts, /export interface Leaf extends Base \{/);
  assert.match(dts, /readonly controller: EntityRef \| null;/);  // handle → readonly
  assert.match(dts, /scoped: boolean \| null;/);
  assert.doesNotMatch(dts, /origin/);                  // Vector skipped
  assert.doesNotMatch(dts, /m_vecOrigin/);
});

test("emitJs: char[N] emits readString(len); 64-bit int emits a null-safe readUInt64().toString()", () => {
  const CATALOG = { Base: { parent: null, fields: [
    { name: "m_iszName", offset: 8, type: { kind: "unknown", name: "char[64]" } },
    { name: "m_steamID", offset: 16, type: { kind: "atomic", name: "uint64" } },
  ] } };
  const model = buildModel(CATALOG, ["Base"]);
  const js = emitJs(model);
  assert.match(js, /readString\(\(O\[\d+\] >= 0 \? O\[\d+\] : \(O\[\d+\] = off\("Base","m_iszName"\)\)\), 64\)/);
  // 64-bit int -> decimal string: reads the bigint primitive, null-guards, stringifies.
  assert.match(js, /var v = this\.ref\.readUInt64\(\(O\[\d+\] >= 0 \? O\[\d+\] : \(O\[\d+\] = off\("Base","m_steamID"\)\)\)\); return v === null \? null : v\.toString\(\);/);
  // and the .d.ts types the 64-bit int as string (SM-parity), NOT bigint:
  const dts = emitDts(model);
  assert.match(dts, /steamID[^;\n]*: string \| null/);
  assert.doesNotMatch(dts, /steamID[^;\n]*: bigint/);
});

// The module is shared across plugins from the framework realm, so a vector is built by the
// receiver (in the reading plugin's realm), never by a `new QAngle` inside the module.
test("emitJs: a Vector/QAngle field reads through the receiver's readQAngle; no math require", () => {
  const CATALOG = { Base: { parent: null, fields: [
    { name: "m_angEyeAngles", offset: 8, type: { kind: "atomic", name: "QAngle" } },
  ] } };
  const js = emitJs(buildModel(CATALOG, ["Base"]));
  assert.doesNotMatch(js, /__s2require/);
  assert.match(js, /get: function \(\) \{ return this\.ref\.readQAngle\(\(O\[\d+\] >= 0 \? O\[\d+\] : \(O\[\d+\] = off\("Base","m_angEyeAngles"\)\)\)\); \}/);
});

test("emitDts: a Vector/QAngle field adds the @s2script/sdk/math import + the field type", () => {
  const CATALOG = { Base: { parent: null, fields: [
    { name: "m_vecAbsVelocity", offset: 8, type: { kind: "atomic", name: "Vector" } },
  ] } };
  const dts = emitDts(buildModel(CATALOG, ["Base"]));
  assert.match(dts, /import type \{ Vector \} from "@s2script\/sdk\/math";/);
  assert.match(dts, /absVelocity: Vector \| null;/);
});

test("emits a typed wrapEntity keyed on entity classes only", () => {
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
      { name: "m_Glow", offset: 64, type: { kind: "class", name: "CGlowProperty" } },
    ] },
    CGlowProperty: { parent: null, fields: [{ name: "m_bGlowing", offset: 81, type: { kind: "atomic", name: "bool" } }] },
  };
  const m = buildModel(catalog, ["Base"]);
  const dts = emitDts(m);

  assert.match(dts, /export interface SchemaClasses \{\n  Base: Base;\n\}/);
  // An embedded struct has no entity of its own -- only a base offset inside one -- so it must not
  // appear as something you can hand an EntityRef to.
  assert.doesNotMatch(dts, /CGlowProperty: CGlowProperty;/);
  assert.match(dts, /export declare function wrapEntity<K extends keyof SchemaClasses>/);

  // And it has to actually exist at runtime, on the package object plugins import from.
  assert.match(emitJs(m), /globalThis\.__s2pkg_cs2 = Object\.assign\(\{\}, globalThis\.__s2pkg_cs2, \{ wrapEntity: M\.wrap, structView: M\.structView \}, M\.enums\)/);
  assert.match(dts, /export declare function structView<K extends keyof SchemaClasses>\(className: K\): \(ptr: Ptr\) => SchemaClasses\[K\];/);
  assert.match(dts, /import type \{ Ptr \} from "@s2script\/sdk\/unsafe";/);
});

// Stage 3: views share one prototype per class (accessors are no longer redefined on every wrap or
// every embedded read), and structView hands the same accessors any ref-shaped object, such as a
// raw Ptr, so one generator serves checked entity views and unchecked raw views.
test("views share a per-class prototype; structView wraps any ref", async () => {
  const vm = await import("node:vm");
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
      { name: "m_Glow", offset: 64, type: { kind: "class", name: "CGlowProperty" } },
    ] },
    CGlowProperty: { parent: null, fields: [{ name: "m_bGlowing", offset: 81, type: { kind: "atomic", name: "bool" } }] },
  };
  const m = buildModel(catalog, ["Base"], ["CGlowProperty"]);
  const reads = [];
  const ref = { readInt32: (o) => (reads.push(["i32", o]), 42), readBool: (o) => (reads.push(["bool", o]), true) };
  const ctx = {
    __s2_schema_offset: (cls, f) => catalog[cls].fields.find((x) => x.name === f).offset,
    __s2require: () => ({ Vector: function () {}, QAngle: function () {} }),
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(emitJs(m), ctx);
  const { wrapEntity, structView } = ctx.__s2pkg_cs2;
  const a = wrapEntity("Base", ref), b = wrapEntity("Base", ref);
  assert.strictEqual(Object.getPrototypeOf(a), Object.getPrototypeOf(b), "one prototype per class");
  assert.deepStrictEqual(Object.keys(a), ["ref"], "accessors live on the prototype, not on each view");
  assert.strictEqual(a.health, 42);
  assert.strictEqual(Object.getPrototypeOf(a.glow), Object.getPrototypeOf(b.glow), "embedded views share one too");
  assert.strictEqual(a.glow.glowing, true);
  assert.strictEqual(structView("Base"), structView("Base"), "one factory per class");
  assert.strictEqual(structView("Base")(ref).health, 42);
  assert.deepStrictEqual(reads, [["i32", 8], ["bool", 145], ["i32", 8]]);
});

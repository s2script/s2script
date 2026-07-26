import { test } from "node:test";
import assert from "node:assert";
import { buildNavModel, SUPPORTED_NAV_KINDS } from "../src/navgen/model.ts";
import { emitNavJs } from "../src/navgen/emit-js.ts";
import { emitNavDts } from "../src/navgen/emit-dts.ts";

const CAT = {
  CCSPlayerPawn: { parent: "CBaseEntity", fields: [] },
  CBaseEntity: { parent: null, fields: [{ name: "m_CBodyComponent", offset: 48, type: { kind: "ptr", inner: "CBodyComponent" } }] },
  CBodyComponent: { parent: null, fields: [{ name: "m_pSceneNode", offset: 8, type: { kind: "ptr", inner: "CGameSceneNode" } }] },
  CGameSceneNode: { parent: null, fields: [
    { name: "m_flScale", offset: 160, type: { kind: "atomic", name: "float32" } },
    { name: "m_vecOrigin", offset: 200, type: { kind: "atomic", name: "VectorWS" } },
    { name: "m_bDormant", offset: 228, type: { kind: "atomic", name: "bool" } },
    { name: "m_pParent", offset: 56, type: { kind: "ptr", inner: "CGameSceneNode" } },   // skipped (ptr)
  ] },
};
const CONFIG = [{ prop: "sceneNode", wrapper: "SceneNode", target: "CGameSceneNode", source: "CCSPlayerPawn",
  path: [{ cls: "CBaseEntity", field: "m_CBodyComponent" }, { cls: "CBodyComponent", field: "m_pSceneNode" }] }];

test("buildNavModel builds a wrapper's readable fields (scalars+vector; skips ptr)", () => {
  const m = buildNavModel(CONFIG, CAT);
  const w = m.wrappers.find(x => x.wrapper === "SceneNode");
  const props = w.fields.map(f => f.propName).sort();
  assert.deepEqual(props, ["dormant", "origin", "scale"]);   // m_pParent (ptr) skipped
  assert.equal(w.fields.find(f => f.propName === "scale").accessorKind, "f32");
  assert.equal(w.fields.find(f => f.propName === "origin").accessorKind, "vector");
});

test("emitNavJs: wrapper getters read via the chain; nav accessor resolves the path", () => {
  const js = emitNavJs(buildNavModel(CONFIG, CAT));
  assert.match(js, /function SceneNode\(root, path, base\)/);
  assert.match(js, /this\.root\.readFloat32Via\(this\.path, off\("CGameSceneNode","m_flScale"\) \+ this\.base\)/);
  assert.match(js, /var a = this\.root\.readFloatsChain\(this\.path, off\("CGameSceneNode","m_vecOrigin"\) \+ this\.base, 3\); return a === null \? null : new Vector/);
  // the nav accessor + per-access hop resolution (boot-window-safe, no baked NAV table):
  assert.match(js, /var o0 = off\("CBaseEntity","m_CBodyComponent"\); if \(o0 < 0\) return null; _p\.push\(o0\);/);
  assert.match(js, /var o1 = off\("CBodyComponent","m_pSceneNode"\); if \(o1 < 0\) return null; _p\.push\(o1\);/);
  assert.match(js, /globalThis\.__s2pkg_cs2_nav = \{ applyNav/);
});

test("emitNavDts: a wrapper interface + the nav prop type", () => {
  const dts = emitNavDts(buildNavModel(CONFIG, CAT));
  assert.match(dts, /export interface SceneNode \{/);
  assert.match(dts, /readonly scale: number \| null;/);
  assert.match(dts, /readonly origin: Vector \| null;/);
  // (the nav prop `sceneNode: SceneNode | null` is declared on Pawn in index.d.ts by T3, not here.)
});

// ---- SUPPORTED_NAV_KINDS filter tests ----

const CAT_UNSUPPORTED = {
  CCSPlayerPawn: { parent: "CBaseEntity", fields: [] },
  CBaseEntity: { parent: null, fields: [{ name: "m_pNavTarget", offset: 48, type: { kind: "ptr", inner: "CNavTarget" } }] },
  CNavTarget: { parent: null, fields: [
    { name: "m_nCount", offset: 0, type: { kind: "atomic", name: "int32" } },        // supported: i32
    { name: "m_flPrecision", offset: 8, type: { kind: "atomic", name: "float64" } }, // unsupported: f64
    { name: "m_szName", offset: 16, type: { kind: "unknown", name: "char[64]" } },     // unsupported: str
  ] },
};
const CONFIG_UNSUPPORTED = [{ prop: "navTarget", wrapper: "NavTarget", target: "CNavTarget", source: "CCSPlayerPawn",
  path: [{ cls: "CBaseEntity", field: "m_pNavTarget" }] }];

test("buildNavModel: SUPPORTED_NAV_KINDS set excludes f64 and str", () => {
  assert.ok(!SUPPORTED_NAV_KINDS.has("f64"), "f64 must not be in SUPPORTED_NAV_KINDS");
  assert.ok(!SUPPORTED_NAV_KINDS.has("str"), "str must not be in SUPPORTED_NAV_KINDS");
  assert.ok(SUPPORTED_NAV_KINDS.has("i32"), "i32 must be in SUPPORTED_NAV_KINDS");
  assert.ok(SUPPORTED_NAV_KINDS.has("f32"), "f32 must be in SUPPORTED_NAV_KINDS");
  assert.ok(SUPPORTED_NAV_KINDS.has("handle"), "handle must be in SUPPORTED_NAV_KINDS");
  assert.ok(SUPPORTED_NAV_KINDS.has("vector"), "vector must be in SUPPORTED_NAV_KINDS");
  assert.ok(SUPPORTED_NAV_KINDS.has("qangle"), "qangle must be in SUPPORTED_NAV_KINDS");
});

test("buildNavModel: filters out f64 and str fields; keeps supported kinds; records skippedKinds", () => {
  const m = buildNavModel(CONFIG_UNSUPPORTED, CAT_UNSUPPORTED);
  const w = m.wrappers.find(x => x.wrapper === "NavTarget");
  assert.ok(w, "NavTarget wrapper must exist");

  // Only the i32 field should survive
  assert.equal(w.fields.length, 1, "only the i32 field survives the filter");
  assert.equal(w.fields[0].accessorKind, "i32");
  assert.equal(w.fields[0].propName, "count");

  // f64 and str are recorded in skippedKinds
  assert.equal(w.skippedKinds.length, 2, "two fields skipped (f64 + str)");
  const skippedKindValues = w.skippedKinds.map(s => s.accessorKind).sort();
  assert.deepEqual(skippedKindValues, ["f64", "str"]);
});

test("emitNavJs and emitNavDts agree on field count when unsupported kinds are present", () => {
  const m = buildNavModel(CONFIG_UNSUPPORTED, CAT_UNSUPPORTED);
  const js = emitNavJs(m);
  const dts = emitNavDts(m);

  // JS emitter: only 'count' getter (i32)
  assert.match(js, /"count":\s*\{/);
  assert.doesNotMatch(js, /"precision"/, "f64 field must not appear in JS");
  assert.doesNotMatch(js, /"name"/, "str field must not appear in JS");

  // DTS emitter: only 'count' property
  assert.match(dts, /readonly count: number \| null;/);
  assert.doesNotMatch(dts, /readonly precision/, "f64 field must not appear in d.ts");
  assert.doesNotMatch(dts, /readonly name/, "str field must not appear in d.ts");
});

// ---------------------------------------------------------------------------
// nav-targets `writable` allowlist. Writability is opt-in per field because which byte a field
// lives at is regenerable layout, but whether writing it is SAFE is a behavioural fact — the
// catalog's own `writable` flag would happily expose engine bookkeeping like m_nTraceCount.
// ---------------------------------------------------------------------------

const CAT_W = {
  CCSPlayerPawn: { parent: null, fields: [] },
  CMoveServices: { parent: null, fields: [
    { name: "m_flMaxspeed", offset: 16, type: { kind: "atomic", name: "float32" } },
    { name: "m_bDucked", offset: 20, type: { kind: "atomic", name: "bool" } },
    { name: "m_nTraceCount", offset: 24, type: { kind: "atomic", name: "int32" } },
    { name: "m_nBigMask", offset: 32, type: { kind: "atomic", name: "uint64" } },   // no write*Via
  ] },
};
const cfgW = (writable) => [{ prop: "mv", wrapper: "Move", target: "CMoveServices", source: "CCSPlayerPawn",
  path: [{ cls: "CCSPlayerPawn", field: "m_pMove" }], ...(writable ? { writable } : {}) }];

test("writable allowlist: only listed fields get a setter; the rest stay read-only", () => {
  const m = buildNavModel(cfgW(["m_flMaxspeed", "m_bDucked"]), CAT_W);
  const w = m.wrappers[0];
  assert.equal(w.fields.find(f => f.rawName === "m_flMaxspeed").navWritable, true);
  assert.equal(w.fields.find(f => f.rawName === "m_bDucked").navWritable, true);
  assert.equal(w.fields.find(f => f.rawName === "m_nTraceCount").navWritable, false,
    "an unlisted field must stay read-only even though its KIND is writable");

  const js = emitNavJs(m);
  assert.match(js, /"maxspeed": \{ get: [^}]*\}, set: function \(v\) \{ this\.root\.writeFloat32Via\(this\.path, off\("CMoveServices","m_flMaxspeed"\) \+ this\.base, v\); \}/);
  assert.match(js, /"ducked": \{ get: [^}]*\}, set: function \(v\) \{ this\.root\.writeBoolVia\(/);
  assert.doesNotMatch(js, /"traceCount": \{ get: [^}]*\}, set:/, "unlisted field must emit no setter");

  const dts = emitNavDts(m);
  assert.match(dts, /^  maxspeed: number \| null;$/m, "writable field loses `readonly`");
  assert.match(dts, /^  readonly traceCount: number \| null;$/m, "unlisted field keeps `readonly`");
});

test("writable allowlist: with no allowlist at all, the wrapper is entirely read-only", () => {
  const m = buildNavModel(cfgW(null), CAT_W);
  assert.equal(m.wrappers[0].fields.some(f => f.navWritable), false);
  assert.doesNotMatch(emitNavJs(m), /set: function/);
  // and the interface-level write caveat is not emitted for a read-only wrapper
  assert.doesNotMatch(emitNavDts(m), /Fields NOT marked/);
});

test("writable allowlist: a field name that does not exist FAILS generation", () => {
  // The CS2-update case: the field was renamed, so the allowlist silently matches nothing.
  assert.throws(() => buildNavModel(cfgW(["m_flMaxSpeed"]), CAT_W),
    /does not exist on CMoveServices or any of its ancestors/);
});

test("writable allowlist: a kind with no write*Via FAILS generation", () => {
  assert.throws(() => buildNavModel(cfgW(["m_nBigMask"]), CAT_W),
    /no EntityRef\.write\*Via for that kind/);
});

// --- embedded base hops ----------------------------------------------------

const BASE_CAT = {
  Owner: { parent: null, fields: [{ name: "m_pSvc", offset: 2760, type: { kind: "ptr", inner: "Svc" } }] },
  Svc: { parent: null, fields: [{ name: "m_stats", offset: 208, type: { kind: "class", name: "Stats" } }] },
  Stats: { parent: null, fields: [{ name: "m_iKills", offset: 48, type: { kind: "atomic", name: "int32" } }] },
};
const BASE_CFG = [{
  prop: "matchStats", wrapper: "MatchStats", source: "Owner", target: "Stats",
  path: [{ cls: "Owner", field: "m_pSvc" }],
  base: [{ cls: "Svc", field: "m_stats" }],
  writable: ["m_iKills"],
}];

test("a base hop is SUMMED into the field offset, never dereferenced", () => {
  const js = emitNavJs(buildNavModel(BASE_CFG, BASE_CAT));
  // The pointer hop goes on the path; the embedded hop goes on the offset. Getting this backwards
  // would dereference the middle of the services object as if it were a pointer.
  assert.match(js, /var o0 = off\("Owner","m_pSvc"\); if \(o0 < 0\) return null; _p\.push\(o0\);/);
  assert.match(js, /var b0 = off\("Svc","m_stats"\); if \(b0 < 0\) return null; _b \+= b0;/);
  assert.match(js, /off\("Stats","m_iKills"\) \+ this\.base/);
});

test("a base lookup that fails yields null rather than reading offset 0", () => {
  const js = emitNavJs(buildNavModel(BASE_CFG, BASE_CAT));
  // Falling through as 0 would silently read the START of the services object instead of the
  // embedded struct — plausible-looking numbers from the wrong field.
  assert.match(js, /if \(b0 < 0\) return null/);
});

test("wrappers with no base hop still receive an explicit 0", () => {
  const cfg = [{ prop: "svc", wrapper: "Svc2", source: "Owner", target: "Stats", path: [{ cls: "Owner", field: "m_pSvc" }] }];
  const js = emitNavJs(buildNavModel(cfg, BASE_CAT));
  // `this.base` is in every field expression, so omitting the argument would make it undefined and
  // turn every offset into NaN.
  assert.match(js, /return new Svc2\(this\.ref, _p, 0\)/);
  assert.doesNotMatch(js, /return new Svc2\(this\.ref, _p\)/);
});

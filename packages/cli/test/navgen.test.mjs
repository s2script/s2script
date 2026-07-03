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
  assert.match(js, /function SceneNode\(root, path\)/);
  assert.match(js, /this\.root\.readFloat32Via\(this\.path, off\("CGameSceneNode","m_flScale"\)\)/);
  assert.match(js, /var a = this\.root\.readFloatsChain\(this\.path, off\("CGameSceneNode","m_vecOrigin"\), 3\); return a === null \? null : new Vector/);
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

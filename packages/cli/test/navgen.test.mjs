import { test } from "node:test";
import assert from "node:assert";
import { buildNavModel } from "../src/navgen/model.ts";
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
  // the nav accessor + its per-hop path resolution:
  assert.match(js, /off\("CBaseEntity","m_CBodyComponent"\)/);
  assert.match(js, /off\("CBodyComponent","m_pSceneNode"\)/);
  assert.match(js, /globalThis\.__s2pkg_cs2_nav = \{ applyNav/);
});

test("emitNavDts: a wrapper interface + the nav prop type", () => {
  const dts = emitNavDts(buildNavModel(CONFIG, CAT));
  assert.match(dts, /export interface SceneNode \{/);
  assert.match(dts, /readonly scale: number \| null;/);
  assert.match(dts, /readonly origin: Vector \| null;/);
  // (the nav prop `sceneNode: SceneNode | null` is declared on Pawn in index.d.ts by T3, not here.)
});

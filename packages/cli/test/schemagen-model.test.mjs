import { test } from "node:test";
import assert from "node:assert";
import { idiomaticName, classifyField, buildModel, flattenedFields } from "../src/schemagen/model.ts";

test("idiomaticName strips m_ + Hungarian tag, camelCases", () => {
  assert.equal(idiomaticName("m_iHealth"), "health");
  assert.equal(idiomaticName("m_flFriction"), "friction");
  assert.equal(idiomaticName("m_hController"), "controller");
  assert.equal(idiomaticName("m_bClientSideRagdoll"), "clientSideRagdoll");
  assert.equal(idiomaticName("m_ArmorValue"), "armorValue");   // no lowercase tag
  assert.equal(idiomaticName("m_flags"), "flags");             // all-lowercase, no uppercase boundary → unchanged
});

test("classifyField maps in-scope kinds, skips the rest with a reason", () => {
  assert.deepEqual(classifyField({ kind: "atomic", name: "float32" }), { accessorKind: "f32", writable: true });
  assert.deepEqual(classifyField({ kind: "atomic", name: "bool" }), { accessorKind: "bool", writable: true });
  assert.deepEqual(classifyField({ kind: "atomic", name: "int32" }), { accessorKind: "i32", writable: true });
  assert.deepEqual(classifyField({ kind: "atomic", name: "uint8" }), { accessorKind: "u8", writable: false });
  assert.deepEqual(classifyField({ kind: "handle", inner: "CBaseEntity" }), { accessorKind: "handle", writable: false });
  assert.ok("skip" in classifyField({ kind: "enum", name: "Team_t" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "CUtlSymbolLarge" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Vector" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "uint64" }));
  assert.ok("skip" in classifyField({ kind: "class", name: "CTransform" }));
  assert.ok("skip" in classifyField({ kind: "ptr" }));
  assert.ok("skip" in classifyField({ kind: "unknown" }));
});

test("buildModel: closure includes ancestors, own fields per class, skips logged, parent flatten", () => {
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
      { name: "m_vecStuff", offset: 12, type: { kind: "atomic", name: "Vector" } },   // skipped
    ] },
    Mid: { parent: "Base", fields: [
      { name: "m_hOwner", offset: 20, type: { kind: "handle", inner: "Base" } },
    ] },
    Leaf: { parent: "Mid", fields: [
      { name: "m_flSpeed", offset: 24, type: { kind: "atomic", name: "float32" } },
    ] },
  };
  const m = buildModel(catalog, ["Leaf"]);
  // closure = Base, Mid, Leaf ; topo order root→leaf
  assert.deepEqual(m.classes.map(c => c.className), ["Base", "Mid", "Leaf"]);
  const base = m.classes.find(c => c.className === "Base");
  assert.deepEqual(base.ownFields.map(f => f.propName), ["health"]);      // Vector skipped
  assert.equal(base.ownFields[0].declaringClass, "Base");
  assert.equal(base.ownFields[0].writable, true);
  assert.equal(base.skipped.length, 1);
  assert.equal(base.skipped[0].rawName, "m_vecStuff");
  // flatten Leaf = Base.health + Mid.owner + Leaf.speed (root→leaf)
  assert.deepEqual(flattenedFields(m, "Leaf").map(f => f.propName), ["health", "owner", "speed"]);
  assert.equal(flattenedFields(m, "Leaf").find(f => f.propName === "owner").accessorKind, "handle");
});

test("buildModel: idiomatic-name collision across distinct fields → both fall back to raw", () => {
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
      { name: "m_flHealth", offset: 12, type: { kind: "atomic", name: "float32" } },   // also → "health"
    ] },
  };
  const m = buildModel(catalog, ["Base"]);
  const names = m.classes[0].ownFields.map(f => f.propName).sort();
  assert.deepEqual(names, ["m_flHealth", "m_iHealth"]);   // both raw-fallback
  assert.equal(m.collisions.length, 1);
});

test("buildModel: a requested class absent from the catalog is a hard error", () => {
  assert.throws(() => buildModel({ Base: { parent: null, fields: [] } }, ["Nope"]), /Nope/);
});

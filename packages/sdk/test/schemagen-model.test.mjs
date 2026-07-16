import { test } from "node:test";
import assert from "node:assert";
import { idiomaticName, classifyField, buildModel, flattenedFields, TSTYPE } from "../src/schemagen/model.ts";

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
  assert.deepEqual(classifyField({ kind: "atomic", name: "uint8" }), { accessorKind: "u8", writable: true });
  assert.deepEqual(classifyField({ kind: "handle", inner: "CBaseEntity" }), { accessorKind: "handle", writable: false });
  assert.ok("skip" in classifyField({ kind: "enum", name: "Team_t" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "CUtlSymbolLarge" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Vector2D" }));  // Vector2D/4D deferred
  // uint64/int64/float64 are now supported (not skipped) — see the new test below
  assert.ok("skip" in classifyField({ kind: "class", name: "CTransform" }));
  assert.ok("skip" in classifyField({ kind: "ptr" }));
  assert.ok("skip" in classifyField({ kind: "unknown" }));
});

test("buildModel: closure includes ancestors, own fields per class, skips logged, parent flatten", () => {
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
      { name: "m_vecStuff", offset: 12, type: { kind: "atomic", name: "Vector2D" } },   // skipped (Vector2D deferred)
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

test("idiomaticName strips only KNOWN Hungarian tags (steamID/bombSite fixed)", () => {
  assert.equal(idiomaticName("m_iHealth"), "health");         // i ∈ tags
  assert.equal(idiomaticName("m_flFriction"), "friction");    // fl ∈ tags
  assert.equal(idiomaticName("m_hController"), "controller");  // h ∈ tags
  assert.equal(idiomaticName("m_iszPlayerName"), "playerName");// isz ∈ tags
  assert.equal(idiomaticName("m_steamID"), "steamID");        // "steam" ∉ tags → kept (was "iD")
  assert.equal(idiomaticName("m_bombSite"), "bombSite");      // "bomb" ∉ tags → kept (was "site")
  assert.equal(idiomaticName("m_flags"), "flags");            // no uppercase core → unchanged
});

test("classifyField maps 64-bit + char[N], skips other unknowns", () => {
  assert.deepEqual(classifyField({ kind: "atomic", name: "uint64" }), { accessorKind: "u64", writable: false });
  assert.deepEqual(classifyField({ kind: "atomic", name: "int64" }), { accessorKind: "i64", writable: false });
  assert.deepEqual(classifyField({ kind: "atomic", name: "float64" }), { accessorKind: "f64", writable: false });
  assert.deepEqual(classifyField({ kind: "unknown", name: "char[128]" }), { accessorKind: "str", writable: false, strLen: 128 });
  assert.ok("skip" in classifyField({ kind: "unknown", name: "CUtlSomething" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "CUtlSymbolLarge" }));
});

test("buildModel threads strLen onto a char[N] field descriptor", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_iszName", offset: 8, type: { kind: "unknown", name: "char[64]" } },
    { name: "m_steamID", offset: 16, type: { kind: "atomic", name: "uint64" } },
  ] } };
  const m = buildModel(catalog, ["Base"]);
  const f = m.classes[0].ownFields.find(x => x.rawName === "m_iszName");
  assert.equal(f.propName, "name");           // isz stripped
  assert.equal(f.accessorKind, "str");
  assert.equal(f.strLen, 64);
  const sid = m.classes[0].ownFields.find(x => x.rawName === "m_steamID");
  assert.equal(sid.propName, "steamID");
  assert.equal(sid.accessorKind, "u64");
});

test("classifyField maps Vector/QAngle atomics to vector/qangle kinds", () => {
  assert.deepEqual(classifyField({ kind: "atomic", name: "Vector" }), { accessorKind: "vector", writable: false });
  assert.deepEqual(classifyField({ kind: "atomic", name: "QAngle" }), { accessorKind: "qangle", writable: false });
  // an unmapped vector-ish atomic still skips (Vector2D/Color/Quaternion deferred):
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Vector2D" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Color" }));
});

test("buildModel emits a vector/qangle field with the right kind + TS type", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_vecAbsVelocity", offset: 8, type: { kind: "atomic", name: "Vector" } },
    { name: "m_angEyeAngles", offset: 24, type: { kind: "atomic", name: "QAngle" } },
  ] } };
  const m = buildModel(catalog, ["Base"]);
  const vel = m.classes[0].ownFields.find(x => x.rawName === "m_vecAbsVelocity");
  assert.equal(vel.propName, "absVelocity");     // vec ∈ tags stripped
  assert.equal(vel.accessorKind, "vector");
  assert.equal(TSTYPE.vector, "Vector | null");
  const ang = m.classes[0].ownFields.find(x => x.rawName === "m_angEyeAngles");
  assert.equal(ang.propName, "eyeAngles");        // ang ∈ tags stripped
  assert.equal(ang.accessorKind, "qangle");
  assert.equal(TSTYPE.qangle, "QAngle | null");
});

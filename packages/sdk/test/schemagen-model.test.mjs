import { test } from "node:test";
import assert from "node:assert";
import { idiomaticName, classifyField, buildModel, flattenedFields, transparentValue, enumMemberNames, enumTsName, TSTYPE } from "../src/schemagen/model.ts";

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
  // an unmapped vector-ish atomic still skips (Vector2D/Quaternion deferred):
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Vector2D" }));
});

test("Color maps to a writable uint32 (packed RGBA, R in the low byte)", () => {
  assert.deepEqual(classifyField({ kind: "atomic", name: "Color" }), { accessorKind: "u32", writable: true });
});

test("class-kind fields skip by default and embed only when allow-listed", () => {
  const glow = { kind: "class", name: "CGlowProperty" };
  assert.ok("skip" in classifyField(glow));
  assert.deepEqual(classifyField(glow, new Set(["CGlowProperty"])), { embedded: "CGlowProperty" });
  // allow-listing one struct must not drag in every other embedded class:
  assert.ok("skip" in classifyField({ kind: "class", name: "CCollisionProperty" }, new Set(["CGlowProperty"])));
});

test("buildModel exposes an allow-listed embedded struct as a nested descriptor", () => {
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_Glow", offset: 2424, type: { kind: "class", name: "CGlowProperty" } },
      { name: "m_Collision", offset: 100, type: { kind: "class", name: "CCollisionProperty" } },
    ] },
    CGlowProperty: { parent: null, fields: [
      { name: "m_bGlowing", offset: 81, type: { kind: "atomic", name: "bool" } },
      { name: "m_glowColorOverride", offset: 64, type: { kind: "atomic", name: "Color" } },
    ] },
    CCollisionProperty: { parent: null, fields: [] },
  };
  const m = buildModel(catalog, ["Base"], ["CGlowProperty"]);
  const base = m.classes.find((c) => c.className === "Base");

  // The allow-listed one becomes an embedded field; the other stays skipped.
  assert.deepEqual(base.embeddedFields.map((f) => [f.propName, f.embeddedClass]), [["glow", "CGlowProperty"]]);
  assert.ok(base.skipped.some((s) => s.rawName === "m_Collision"));
  // ...and it is NOT also emitted as a plain scalar field.
  assert.equal(base.ownFields.find((f) => f.rawName === "m_Glow"), undefined);

  // Only referenced structs are emitted, with their own fields classified normally.
  assert.deepEqual(m.embedded.map((e) => e.className), ["CGlowProperty"]);
  assert.deepEqual(
    m.embedded[0].fields.map((f) => [f.propName, f.accessorKind, f.writable]),
    [["glowing", "bool", true], ["glowColorOverride", "u32", true]],
  );
});

test("buildModel rejects an embedded class that is not in the catalog", () => {
  assert.throws(() => buildModel({ Base: { parent: null, fields: [] } }, ["Base"], ["CNope"]), /not in the catalog/);
});

test("an allow-listed struct nothing embeds produces no descriptor", () => {
  const catalog = {
    Base: { parent: null, fields: [{ name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } }] },
    CGlowProperty: { parent: null, fields: [] },
  };
  assert.deepEqual(buildModel(catalog, ["Base"], ["CGlowProperty"]).embedded, []);
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

// --- transparent value wrappers -------------------------------------------

const WRAPPERS = {
  GameTime_t: { parent: null, fields: [{ name: "m_Value", offset: 0, type: { kind: "atomic", name: "float32" } }] },
  GameTick_t: { parent: null, fields: [{ name: "m_Value", offset: 0, type: { kind: "atomic", name: "int32" } }] },
  // Single-field, but the field carries its own meaning -> NOT transparent.
  CHitboxComponent: { parent: null, fields: [{ name: "m_flBoundsExpandRadius", offset: 20, type: { kind: "atomic", name: "float32" } }] },
};

test("transparentValue accepts only an anonymous single m_Value", () => {
  assert.equal(transparentValue(WRAPPERS, "GameTime_t").name, "m_Value");
  assert.equal(transparentValue(WRAPPERS, "GameTick_t").name, "m_Value");
  // A named single field is not anonymous enough to absorb -- flattening it would produce a
  // property called "hitboxComponent" holding a radius.
  assert.equal(transparentValue(WRAPPERS, "CHitboxComponent"), null);
  assert.equal(transparentValue(WRAPPERS, "Nope"), null);
});

test("a wrapper field flattens to the scalar it wraps, not a nested object", () => {
  const catalog = {
    ...WRAPPERS,
    Base: { parent: null, fields: [
      { name: "m_flDeathTime", offset: 100, type: { kind: "class", name: "GameTime_t" } },
      { name: "m_nNextThinkTick", offset: 108, type: { kind: "class", name: "GameTick_t" } },
    ] },
  };
  const m = buildModel(catalog, ["Base"]);
  const base = m.classes.find((c) => c.className === "Base");
  assert.deepEqual(base.embeddedFields, []);                 // no nested object
  assert.deepEqual(base.ownFields.map((f) => [f.propName, f.accessorKind, f.writable]),
    [["deathTime", "f32", true], ["nextThinkTick", "i32", true]]);
  // Wrappers put m_Value at +0 here, so no delta is carried.
  assert.equal(m.embedded.length, 0);
});

test("a wrapper whose value is NOT at +0 carries the delta", () => {
  const catalog = {
    Off_t: { parent: null, fields: [{ name: "m_Value", offset: 8, type: { kind: "atomic", name: "int32" } }] },
    Base: { parent: null, fields: [{ name: "m_thing", offset: 64, type: { kind: "class", name: "Off_t" } }] },
  };
  const f = buildModel(catalog, ["Base"]).classes[0].ownFields[0];
  assert.equal(f.addOffset, 8);
});

// --- auto-embed + recursion ------------------------------------------------

const NESTED = {
  Inner: { parent: null, fields: [{ name: "m_iCount", offset: 4, type: { kind: "atomic", name: "int32" } }] },
  Outer: { parent: null, fields: [{ name: "m_Inner", offset: 80, type: { kind: "class", name: "Inner" } }] },
  Base: { parent: null, fields: [{ name: "m_Outer", offset: 16, type: { kind: "class", name: "Outer" } }] },
};

test("with no allow-list every catalog struct embeds, transitively", () => {
  const m = buildModel(NESTED, ["Base"]);
  assert.deepEqual(m.embedded.map((e) => e.className), ["Inner", "Outer"]);
  // Outer reaches Inner as an embedded field of its own -- the mechanism nests.
  const outer = m.embedded.find((e) => e.className === "Outer");
  assert.deepEqual(outer.embeddedFields.map((f) => f.embeddedClass), ["Inner"]);
});

test("a non-empty allow-list still restricts which structs embed", () => {
  const m = buildModel(NESTED, ["Base"], ["Outer"]);
  assert.deepEqual(m.embedded.map((e) => e.className), ["Outer"]);
  // Inner is not allow-listed, so Outer's field skips rather than nesting.
  assert.deepEqual(m.embedded[0].embeddedFields, []);
  assert.ok(m.embedded[0].skipped.some((s) => s.rawName === "m_Inner"));
});

test("a self-referential struct terminates instead of recursing forever", () => {
  const catalog = {
    Loop: { parent: null, fields: [
      { name: "m_self", offset: 8, type: { kind: "class", name: "Loop" } },
      { name: "m_iX", offset: 16, type: { kind: "atomic", name: "int32" } },
    ] },
    Base: { parent: null, fields: [{ name: "m_loop", offset: 0, type: { kind: "class", name: "Loop" } }] },
  };
  const m = buildModel(catalog, ["Base"]);
  assert.deepEqual(m.embedded.map((e) => e.className), ["Loop"]);   // emitted exactly once
});

// --- collisions are per inheritance chain, not global ----------------------

const CHAIN = {
  Root:   { parent: null,   fields: [{ name: "m_flSpeed", offset: 8,  type: { kind: "atomic", name: "float32" } }] },
  Mid:    { parent: "Root", fields: [] },
  Leaf:   { parent: "Mid",  fields: [{ name: "m_fSpeed", offset: 12, type: { kind: "atomic", name: "float32" } }] },
  Sibling:{ parent: null,   fields: [{ name: "m_flSpeed", offset: 16, type: { kind: "atomic", name: "float32" } }] },
  Twice:  { parent: null,   fields: [
    { name: "m_iRecoilIndex",  offset: 8,  type: { kind: "atomic", name: "int32" } },
    { name: "m_flRecoilIndex", offset: 12, type: { kind: "atomic", name: "float32" } },
  ] },
};

test("an unrelated class sharing a name does NOT rename the existing property", () => {
  // The regression this guards: adding CTeam (m_iScore) once renamed
  // CCSPlayerController.score to m_iScore for every plugin already using it.
  const m = buildModel(CHAIN, ["Root", "Sibling"]);
  const root = m.classes.find((c) => c.className === "Root");
  const sib = m.classes.find((c) => c.className === "Sibling");
  assert.equal(root.ownFields[0].propName, "speed");
  assert.equal(sib.ownFields[0].propName, "speed");   // both keep it — they never share an object
  assert.deepEqual(m.collisions, []);
});

test("a real chain conflict is resolved ancestor-wins", () => {
  const m = buildModel(CHAIN, ["Leaf"]);
  const root = m.classes.find((c) => c.className === "Root");
  const leaf = m.classes.find((c) => c.className === "Leaf");
  // Leaf flattens Root's fields onto one object, so these two genuinely meet.
  assert.equal(root.ownFields[0].propName, "speed");        // ancestor keeps the good name
  assert.equal(leaf.ownFields[0].propName, "m_fSpeed");     // descendant falls back
  assert.equal(m.collisions.length, 1);
  assert.match(m.collisions[0], /kept by Root/);
});

test("two fields on the SAME class both fall back — no ancestor to break the tie", () => {
  const m = buildModel(CHAIN, ["Twice"]);
  assert.deepEqual(m.classes[0].ownFields.map((f) => f.propName).sort(),
    ["m_flRecoilIndex", "m_iRecoilIndex"]);
  assert.equal(m.collisions.length, 1);
  assert.doesNotMatch(m.collisions[0], /kept by/);
});

test("ancestor-wins makes naming independent of which descendants are generated", () => {
  // Root's property must be `speed` whether or not Leaf is in the class list -- otherwise the
  // generated API would shift under an unrelated addition.
  const withLeaf = buildModel(CHAIN, ["Root", "Leaf"]);
  const without = buildModel(CHAIN, ["Root"]);
  const nameIn = (m) => m.classes.find((c) => c.className === "Root").ownFields[0].propName;
  assert.equal(nameIn(withLeaf), nameIn(without));
});

// --- enums ------------------------------------------------------------------

test("an enum maps to an unsigned reader of its stated width", () => {
  // enumType rides along so the emitters can declare the field as the enum rather than a bare number.
  assert.deepEqual(classifyField({ kind: "enum", name: "Team_t", size: 1 }), { accessorKind: "u8", writable: true, enumType: "Team_t" });
  assert.deepEqual(classifyField({ kind: "enum", name: "MoveType_t", size: 2 }), { accessorKind: "u16", writable: true, enumType: "MoveType_t" });
  assert.deepEqual(classifyField({ kind: "enum", name: "SolidType_t", size: 4 }), { accessorKind: "u32", writable: true, enumType: "SolidType_t" });
  assert.deepEqual(classifyField({ kind: "enum", name: "Big_t", size: 8 }), { accessorKind: "u64", writable: false, enumType: "Big_t" });
});

test("an enum with no stated width still skips, and says why", () => {
  // The width is dumped from the live SchemaSystem, never assumed -- an enum whose binding did not
  // report one must skip rather than be guessed at.
  const noSize = classifyField({ kind: "enum", name: "Mystery_t" });
  assert.ok("skip" in noSize);
  assert.match(noSize.skip, /byte width not stated/);
  assert.ok("skip" in classifyField({ kind: "enum", name: "Odd_t", size: 3 }));
});

test("enum fields become real properties on the generated class", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_iTeamNum", offset: 8, type: { kind: "enum", name: "Team_t", size: 1 } },
    { name: "m_nUnbound", offset: 12, type: { kind: "enum", name: "Mystery_t" } },
  ] } };
  const m = buildModel(catalog, ["Base"]);
  assert.deepEqual(m.classes[0].ownFields.map((f) => [f.propName, f.accessorKind, f.writable]),
    [["teamNum", "u8", true]]);
  assert.ok(m.classes[0].skipped.some((s) => s.rawName === "m_nUnbound"));
});

// --- enumerator naming ------------------------------------------------------

test("a shared enum prefix is stripped", () => {
  // Case is PRESERVED, not converted. `MOVETYPE_VPHYSICS` has no unambiguous PascalCase form
  // (`Vphysics`? `VPhysics`?), so guessing would be lossy and could collide.
  assert.deepEqual(enumMemberNames("MoveType_t", ["MOVETYPE_NONE", "MOVETYPE_FLY"]),
    { MOVETYPE_NONE: "NONE", MOVETYPE_FLY: "FLY" });
  // the `kRenderNone` convention
  assert.deepEqual(enumMemberNames("RenderMode_t", ["kRenderNormal", "kRenderNone"]),
    { kRenderNormal: "RenderNormal", kRenderNone: "RenderNone" });
});

test("stripping is all-or-nothing per enum", () => {
  // One member that does not share the prefix keeps the WHOLE enum raw — a partly-stripped enum is
  // worse than an unstripped one, because the caller cannot predict which form a member takes.
  assert.deepEqual(enumMemberNames("MoveType_t", ["MOVETYPE_NONE", "SOMETHING_ELSE"]),
    { MOVETYPE_NONE: "MOVETYPE_NONE", SOMETHING_ELSE: "SOMETHING_ELSE" });
});

test("stripping that would collide keeps raw names", () => {
  // Both would become "X"; silently dropping one would lose a constant.
  assert.deepEqual(enumMemberNames("E_t", ["E_X", "EX"]), { E_X: "E_X", EX: "EX" });
});

test("a stripped name that would start with a digit is prefixed, not emitted invalid", () => {
  assert.deepEqual(enumMemberNames("Slot_t", ["SLOT_1", "SLOT_2"]), { SLOT_1: "_1", SLOT_2: "_2" });
});

test("an enum whose member IS the prefix keeps raw names", () => {
  // Stripping would leave an empty identifier.
  assert.deepEqual(enumMemberNames("Foo_t", ["FOO"]), { FOO: "FOO" });
});

test("a nested enum name is sanitised into a legal identifier", () => {
  // The schema names nested enums `CFuncMover::Move_t`. Emitting `::` produced a .d.ts that would
  // not parse at all — every consumer's typecheck failed, not just the enum's own users.
  assert.equal(enumTsName("CFuncMover::Move_t"), "CFuncMover__Move_t");
  assert.equal(enumTsName("MoveType_t"), "MoveType_t");
  assert.equal(enumTsName("9Lives"), "_9Lives");
});

test("enums that sanitise to the SAME identifier are dropped, not emitted ambiguously", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_a", offset: 8, type: { kind: "enum", name: "A::B_t", size: 1 } },
    { name: "m_b", offset: 12, type: { kind: "enum", name: "A:.B_t", size: 1 } },
    { name: "m_c", offset: 16, type: { kind: "enum", name: "Fine_t", size: 1 } },
  ] } };
  const enums = {
    "A::B_t": { size: 1, values: { X: 1 } },
    "A:.B_t": { size: 1, values: { Y: 2 } },
    "Fine_t": { size: 1, values: { Z: 3 } },
  };
  const m = buildModel(catalog, ["Base"], [], enums);
  // Both collapse to A__B_t; emitting either would silently bind the wrong constants.
  assert.deepEqual(m.enums.map((e) => e.tsName), ["Fine_t"]);
  assert.ok(m.collisions.some((c) => c.includes("ambiguous")));
  // The dropped ones fall back to plain integers rather than dangling on a missing type.
  const fa = m.classes[0].ownFields.find((f) => f.rawName === "m_a");
  assert.equal(fa.accessorKind, "u8");
});

test("only enums a generated field references are emitted", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_used", offset: 8, type: { kind: "enum", name: "Used_t", size: 1 } },
  ] } };
  const enums = { Used_t: { size: 1, values: { A: 1 } }, Unused_t: { size: 1, values: { B: 2 } } };
  const m = buildModel(catalog, ["Base"], [], enums);
  // The dump describes 500+ game enums; shipping constants nothing can be assigned to is noise.
  assert.deepEqual(m.enums.map((e) => e.className), ["Used_t"]);
});

test("an enum field with no dumped table stays a plain integer", () => {
  const catalog = { Base: { parent: null, fields: [
    { name: "m_x", offset: 8, type: { kind: "enum", name: "Undumped_t", size: 4 } },
  ] } };
  const m = buildModel(catalog, ["Base"], [], {});
  assert.deepEqual(m.enums, []);
  assert.equal(m.classes[0].ownFields[0].accessorKind, "u32");
});

import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const genJs = readFileSync(join(repo, "games/cs2/js/schema.generated.js"), "utf8");
const navJs = readFileSync(join(repo, "games/cs2/js/nav.generated.js"), "utf8");
const pawnJs = readFileSync(join(repo, "games/cs2/js/pawn.js"), "utf8");

test("Player model: fromSlot/all, generated accessors, .pawn + .controller nav (offline vm)", () => {
  // Stub EntityRef: isValid() true, typed reads return fixed values, readHandle returns a fresh ref (nav).
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readInt32 = function () { return 2; };
  EntityRef.prototype.readUInt8 = function () { return 2; };          // e.g. teamNum = 2 (uint8 in generated schema)
  EntityRef.prototype.readFloat32 = function () { return 0.25; };
  EntityRef.prototype.readBool = function () { return false; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); }; // a live nav target
  const stdEntity = { EntityRef };
  const math = { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                  QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } };
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? stdEntity : n === "@s2script/sdk/math" ? math : null),
    __s2_schema_offset: () => 8,               // any non-negative offset
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Player, Pawn } = ctx.__s2pkg_cs2;
  assert.equal(typeof Player, "function");

  const p = Player.fromSlot(0);                // controller ref (1, 7)
  assert.ok(p, "fromSlot(0) returns a Player");
  assert.equal(p.slot, 0, "slot is 0-based (ref.index - 1)");
  assert.equal(p.teamNum, 2, "generated CCSPlayerController accessor reads through the controller ref");
  const body = p.pawn;                         // readHandle(m_hPlayerPawn) -> a Pawn
  assert.ok(body, "player.pawn is a Pawn");
  assert.equal(body.health, 2, "the pawn's generated accessor reads through the pawn ref");
  const back = body.controller;               // readHandle(m_hController) -> a Player
  assert.ok(back, "pawn.controller is a Player");
  assert.equal(typeof back.teamNum, "number", "the round-tripped controller has generated accessors");

  const all = Player.all();                    // stub isValid() always true → 64 players
  assert.equal(all.length, 64, "Player.all() iterates the slot range and keeps valid controllers");
  assert.equal(all[0].slot, 0);
  assert.equal(all[63].slot, 63);
});

test("Player.fromSlot degrades to null when the controller is invalid (offline vm)", () => {
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return false; };   // invalid slot
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef }
      : n === "@s2script/sdk/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                                   QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } }
      : null),
    __s2_schema_offset: () => 8, __s2_ent_current_serial: () => -1, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Player } = ctx.__s2pkg_cs2;
  assert.equal(Player.fromSlot(0), null, "invalid controller → null");
  assert.deepEqual(Player.all(), [], "Player.all() is empty when no controller is valid");
});

test("schema.generated.js + pawn.js compose: Pawn.prototype has generated accessors", () => {
  const stdPkg = { EntityRef: function (i, s) { this.index = i; this.serial = s; } };
  stdPkg.EntityRef.prototype.isValid = function () { return true; };
  stdPkg.EntityRef.prototype.readInt32 = function () { return 100; };
  stdPkg.EntityRef.prototype.readFloat32 = function () { return 0.25; };
  stdPkg.EntityRef.prototype.readBool = function () { return false; };
  stdPkg.EntityRef.prototype.readHandle = function () { return new stdPkg.EntityRef(1, 7); };
  const ctx = {
    __s2require: (name) => (name === "@s2script/sdk/entity" ? stdPkg
      : name === "@s2script/sdk/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                                      QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } }
      : null),
    __s2_schema_offset: () => 100,
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);   // concatenation order: schema first, pawn second
  const Pawn = ctx.__s2pkg_cs2.Pawn;
  assert.equal(typeof Object.getOwnPropertyDescriptor(Pawn.prototype, "health").get, "function");
  assert.equal(typeof Object.getOwnPropertyDescriptor(Pawn.prototype, "friction").get, "function");
  const p = new Pawn(new stdPkg.EntityRef(5, 9));
  assert.equal(p.health, 100);
  assert.equal(p.friction, 0.25);
});

test("Player.fromSlot excludes a valid controller with no pawn (occupancy filter, offline vm)", () => {
  // A pre-allocated-but-empty controller: the entity is valid, but readHandle(m_hPlayerPawn) is null.
  // The occupancy filter (5C.2 live finding) must reject it — CS2 pre-allocates all 64 controllers.
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };    // controller entity exists...
  EntityRef.prototype.readHandle = function () { return null; };  // ...but has no player pawn (empty slot)
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef }
      : n === "@s2script/sdk/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                                   QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } }
      : null),
    __s2_schema_offset: () => 8, __s2_ent_current_serial: () => 7, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Player } = ctx.__s2pkg_cs2;
  assert.equal(Player.fromSlot(0), null, "valid controller but no pawn → not occupied → null");
  assert.deepEqual(Player.all(), [], "Player.all() excludes controllers without a pawn");
});

test("pawn.origin / pawn.angles: pointer-chain accessors read a value, degrade to null (offline vm)", () => {
  // nav.generated.js is now in the concatenation; origin/angles are compat aliases that delegate to sceneNode.
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  let chainRet = [64, 128, 256];
  // readFloatsChain must respect fieldOff so that offRet=-1 causes null (the nav wrapper passes fieldOff through).
  EntityRef.prototype.readFloatsChain = function (path, fieldOff) { return fieldOff >= 0 ? chainRet : null; };
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  const math = { Vector, QAngle };
  let offRet = 8;                                        // schema-offset stub; toggled to -1 below
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef } : n === "@s2script/sdk/math" ? math : null),
    __s2_schema_offset: () => offRet,
    __s2_ent_current_serial: () => 7, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + navJs + "\n" + pawnJs, ctx);   // addon order: schema, nav, pawn
  const { Pawn } = ctx.__s2pkg_cs2;
  const p = new Pawn(new EntityRef(5, 9));
  assert.ok(p.origin instanceof Vector, "origin is a Vector");
  assert.deepEqual([p.origin.x, p.origin.y, p.origin.z], [64, 128, 256]);
  assert.ok(p.angles instanceof QAngle, "angles is a QAngle");
  chainRet = null;                                      // stale/broken chain → readFloatsChain null
  assert.equal(p.origin, null, "a null readFloatsChain → the accessor returns null");
  chainRet = [1, 2, 3]; offRet = -1;                    // a field-offset miss → readFloatsChain returns null
  assert.equal(p.origin, null, "a missing field offset → the accessor returns null");
});

test("generated Vector/QAngle accessor: reads a value object, degrades to null (offline vm)", () => {
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  let floatsRet = [1, 2, 3];
  EntityRef.prototype.readFloats = function () { return floatsRet; };   // toggled to null below
  // minimal stub value types (match the real shape):
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  const math = { Vector, QAngle };
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef } : n === "@s2script/sdk/math" ? math : null),
    __s2_schema_offset: () => 8, __s2_ent_current_serial: () => 7, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Pawn } = ctx.__s2pkg_cs2;
  const p = new Pawn(new EntityRef(5, 9));
  const ang = p.eyeAngles;                          // generated QAngle accessor
  assert.ok(ang instanceof QAngle, "eyeAngles is a QAngle");
  assert.deepEqual([ang.x, ang.y, ang.z], [1, 2, 3]);
  floatsRet = null;                                 // stale ref → readFloats null
  assert.equal(p.eyeAngles, null, "a null readFloats → the accessor returns null");
});

test("nav.generated.js + pawn.js compose: sceneNode/weaponServices wrappers, null-hop guard (offline vm)", () => {
  // Full addon eval order: schema.generated.js + nav.generated.js + pawn.js.
  // EntityRef stub provides *Via + readFloatsChain methods (fieldOff-respecting).
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readInt32 = function () { return 100; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  EntityRef.prototype.readFloat32Via = function (path, off) { return off >= 0 ? 1.5 : null; };
  EntityRef.prototype.readBoolVia = function (path, off) { return off >= 0 ? false : null; };
  EntityRef.prototype.readInt8Via = function (path, off) { return off >= 0 ? 0 : null; };
  EntityRef.prototype.readInt16Via = function (path, off) { return off >= 0 ? 0 : null; };
  EntityRef.prototype.readInt32Via = function (path, off) { return off >= 0 ? 42 : null; };
  EntityRef.prototype.readUInt8Via = function (path, off) { return off >= 0 ? 0 : null; };
  EntityRef.prototype.readUInt16Via = function (path, off) { return off >= 0 ? 0 : null; };
  EntityRef.prototype.readUInt32Via = function (path, off) { return off >= 0 ? 42 : null; };
  EntityRef.prototype.readUInt64Via = function (path, off) { return off >= 0 ? BigInt(0) : null; };
  EntityRef.prototype.readInt64Via = function (path, off) { return off >= 0 ? BigInt(0) : null; };
  EntityRef.prototype.readHandleVia = function (path, off) { return off >= 0 ? new EntityRef(5, 3) : null; };
  EntityRef.prototype.readFloatsChain = function (path, fieldOff) { return fieldOff >= 0 ? [1, 2, 3] : null; };
  const math = {
    Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
    QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
  };
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef } : n === "@s2script/sdk/math" ? math : null),
    __s2_schema_offset: () => 8,      // all offsets valid; NAV paths resolve to [8, …]
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + navJs + "\n" + pawnJs, ctx);
  const { Pawn } = ctx.__s2pkg_cs2;
  const p = new Pawn(new EntityRef(5, 9));

  // sceneNode wrapper: absOrigin is a Vector, scale reads via readFloat32Via.
  const sn = p.sceneNode;
  assert.ok(sn !== null, "pawn.sceneNode is a SceneNode wrapper (all path offsets ≥ 0)");
  assert.ok(sn.absOrigin instanceof math.Vector, "sceneNode.absOrigin is a Vector");
  assert.equal(sn.scale, 1.5, "sceneNode.scale reads via readFloat32Via");

  // weaponServices wrapper: activeWeapon is an EntityRef (decoded from handle).
  const ws = p.weaponServices;
  assert.ok(ws !== null, "pawn.weaponServices is a WeaponServices wrapper");
  const aw = ws.activeWeapon;
  assert.ok(aw instanceof EntityRef, "weaponServices.activeWeapon is an EntityRef");

  // Compat aliases delegate to the generated SceneNode wrapper.
  assert.ok(p.origin instanceof math.Vector, "pawn.origin delegates to sceneNode.absOrigin");
  assert.ok(p.angles instanceof math.QAngle, "pawn.angles delegates to sceneNode.absRotation");

  // Null-hop guard: re-eval with __s2_schema_offset returning -1 → each nav getter's oN < 0 guard returns null
  // immediately (per-access resolution, boot-window-safe — no baked NAV table).
  const ctx2 = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef } : n === "@s2script/sdk/math" ? math : null),
    __s2_schema_offset: () => -1,     // per-access off() returns -1 → oN < 0 guard returns null
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx2.globalThis = ctx2;
  vm.createContext(ctx2);
  vm.runInContext(genJs + "\n" + navJs + "\n" + pawnJs, ctx2);
  const { Pawn: Pawn2 } = ctx2.__s2pkg_cs2;
  const p2 = new Pawn2(new EntityRef(5, 9));
  assert.equal(p2.sceneNode, null, "pawn.sceneNode is null when off() returns -1 (boot-window guard)");
  assert.equal(p2.origin, null, "pawn.origin is null when sceneNode is null");
  assert.equal(p2.weaponServices, null, "pawn.weaponServices is null when off() returns -1");
});

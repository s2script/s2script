import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const genJs = readFileSync(join(repo, "games/cs2/js/schema.generated.js"), "utf8");
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
    __s2require: (n) => (n === "@s2script/entity" ? stdEntity : n === "@s2script/math" ? math : null),
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
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef }
      : n === "@s2script/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
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
    __s2require: (name) => (name === "@s2script/entity" ? stdPkg
      : name === "@s2script/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
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
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef }
      : n === "@s2script/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
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
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  let chainRet = [64, 128, 256];
  EntityRef.prototype.readFloatsChain = function () { return chainRet; };   // toggled to null below
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  const math = { Vector, QAngle };
  let offRet = 8;                                        // schema-offset stub; toggled to -1 below
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : n === "@s2script/math" ? math : null),
    __s2_schema_offset: () => offRet,
    __s2_ent_current_serial: () => 7, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Pawn } = ctx.__s2pkg_cs2;
  const p = new Pawn(new EntityRef(5, 9));
  assert.ok(p.origin instanceof Vector, "origin is a Vector");
  assert.deepEqual([p.origin.x, p.origin.y, p.origin.z], [64, 128, 256]);
  assert.ok(p.angles instanceof QAngle, "angles is a QAngle");
  chainRet = null;                                      // stale/broken chain → readFloatsChain null
  assert.equal(p.origin, null, "a null readFloatsChain → the accessor returns null");
  chainRet = [1, 2, 3]; offRet = -1;                    // a schema-offset miss → null (before the chain read)
  assert.equal(p.origin, null, "a missing offset → the accessor returns null");
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
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : n === "@s2script/math" ? math : null),
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

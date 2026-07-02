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
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? stdEntity : null),
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
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : null),
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
    __s2require: (name) => (name === "@s2script/entity" ? stdPkg : null),
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
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : null),
    __s2_schema_offset: () => 8, __s2_ent_current_serial: () => 7, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Player } = ctx.__s2pkg_cs2;
  assert.equal(Player.fromSlot(0), null, "valid controller but no pawn → not occupied → null");
  assert.deepEqual(Player.all(), [], "Player.all() excludes controllers without a pawn");
});

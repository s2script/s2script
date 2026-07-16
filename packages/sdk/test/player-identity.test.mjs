import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const genJs = readFileSync(join(repo, "games/cs2/js/schema.generated.js"), "utf8");
const pawnJs = readFileSync(join(repo, "games/cs2/js/pawn.js"), "utf8");

function runWith(clientMock, names) {
  const nameMap = names || {};                                    // controller entity index -> playerName
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readInt32 = function () { return 2; };
  EntityRef.prototype.readUInt8 = function () { return 2; };
  EntityRef.prototype.readFloat32 = function () { return 0.25; };
  EntityRef.prototype.readBool = function () { return false; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  EntityRef.prototype.readString = function () { return nameMap[this.index] || this._name || ""; };
  EntityRef.prototype.writeFloat32 = function (off, v) { (this.writes = this.writes || []).push([off, v]); return true; };
  EntityRef.prototype.notifyStateChanged = function (off) { (this.notified = this.notified || []).push(off); };
  const math = { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                 QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } };
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef } : n === "@s2script/sdk/math" ? math
                       : n === "@s2script/sdk/events" ? {} : null),
    __s2_schema_offset: () => 8,
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
    ...clientMock,
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  return ctx.__s2pkg_cs2;
}

test("Player.allConnected + userId (offline vm): connected slots regardless of pawn", () => {
  const { Player } = runWith({
    __s2_client_valid: (slot) => slot < 2,                       // slots 0,1 connected
    __s2_client_userid: (slot) => (slot === 0 ? 5 : slot === 1 ? 6 : -1),
    __s2_client_find_by_userid: (id) => (id === 6 ? 1 : -1),
  });
  const conn = Player.allConnected();
  assert.equal(conn.length, 2, "two connected slots");
  assert.equal(conn[0].slot, 0);
  assert.equal(conn[0].userId, 5, "userId reads the engine native, not schema");
  assert.equal(conn[1].userId, 6);
});

test("Player.fromUserId (offline vm): round-trips to the right slot, null on miss", () => {
  const { Player } = runWith({
    __s2_client_valid: () => true,
    __s2_client_userid: () => 6,
    __s2_client_find_by_userid: (id) => (id === 6 ? 1 : -1),
  });
  const p = Player.fromUserId(6);
  assert.ok(p, "found");
  assert.equal(p.slot, 1);
  assert.equal(Player.fromUserId(999), null, "miss -> null");
});

test("Player.kick (offline vm): calls __s2_client_kick with slot + reason", () => {
  const calls = [];
  const { Player } = runWith({
    __s2_client_valid: () => true,
    __s2_client_userid: () => 6,
    __s2_client_find_by_userid: (id) => (id === 6 ? 1 : -1),
    __s2_client_kick: (slot, reason) => calls.push([slot, reason]),
  });
  const p = Player.fromUserId(6);
  p.kick("bye");
  assert.deepEqual(calls, [[1, "bye"]]);
  p.kick();                                       // default reason
  assert.equal(calls[1][1], "Kicked by admin");
});

test("Player.target (offline vm): #userid / @all / @me / name / no-match", () => {
  const { Player } = runWith({
    __s2_client_valid: (slot) => slot < 2,
    __s2_client_userid: (slot) => (slot === 0 ? 5 : slot === 1 ? 6 : -1),
    __s2_client_find_by_userid: (id) => (id === 5 ? 0 : id === 6 ? 1 : -1),
    __s2_client_kick: () => {},
  });
  assert.equal(Player.target("#6", -1).length, 1, "#userid hit");
  assert.equal(Player.target("#6", -1)[0].slot, 1);
  assert.equal(Player.target("#999", -1).length, 0, "#userid miss -> empty");
  assert.equal(Player.target("@all", -1).length, 2, "@all -> all connected");
  assert.equal(Player.target("@me", 0).length, 1, "@me -> caller");
  assert.equal(Player.target("@me", 0)[0].slot, 0);
  assert.equal(Player.target("@me", -1).length, 0, "@me from console -> empty");
  assert.equal(Player.target("", 0).length, 0, "empty pattern -> empty");
});

test("Player.target name match (offline vm): exact wins over partial, partial returns all, no-match empty", () => {
  const { Player } = runWith({
    __s2_client_valid: (slot) => slot < 3,                       // slots 0,1,2 connected
    __s2_client_userid: (slot) => slot,
    __s2_client_find_by_userid: () => -1,
    __s2_client_kick: () => {},
  }, { 1: "Specialist", 2: "Rex", 3: "Specialist_2" });          // controller idx = slot+1
  // exact (case-insensitive) match wins even though "specialist" is also a substring of "Specialist_2"
  const exact = Player.target("specialist", -1);
  assert.equal(exact.length, 1, "exact name -> just that one");
  assert.equal(exact[0].slot, 0, "the exact-match slot");
  // partial: "spec" matches "Specialist" and "Specialist_2" (no exact) -> both
  const partial = Player.target("spec", -1).map((p) => p.slot).sort();
  assert.deepEqual(partial, [0, 2], "partial -> all matches");
  // no name match -> empty
  assert.equal(Player.target("nobody", -1).length, 0, "no name match -> empty");
});

test("Pawn.setVelocity (offline vm): writes 3 floats + notifyStateChanged", () => {
  const { Pawn } = runWith({ __s2_client_valid: () => true, __s2_client_userid: () => 0,
    __s2_client_find_by_userid: () => -1, __s2_client_kick: () => {} });
  const pawn = Pawn.forSlot(0);
  assert.ok(pawn, "pawn resolved");
  const ok = pawn.setVelocity(1, 2, 3);
  assert.equal(ok, true);
  assert.equal(pawn.ref.writes.length, 3, "three writeFloat32");
  assert.deepEqual(pawn.ref.writes.map((w) => w[1]), [1, 2, 3]);
  assert.equal(pawn.ref.notified.length, 1, "one notifyStateChanged");
});

import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const genJs = readFileSync(join(repo, "games/cs2/js/schema.generated.js"), "utf8");
const pawnJs = readFileSync(join(repo, "games/cs2/js/pawn.js"), "utf8");

function runWith(clientMock) {
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readInt32 = function () { return 2; };
  EntityRef.prototype.readUInt8 = function () { return 2; };
  EntityRef.prototype.readFloat32 = function () { return 0.25; };
  EntityRef.prototype.readBool = function () { return false; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  const math = { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                 QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } };
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : n === "@s2script/math" ? math
                       : n === "@s2script/events" ? {} : null),
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

// SourceMod multi-target filters on Player.target (@alive/@dead/@bots/@humans/@!me/@aim/@ct/@t/@spec)
// and the group labels Activity.targetName reads from the game package.
import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { installClientHost } from "./client-host.mjs";
import { cs2AddonBundle } from "./cs2-addon.mjs";

// slot -> { team, alive, bot }. Controller entity index = slot + 1; its pawn is index 200 + controller.
const PLAYERS = {
  0: { team: 3, alive: true, bot: false },   // the caller: a live CT
  1: { team: 3, alive: false, bot: false },  // dead CT
  2: { team: 2, alive: true, bot: true },    // live T bot
  3: { team: 2, alive: false, bot: false },  // dead T
  4: { team: 1, alive: false, bot: false },  // spectator
};

function load({ canTarget } = {}) {
  const at = (index) => PLAYERS[index - 1];
  function EntityRef(index, id) { this.index = index; this.id = id; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readUInt8 = function () { return at(this.index)?.team ?? 0; };     // m_iTeamNum
  EntityRef.prototype.readBool = function () { return at(this.index)?.alive ?? false; }; // m_bPawnIsAlive
  EntityRef.prototype.readInt32 = function () { return 200 + this.index; };             // m_hPlayerPawn
  EntityRef.prototype.readHandle = function () { return null; };
  EntityRef.prototype.readString = function () { return ""; };
  const ctx = {
    __s2require: (n) => (n === "@s2script/sdk/entity" ? { EntityRef } : n === "@s2script/sdk/events" ? {} : null),
    __s2_schema_offset: () => 8,
    __s2_ent_id_for_index: () => 0,
    __s2_handle_adopt: (h) => [h & 0x7fff, 0],
    __s2_client_valid: (slot) => slot in PLAYERS,
    __s2_client_userid: (slot) => slot + 10,
    __s2_client_steamid: (slot) => (PLAYERS[slot]?.bot ? "0" : "7656119800000000" + slot),
    __s2_client_find_by_userid: (id) => (id - 10 in PLAYERS ? id - 10 : -1),
    __s2_client_isbot: (slot) => PLAYERS[slot]?.bot === true,
    __s2pkg_trace: {},
  };
  if (canTarget) ctx.__s2_admin_can_target = canTarget;
  ctx.globalThis = ctx;
  const host = installClientHost(ctx);
  for (const slot of Object.keys(PLAYERS)) host.connect(Number(slot));
  vm.createContext(ctx);
  vm.runInContext(cs2AddonBundle, ctx);
  return ctx;
}

const slots = (list) => [...list].map((p) => p.slot).sort((a, b) => a - b);

test("generic group filters select from connected players", () => {
  const { Player } = load().__s2pkg_cs2;
  assert.deepEqual(slots(Player.target("@all", -1)), [0, 1, 2, 3, 4]);
  assert.deepEqual(slots(Player.target("@alive", -1)), [0, 2]);
  assert.deepEqual(slots(Player.target("@dead", -1)), [1, 3, 4], "SM @dead includes spectators");
  assert.deepEqual(slots(Player.target("@bots", -1)), [2]);
  assert.deepEqual(slots(Player.target("@humans", -1)), [0, 1, 3, 4]);
});

test("team filters select by CS2 team number", () => {
  const { Player } = load().__s2pkg_cs2;
  assert.deepEqual(slots(Player.target("@ct", -1)), [0, 1]);
  assert.deepEqual(slots(Player.target("@t", -1)), [2, 3]);
  assert.deepEqual(slots(Player.target("@spec", -1)), [4]);
});

test("caller-relative filters need a player caller", () => {
  const { Player } = load().__s2pkg_cs2;
  assert.deepEqual(slots(Player.target("@!me", 0)), [1, 2, 3, 4]);
  assert.deepEqual(slots(Player.target("@!me", -1)), [], "console has no 'me'");
  assert.deepEqual(slots(Player.target("@aim", -1)), [], "console has no crosshair");
  assert.deepEqual(slots(Player.target("@CT", -1)), [], "filters are case-sensitive, as in SourceMod");
});

test("@aim resolves the hit entity against each player's own pawn", () => {
  const { Player, Pawn } = load().__s2pkg_cs2;
  let hit = null;
  Pawn.prototype.aimTrace = function () { return hit; };
  hit = { didHit: true, entity: { index: 200 + 3 + 1, id: 0 } };          // slot 3's pawn
  assert.deepEqual(slots(Player.target("@aim", 0)), [3]);
  hit = { didHit: true, entity: { index: 200 + 3 + 1, id: 9 } };          // same index, other entity
  assert.deepEqual(slots(Player.target("@aim", 0)), [], "a reused index with another id is not that player");
  hit = { didHit: true, entity: { index: 50, id: 0 } };                   // world prop
  assert.deepEqual(slots(Player.target("@aim", 0)), []);
  hit = { didHit: false, entity: null };
  assert.deepEqual(slots(Player.target("@aim", 0)), []);
  hit = { didHit: true, entity: { index: 201, id: 0 } };                  // the caller's own pawn
  assert.deepEqual(slots(Player.target("@aim", 0)), [], "@aim never targets the caller");
});

test("filterImmunity applies to group filters", () => {
  const { Player } = load({ canTarget: (caller, target) => target !== 1 }).__s2pkg_cs2;
  assert.deepEqual(slots(Player.target("@ct", 0, true)), [0], "immune slot 1 dropped");
  assert.deepEqual(slots(Player.target("@ct", 0)), [0, 1], "no filter unless asked");
});

test("every filter publishes a group label for Activity.targetName", () => {
  const ctx = load();
  const labels = ctx.__s2_target_labels;
  for (const f of ["@all", "@alive", "@dead", "@bots", "@humans", "@!me", "@ct", "@t", "@spec"]) {
    assert.equal(typeof labels[f].key, "string", f);
    assert.equal(typeof labels[f].fallback, "string", f);
  }
  assert.equal(labels["@ct"].fallback, "all Counter-Terrorists");
  assert.ok(Object.isFrozen(labels));
});

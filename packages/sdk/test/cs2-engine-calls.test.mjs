/**
 * A5b — the behaviour that moved OUT of core/shim and INTO games/cs2/js when the eight CS2 engine
 * calls became `calls` descriptors in gamedata/cs2/game.cs2.jsonc.
 *
 * WHY THIS FILE EXISTS. A descriptor expresses LAYOUT: receiver, arg kinds, return kind. It cannot
 * express a precondition, a default, a bound, a fallback to a different descriptor, or a deferral —
 * and every one of the eight had at least one, each of them a live-gate finding paid for once. Those
 * rules used to live in C++/Rust and had core-side tests; the ops are gone and so are those tests.
 * This is where the same rules are pinned now.
 *
 * These are NOT tests of the marshalling path (core/src/gamedata_calls.rs and
 * shim/tests/call_validate_test.cpp own that) and NOT tests of whether a signature resolves (that
 * needs the binary, and is the live gate's job). They test the DECISIONS games/cs2/js makes before
 * and after it invokes: what is called, with which arguments, in which order, on which frame, and
 * — most often the interesting one — when nothing is called at all.
 *
 * The subject is the REAL shipped bundle (packages/sdk/test/cs2-addon.mjs derives the file list from
 * scripts/package-addon.sh), run in a vm context whose natives are fakes that record. Nothing here
 * is a re-implementation of pawn.js, so nothing here can drift from it without going red.
 */
import { test } from "node:test";
import assert from "node:assert";
import vm from "node:vm";
import { cs2AddonBundle } from "./cs2-addon.mjs";

/** Every descriptor gamedata/cs2/game.cs2.jsonc declares — the default "all eight resolved" host. */
const ALL_CALLS = ["commitSuicide", "changeTeam", "switchTeam", "terminateRound",
                   "respawn", "setPawn", "giveNamedItem", "removePlayerItem"];

/**
 * Build a CS2 game-package context.
 *
 * `ready` is the set of descriptors that passed their load-time gates; anything omitted models a
 * DEGRADED descriptor (bad signature / failed validator / older host), which is the state every
 * wrapper must survive without throwing.
 *
 * `onInvoke(name, args, recv)` may return the invoke's value (an entity handle for giveNamedItem) and
 * may itself call back into the package — which is how the re-entrancy tests are written, because
 * re-entrancy is exactly what these calls do on a real server.
 */
function makeHost({ ready = ALL_CALLS, onInvoke } = {}) {
  const readySet = new Set(ready);
  const invokes = [];            // { name, index, id, args }
  const frames = [];             // pending __s2_next_frame resolvers
  const logs = [];

  // Distinct, stable offsets per (class, field) so a read can be attributed to the field it came
  // from rather than to "whatever the mock returns".
  const offsets = new Map();
  const offsetOf = (cls, field) => {
    const k = `${cls}.${field}`;
    if (!offsets.has(k)) offsets.set(k, 8 * (offsets.size + 1));
    return offsets.get(k);
  };
  const ALIVE = offsetOf("CCSPlayerController", "m_bPawnIsAlive");
  const PAWN_HANDLE = offsetOf("CCSPlayerController", "m_hPlayerPawn");

  function EntityRef(index, id) { this.index = index; this.id = id; }
  EntityRef.prototype.isValid = function () { return this.live !== false; };
  // `alive` and `pawnHandle` are set on the ref the test holds; the accessors read them through the
  // same schema path pawn.js uses (offset -> typed read), never by a special case in pawn.js.
  EntityRef.prototype.readBool = function (o) { return o === ALIVE ? this.alive === true : false; };
  EntityRef.prototype.readHandle = function (o) {
    if (o !== PAWN_HANDLE) return null;
    return this.pawnHandle === undefined ? new EntityRef(this.index + 64, 7) : this.pawnHandle;
  };
  EntityRef.prototype.readInt32 = function () { return 1; };
  EntityRef.prototype.readUInt8 = function () { return 2; };
  EntityRef.prototype.readUInt32 = function () { return 1; };
  EntityRef.prototype.readFloat32 = function () { return 0; };
  EntityRef.prototype.readString = function () { return ""; };
  EntityRef.prototype.readBoolVia = function () { return false; };
  EntityRef.prototype.readInt32Via = function () { return 0; };
  EntityRef.prototype.readFloat32Via = function () { return 0; };
  EntityRef.prototype.writeFloat32 = function () { return true; };
  EntityRef.prototype.notifyStateChanged = function () {};
  EntityRef.prototype.remove = function () { this.removed = true; return true; };

  const gamerules = new EntityRef(99, 3);

  const ctx = {
    __s2require: (n) => n === "@s2script/sdk/entity" ? { EntityRef }
      : n === "@s2script/sdk/math" ? {
          Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
          QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
        }
      : n === "@s2script/sdk/events" ? {} : null,
    __s2_schema_offset: offsetOf,
    __s2_ent_id_for_index: (i) => i,
    __s2_handle_adopt: (h) => [h & 0x7fff, 0],
    __s2_client_valid: () => true,
    __s2_client_userid: (s) => s,
    __s2_client_find_by_userid: () => -1,

    // The four game-package call natives (core/src/v8host.rs). Absent natives are a DIFFERENT case
    // — an older host — and are covered by the "no natives at all" test below.
    __s2_game_call_ready: (name) => readySet.has(name),
    __s2_game_call_receiverless: () => false,
    __s2_game_call_status: (name) => readySet.has(name) ? "available" : "degraded in this test",
    __s2_game_call_invoke: (name, index, id, args) => {
      invokes.push({ name, index, id, args });
      return onInvoke ? onInvoke(name, args, { index, id }) : undefined;
    },

    // A stand-in for core's tick-integrated nextFrame: the promise resolves only when the test says
    // a frame passed, so "which frame did this land on" is an assertion rather than a race.
    __s2_next_frame: () => new Promise((res) => frames.push(res)),

    __s2pkg_entity: { Entity: { findByClass: (c) => (c === "cs_gamerules" ? [gamerules] : []) } },
    console: { log: (m) => logs.push(String(m)) },
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(cs2AddonBundle, ctx);

  /** Advance one engine frame: resolve every armed nextFrame, then let the microtasks run. */
  async function frame() {
    const pending = frames.splice(0);
    pending.forEach((r) => r());
    await new Promise((r) => setImmediate(r));
  }

  const pkg = ctx.__s2pkg_cs2;
  return {
    ctx, pkg, invokes, logs, frame, EntityRef, gamerules, offsetOf,
    names: () => invokes.map((i) => i.name),
    /** A dead player on `slot`, with a live controller and a live m_hPlayerPawn. */
    player(slot) {
      const p = pkg.Player._fromSlotUnchecked(slot);
      p.ref.alive = false;
      return p;
    },
  };
}

// -------------------------------------------------------------------------------------------
// commitSuicide — pawn.slay()
// -------------------------------------------------------------------------------------------

test("slay: bForce is TRUE and bExplode false — the rate limiter is bypassed", () => {
  const h = makeHost();
  h.pkg.Pawn.forSlot(0).slay();
  assert.deepEqual(h.invokes.length, 1);
  assert.equal(h.invokes[0].name, "commitSuicide");
  // The body is an m_fNextSuicideTime rate limiter with `test bForce; je ret`. bForce=false makes
  // slay() a SILENT no-op for anyone who died recently — the reason this pair is hardcoded.
  assert.deepEqual(h.invokes[0].args, [false, true]);
});

test("slay: a degraded descriptor is a silent no-op, not a throw", () => {
  const h = makeHost({ ready: ALL_CALLS.filter((n) => n !== "commitSuicide") });
  assert.doesNotThrow(() => h.pkg.Pawn.forSlot(0).slay());
  assert.equal(h.invokes.length, 0);
});

test("the receiver crosses as its EntityRef (index, id) pair, never the wrapper", () => {
  const h = makeHost();
  const pawn = h.pkg.Pawn.forSlot(3);
  pawn.slay();
  assert.equal(h.invokes[0].index, pawn.ref.index);
  assert.equal(h.invokes[0].id, pawn.ref.id);
});

// -------------------------------------------------------------------------------------------
// changeTeam / switchTeam — the bound, and the dispatch BETWEEN two descriptors
// -------------------------------------------------------------------------------------------

test("changeTeam: 0..3 only (Unassigned/Spectator/T/CT); out of range calls nothing", () => {
  const h = makeHost();
  for (const t of [0, 1, 2, 3]) h.player(0).changeTeam(t);
  assert.deepEqual(h.invokes.map((i) => i.args[0]), [0, 1, 2, 3]);
  const before = h.invokes.length;
  for (const t of [-1, 4, 99]) h.player(0).changeTeam(t);
  assert.equal(h.invokes.length, before, "an out-of-range team must not reach the engine");
});

test("spectate() is changeTeam(1)", () => {
  const h = makeHost();
  h.player(0).spectate();
  assert.deepEqual(h.names(), ["changeTeam"]);
  assert.deepEqual(h.invokes[0].args, [1]);
});

test("switchTeam: team <= 1 ROUTES to changeTeam (the engine SwitchTeam is T/CT-only)", () => {
  const h = makeHost();
  h.player(0).switchTeam(0);
  h.player(0).switchTeam(1);
  assert.deepEqual(h.names(), ["changeTeam", "changeTeam"]);
  h.player(0).switchTeam(2);
  h.player(0).switchTeam(3);
  assert.deepEqual(h.names().slice(2), ["switchTeam", "switchTeam"]);
});

test("switchTeam(1) still works when SwitchTeam is degraded but ChangeTeam is not", () => {
  // The ORDER of the guards is the whole point: bound, then route, then THIS descriptor's
  // readiness. Testing readiness first would make a perfectly serviceable switchTeam(1) fail
  // because an unrelated signature went stale.
  const h = makeHost({ ready: ALL_CALLS.filter((n) => n !== "switchTeam") });
  h.player(0).switchTeam(1);
  assert.deepEqual(h.names(), ["changeTeam"]);
  h.player(0).switchTeam(2);
  assert.deepEqual(h.names(), ["changeTeam"], "T/CT has nowhere to route — no call");
});

test("switchTeam: the 0..3 bound is checked BEFORE the route (a bad team never reaches either)", () => {
  const h = makeHost();
  for (const t of [-1, 4]) h.player(0).switchTeam(t);
  assert.equal(h.invokes.length, 0);
});

// -------------------------------------------------------------------------------------------
// respawn + setPawn — the richest precondition set, and the drain that used to be C++
// -------------------------------------------------------------------------------------------

test("respawn: a LIVE player is refused and nothing is queued", async () => {
  const h = makeHost();
  const p = h.player(0);
  p.ref.alive = true;
  assert.equal(p.respawn(), false);
  await h.frame();
  assert.equal(h.invokes.length, 0);
});

test("respawn: BOTH Respawn and SetPawn or NEITHER — a half-ready pair never half-runs", async () => {
  // Live-gate finding on 2000875: Respawn without SetPawn clears the death screen and never spawns,
  // which reads to a player as a broken server rather than a disabled feature.
  for (const missing of ["respawn", "setPawn"]) {
    const h = makeHost({ ready: ALL_CALLS.filter((n) => n !== missing) });
    assert.equal(h.player(0).respawn(), false, `${missing} degraded -> refused`);
    await h.frame();
    assert.equal(h.invokes.length, 0, `${missing} degraded -> no engine call at all`);
  }
});

test("respawn: nothing runs synchronously; SetPawn then Respawn on the NEXT frame", async () => {
  const h = makeHost();
  const p = h.player(0);
  assert.equal(p.respawn(), true, "queued");
  assert.equal(h.invokes.length, 0, "the engine call must not run inside the caller's dispatch");
  await h.frame();
  // SetPawn BEFORE Respawn, same frame, in that order (SwiftlyS2/CSSharp's exact sequence).
  assert.deepEqual(h.names(), ["setPawn", "respawn"]);
  // EXACTLY three args after `this`: (playerPawn, true, false). A fourth feeds the function a
  // different reset flag — the arity is a correctness fact, not a convenience.
  assert.equal(h.invokes[0].args.length, 3);
  assert.equal(h.invokes[0].args[0].index, p.ref.index + 64, "arg 0 is the pawn's EntityRef");
  assert.deepEqual(h.invokes[0].args.slice(1), [true, false]);
  assert.deepEqual(h.invokes[1].args, [], "Respawn is nullary");
});

test("respawn: dedupe within a frame — the second call returns TRUE and queues nothing", async () => {
  const h = makeHost();
  const p = h.player(0);
  assert.equal(p.respawn(), true);
  // The idempotent second call reports SUCCESS, not failure: TTT's round-start loop legitimately
  // respawns the same player twice and must not read that as a rejection.
  assert.equal(p.respawn(), true, "idempotent second call is a success");
  // A DIFFERENT Player object for the same slot dedupes too — the key is the (index, id) pair.
  assert.equal(h.player(0).respawn(), true);
  await h.frame();
  assert.deepEqual(h.names(), ["setPawn", "respawn"], "one pair, not three");
});

test("respawn: the pending set is capped, and the cap rejects rather than grows", async () => {
  const h = makeHost();
  const queued = [];
  for (let slot = 0; slot < 140; slot++) queued.push(h.player(slot).respawn());
  const accepted = queued.filter(Boolean).length;
  assert.equal(accepted, 130, "the shim's kRespawnPendingMax, verbatim");
  assert.equal(queued[130], false, "over the cap -> rejected, not silently dropped");
  assert.ok(h.logs.some((l) => /pending set full/.test(l)), "and it says so");
  await h.frame();
  assert.equal(h.invokes.length, 260, "130 pairs");
});

test("respawn: CONSUME BEFORE CALL — a respawn issued from the drain lands on the NEXT frame", async () => {
  // The engine's Respawn fires player_spawn synchronously, so a player_spawn handler calling
  // respawn() re-enters this queue mid-drain. Draining a list you are still appending to is an
  // unbounded loop inside one frame; emptying it first turns that into one pair per frame.
  let reentered = false;
  const h = makeHost({
    onInvoke: (name) => {
      if (name === "respawn" && !reentered) { reentered = true; h.player(1).respawn(); }
    },
  });
  h.player(0).respawn();
  await h.frame();
  assert.deepEqual(h.names(), ["setPawn", "respawn"], "the re-entrant request is NOT in this batch");
  await h.frame();
  assert.deepEqual(h.names().slice(2), ["setPawn", "respawn"], "it lands on the following frame");
});

test("respawn: a controller that goes stale between enqueue and drain is skipped", async () => {
  const h = makeHost();
  const p = h.player(0);
  assert.equal(p.respawn(), true);
  p.ref.live = false;                       // died in the intervening frame
  await h.frame();
  assert.equal(h.invokes.length, 0);
  assert.ok(h.logs.some((l) => /stale controller at drain/.test(l)));
});

test("respawn: a player who came ALIVE between enqueue and drain is skipped (the TOCTOU)", async () => {
  const h = makeHost();
  const p = h.player(0);
  assert.equal(p.respawn(), true);
  p.ref.alive = true;                       // another plugin respawned them first
  await h.frame();
  assert.equal(h.invokes.length, 0, "the drain re-check, not just the enqueue check");
});

test("respawn: a stale m_hPlayerPawn SKIPS SetPawn but still runs Respawn", async () => {
  // The guard a descriptor INVERTS: an `entity` argument that fails to resolve does not abort the
  // call, it marshals to nullptr and the engine still runs — so without an explicit null test the
  // engine would get SetPawn(controller, nullptr).
  const h = makeHost();
  const p = h.player(0);
  assert.equal(p.respawn(), true);
  p.ref.pawnHandle = null;                  // handle went stale/absent between enqueue and drain
  await h.frame();
  assert.deepEqual(h.names(), ["respawn"]);
});

test("respawn: refused when the host has no nextFrame (nothing would drain the queue)", () => {
  const h = makeHost();
  h.ctx.__s2_next_frame = undefined;
  assert.equal(h.player(0).respawn(), false);
  assert.equal(h.invokes.length, 0);
});

test("respawn: a stale controller is refused at enqueue too", () => {
  const h = makeHost();
  const p = h.player(0);
  p.ref.live = false;
  assert.equal(p.respawn(), false);
});

// -------------------------------------------------------------------------------------------
// terminateRound — the bound, the default, DELAY-FIRST, and the single-slot drain
// -------------------------------------------------------------------------------------------

test("terminateRound: DELAY FIRST, and the default delay is 5s", async () => {
  const h = makeHost();
  assert.equal(h.pkg.GameRules.terminateRound(8), true);
  await h.frame();
  assert.equal(h.invokes.length, 1);
  assert.equal(h.invokes[0].name, "terminateRound");
  // (float delay /*xmm0*/, uint32 reason /*esi*/, void* unk3, uint32 unk4). Reason-first is a
  // managed-marshaller artifact of the other frameworks; swapping them is SILENT — the reason
  // would land in xmm0 and the delay in the reason register.
  assert.deepEqual(h.invokes[0].args, [5.0, 8, 0, 0]);
});

test("terminateRound: an explicit delay is passed through, still first", async () => {
  const h = makeHost();
  h.pkg.GameRules.terminateRound(8, 1.5);
  await h.frame();
  assert.deepEqual(h.invokes[0].args, [1.5, 8, 0, 0]);
});

test("terminateRound: the reason bound is the engine's own 0..22", async () => {
  const h = makeHost();
  for (const r of [-1, 23, 99]) {
    assert.equal(h.pkg.GameRules.terminateRound(r), false, `reason ${r} rejected`);
  }
  await h.frame();
  assert.equal(h.invokes.length, 0);
  assert.ok(h.logs.some((l) => /out of range 0\.\.22/.test(l)));
  // The in-range legacy holes pass through deliberately — the engine's own switch handles them.
  for (const r of [0, 2, 3, 15, 22]) assert.equal(h.pkg.GameRules.terminateRound(r), true, `reason ${r}`);
});

// Scoped deliberately: `makeHost()` is ONE plugin context, and the pending slot lives in the
// prelude closure, which is evaluated once per context. This pins latest-wins WITHIN a plugin.
// It is NOT "a round ends once" — the shim's s_pendingTerminate was a host-global static and this
// is not, so two plugins in one frame drain two requests (spec §9.2a). Nothing here can assert
// that, and no test should be written as if it did.
test("terminateRound: single slot, LATEST WINS within one plugin context", async () => {
  const h = makeHost();
  h.pkg.GameRules.terminateRound(8);
  h.pkg.GameRules.terminateRound(9);
  assert.ok(h.logs.some((l) => /overwriting a pending request/.test(l)));
  await h.frame();
  assert.equal(h.invokes.length, 1, "one round end from this context, not two");
  assert.equal(h.invokes[0].args[1], 9, "the latest reason");
});

test("terminateRound: CONSUME BEFORE CALL — a request from the drain arms the NEXT frame", async () => {
  let reentered = false;
  const h = makeHost({
    onInvoke: (name) => {
      if (name === "terminateRound" && !reentered) { reentered = true; h.pkg.GameRules.terminateRound(10); }
    },
  });
  h.pkg.GameRules.terminateRound(8);
  await h.frame();
  assert.equal(h.invokes.length, 1, "the re-entrant request did not run inside this drain");
  await h.frame();
  assert.equal(h.invokes.length, 2);
  assert.equal(h.invokes[1].args[1], 10);
});

test("terminateRound: a proxy that dies before the drain drops the request", async () => {
  const h = makeHost();
  h.pkg.GameRules.terminateRound(8);
  h.gamerules.live = false;                 // map change between enqueue and drain
  await h.frame();
  assert.equal(h.invokes.length, 0);
  assert.ok(h.logs.some((l) => /stale gamerules proxy at drain/.test(l)));
});

test("terminateRound: a degraded descriptor answers false immediately", async () => {
  const h = makeHost({ ready: ALL_CALLS.filter((n) => n !== "terminateRound") });
  assert.equal(h.pkg.GameRules.terminateRound(8), false);
  await h.frame();
  assert.equal(h.invokes.length, 0);
});

// -------------------------------------------------------------------------------------------
// giveNamedItem / removePlayerItem
// -------------------------------------------------------------------------------------------

test("giveNamedItem: the four trailing args are hardcoded 0 and the name is stringified", () => {
  const h = makeHost({ onInvoke: (n) => (n === "giveNamedItem" ? new h.EntityRef(50, 4) : undefined) });
  const w = h.pkg.Pawn.forSlot(0).giveNamedItem("weapon_ak47");
  assert.deepEqual(h.invokes[0].args, ["weapon_ak47", 0, 0, 0, 0]);
  assert.ok(w, "a returned handle becomes a Weapon");
  assert.equal(w.ref.index, 50);
});

test("giveNamedItem: a null name calls nothing; a failed call yields null", () => {
  const h = makeHost({ onInvoke: () => null });
  assert.equal(h.pkg.Pawn.forSlot(0).giveNamedItem(null), null);
  assert.equal(h.invokes.length, 0, "null name -> no engine call");
  assert.equal(h.pkg.Pawn.forSlot(0).giveNamedItem("weapon_ak47"), null, "no handle -> null");
});

test("weapon.remove: the unequip carries (owner receiver, weapon EntityRef arg)", () => {
  const h = makeHost();
  const wref = new h.EntityRef(50, 4);
  wref.pawnOwner = true;
  const w = new h.pkg.Weapon(wref);
  const owner = new h.EntityRef(7, 2);
  Object.defineProperty(w, "owner", { value: new h.pkg.Pawn(owner) });
  assert.equal(w.remove(), true);
  assert.deepEqual(h.names(), ["removePlayerItem"]);
  assert.equal(h.invokes[0].index, 7, "receiver = the owning pawn");
  assert.equal(h.invokes[0].args[0], wref, "the `entity` ARG is the EntityRef itself, not the wrapper");
  assert.equal(wref.removed, true, "and the entity is still destroyed");
});

test("weapon.remove: BOTH refs must resolve or there is NO unequip call at all", () => {
  // A stale `entity` argument marshals to nullptr and the call still runs, which would be
  // RemovePlayerItem(pawn, nullptr) — so the guard has to be here, not in the descriptor.
  const h = makeHost();
  const wref = new h.EntityRef(50, 4);
  const w = new h.pkg.Weapon(wref);
  const dead = new h.EntityRef(7, 2);
  dead.live = false;
  Object.defineProperty(w, "owner", { value: new h.pkg.Pawn(dead) });
  assert.equal(w.remove(), true, "the destroy still happens");
  assert.equal(h.invokes.length, 0, "the unequip does not");
});

test("weapon.remove: an unowned weapon is destroyed with no unequip", () => {
  const h = makeHost();
  const wref = new h.EntityRef(50, 4);
  const w = new h.pkg.Weapon(wref);
  Object.defineProperty(w, "owner", { value: null });
  assert.equal(w.remove(), true);
  assert.equal(h.invokes.length, 0);
});

test("weapon.remove: a degraded removePlayerItem still destroys the entity", () => {
  const h = makeHost({ ready: ALL_CALLS.filter((n) => n !== "removePlayerItem") });
  const wref = new h.EntityRef(50, 4);
  const w = new h.pkg.Weapon(wref);
  Object.defineProperty(w, "owner", { value: new h.pkg.Pawn(new h.EntityRef(7, 2)) });
  assert.equal(w.remove(), true);
  assert.equal(h.invokes.length, 0);
  assert.equal(wref.removed, true);
});

// -------------------------------------------------------------------------------------------
// The host contract itself
// -------------------------------------------------------------------------------------------

test("an older host with no game-call natives: every wrapper degrades, nothing throws", async () => {
  const h = makeHost({ ready: [] });
  h.ctx.__s2_game_call_ready = undefined;
  h.ctx.__s2_game_call_status = undefined;
  // The bundle already captured its callables, so re-evaluate it against the stripped host — which
  // is exactly what an older core looks like at plugin-context creation.
  vm.runInContext(cs2AddonBundle, h.ctx);
  const pkg = h.ctx.__s2pkg_cs2;
  assert.doesNotThrow(() => pkg.Pawn.forSlot(0).slay());
  assert.doesNotThrow(() => pkg.Player._fromSlotUnchecked(0).changeTeam(2));
  assert.equal(pkg.Player._fromSlotUnchecked(0).respawn(), false);
  assert.equal(pkg.GameRules.terminateRound(8), false);
  assert.equal(pkg.Pawn.forSlot(0).giveNamedItem("weapon_ak47"), null);
  await h.frame();
  assert.equal(h.invokes.length, 0);
  assert.match(h.ctx.__s2pkg_cs2_calls.status("respawn"), /too old/);
});

test("__s2pkg_cs2_calls.status names WHY a descriptor is unavailable", () => {
  const h = makeHost({ ready: ALL_CALLS.filter((n) => n !== "respawn") });
  assert.equal(h.ctx.__s2pkg_cs2_calls.status("changeTeam"), "available");
  assert.equal(h.ctx.__s2pkg_cs2_calls.status("respawn"), "degraded in this test");
});

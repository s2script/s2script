import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { buildSync } from "esbuild";

// Exercise the real registrations and effect manager, replacing only host-injected operations.
const source = buildSync({ entryPoints: [new URL("./plugin.ts", import.meta.url).pathname], bundle: true,
  platform: "node", format: "cjs", external: ["@s2script/*"], write: false }).outputFiles[0].text;

function fixture() {
  const commands = new Map(), players = new Map(), timers = new Set(), replies = [], targetsCalled = [];
  const fades = [], teleports = [], beams = [], events = new Map(), moves = [];
  let targets = [], now = 0, identity = 0, failBeamAt = -1, failFade = false, onFade, onBeam, failTimer = false;
  const timer = (ms, fn, repeat) => {
    if (failTimer) throw Error("AsyncQueueFull");
    const t = { alive: true, due: now + ms, ms, fn, repeat, kill() { const was = this.alive; this.alive = false; timers.delete(this); return was; } };
    timers.add(t); return t;
  };
  const sdk = {
    command: { admin: (name, flags, handler) => commands.set(name, { flags, handler }) },
    translations: { load() {} }, ADMFLAG: { SLAY: 32, CHEATS: 16384 }, HookResult: { Handled: 2 },
    Clients: { fromSlot: slot => players.get(slot)?.client ?? null },
    every: (ms, fn) => timer(ms, fn, true), after: (ms, fn) => timer(ms, fn, false),
    delay: ms => new Promise(resolve => timer(ms, resolve, false)),
    Vector: class { constructor(x, y, z) { Object.assign(this, { x, y, z }); } },
  };
  const cs2 = {
    Player: { fromSlot: slot => players.get(slot)?.pawn ? players.get(slot) : null,
      fromUserId: uid => [...players.values()].find(p => p.userId === uid) ?? null,
      target(pattern, caller, immunity) { targetsCalled.push([pattern, caller, immunity]); return targets; } },
    Pawn: { forSlot: slot => players.get(slot)?.pawn ?? null },
    Fade: { to(slot, opts) { fades.push([slot, structuredClone(opts)]); onFade?.(); return !failFade; }, blind: () => true },
    Beam: { draw(start, end, opts) {
      if (beams.length === failBeamAt) return null;
      const b = { start: structuredClone(start), end: structuredClone(end), opts, removed: false,
        updates: 0, update(a, z) { this.start = structuredClone(a); this.end = structuredClone(z); this.updates++; },
        remove() { const was = !this.removed; this.removed = true; return was; } };
      beams.push(b); onBeam?.(); return b;
    } },
    Events: { on: (name, handler) => events.set(name, handler) },
    movement: { onProcessMove: fn => moves.push(fn) },
  };
  const context = vm.createContext({ module: { exports: {} }, console, require(name) {
    if (name === "@s2script/sdk") return sdk;
    if (name === "@s2script/cs2") return cs2;
    throw Error(name);
  } });
  vm.runInContext(source, context);
  const api = context.module.exports; api.OnPluginStart();
  const addPawn = (p, props = {}) => {
    const pawn = { ref: { index: 100 + p.slot, id: ++identity, teleport(origin, angles, velocity) {
      teleports.push({ pawn, origin: structuredClone(origin), angles: structuredClone(angles), velocity: structuredClone(velocity) });
      if (!pawn.failTeleport && angles) pawn.eyeAngles = { x: angles[0], y: angles[1], z: angles[2] };
      return !pawn.failTeleport;
    } }, isValid: true, lifeState: 0, health: 100, takesDamage: true, velocityModifier: 1, moveType: 2,
    origin: { x: 10, y: 20, z: 30 }, eyeAngles: { x: 11, y: 22, z: 7 }, movementServices: { maxspeed: 250 }, maxSpeed: 250,
    scales: [], setModelScale(scale) { if (this.failScale) return false; this.scales.push(scale); return true; }, ...props };
    p.pawn = pawn; return pawn;
  };
  const addPlayer = (slot = 1, props = {}) => {
    const p = { slot, userId: ++identity, ref: { index: slot + 1, id: ++identity }, steamId: "0", ...props };
    p.client = { slot, userId: p.userId, isValid: () => players.get(slot) === p };
    players.set(slot, p); addPawn(p); return p;
  };
  return { api, players, commands, timers, replies, targetsCalled, fades, teleports, beams, addPlayer, addPawn,
    setTargets: list => targets = list,
    failBeams: index => failBeamAt = index, failFades: () => failFade = true, failTimers: () => failTimer = true,
    onFade: fn => onFade = fn, onBeam: fn => onBeam = fn,
    call(name, args = [], callerSlot = -1) {
      assert.ok(commands.has(name), `${name} must be registered`);
      return commands.get(name).handler({ callerSlot, args, arg: i => args[i] ?? "", argFloat: (i, fallback) => parseFloat(args[i] ?? fallback), replyT: (...a) => replies.push(a) });
    },
    move(p) { for (const fn of moves) fn({ player: p }); },
    event(name, slot) { events.get(name)?.({ getPlayerSlot: () => slot }); },
    tick(ms) {
      const end = now + ms;
      while (true) {
        const t = [...timers].filter(t => t.alive && t.due <= end).sort((a, b) => a.due - b.due)[0];
        if (!t) break;
        now = t.due;
        if (!t.repeat) t.kill();
        t.fn();
        if (t.alive) t.due = now + t.ms;
      }
      now = end;
    },
  };
}

test("god toggles and obeys explicit enable/disable through takesDamage", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
  for (const [token, takesDamage] of [[undefined, false], ["0", true], ["1", false], ["1", false], ["-1", true]]) {
    assert.equal(f.call("sm_god", token === undefined ? ["@me"] : ["@me", token]), 2);
    assert.equal(p.pawn.takesDamage, takesDamage);
    assert.equal(f.replies.at(-1)[1], 1);
  }
  assert.equal(f.commands.get("sm_god").flags, 16384);
});

for (const [command, bad] of [["sm_god", ["", " ", "1x", "2", ".5", "NaN", "Infinity"]],
  ["sm_speed", [undefined, "", "1x", ".009", "10.01", "NaN", "Infinity", "0x2", " 1", "1e999"]],
  ["sm_size", ["", " ", ".19", "4.01", "1x", "NaN", "Infinity"]], ["sm_drug", ["", " ", "-1", "1x", "NaN", "Infinity", "1e999"]]]) {
  test(`${command} rejects malformed or out-of-range tokens before mutation`, () => {
    const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
    for (const token of bad) {
      f.call(command, token === undefined ? ["@all"] : ["@all", token]);
      assert.match(f.replies.at(-1)[0], /Invalid|Usage/);
    }
    assert.equal(p.pawn.takesDamage, true); assert.equal(p.pawn.velocityModifier, 1);
    assert.equal(p.pawn.scales.length, 0); assert.equal(f.fades.length, 0); assert.equal(f.timers.size, 0);
  });
}

test("new commands require targets, filter immunity, and use standard permissions", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
  for (const name of ["sm_god", "sm_speed", "sm_size", "sm_scale", "sm_unblind", "sm_drug", "sm_undrug", "sm_beacon"]) {
    assert.equal(f.call(name, [], 1), 2); assert.match(f.replies.at(-1)[0], /Usage/);
    f.call(name, ["@all", "1"], 1);
    assert.deepEqual(f.targetsCalled.at(-1), ["@all", 1, true]);
    assert.equal(f.commands.get(name).flags, name === "sm_god" ? 16384 : 32);
  }
});

test("size aliases use default 1 and report only successful valid pawn operations", () => {
  const f = fixture(), good = f.addPlayer(), bad = f.addPlayer(2), absent = f.addPlayer(3);
  bad.pawn.failScale = true; absent.pawn = null; f.setTargets([good, bad, absent]);
  f.call("sm_size", ["@all", "4"]); assert.deepEqual(good.pawn.scales, [4]); assert.equal(f.replies.at(-1)[1], 1);
  f.call("sm_scale", ["@all"]); assert.deepEqual(good.pawn.scales, [4, 1]);
});

test("missing schema fields and failed fades are not reported as successes", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); p.pawn.takesDamage = null; p.pawn.velocityModifier = null;
  f.call("sm_god", ["@all"]); assert.equal(f.replies.at(-1)[1], 0);
  f.call("sm_speed", ["@all", "2"]); assert.equal(f.replies.at(-1)[1], 0);
  f.failFades(); f.call("sm_unblind", ["@all"]); assert.equal(f.replies.at(-1)[1], 0);
});

test("failed schema writes do not count targets or retain effect timers", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
  Object.defineProperty(p.pawn, "takesDamage", { get: () => true, set() {} });
  Object.defineProperty(p.pawn, "velocityModifier", { get: () => 1, set() {} });
  Object.defineProperty(p.pawn, "moveType", { get: () => 2, set() {} });
  for (const [name, args] of [["sm_god", ["@all", "1"]], ["sm_speed", ["@all", "2"]], ["sm_freeze", ["@all", "5"]]]) {
    f.call(name, args); assert.equal(f.replies.at(-1)[1], 0);
  }
  assert.equal(f.timers.size, 0);
});

test("unblind sends transparent purge fade", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); f.call("sm_unblind", ["@all"]);
  assert.deepEqual(f.fades.at(-1), [1, { duration: 0, holdTime: 0, color: 0, flags: 16 }]);
  assert.equal(f.replies.at(-1)[1], 1);
});

test("speed refreshes a stable multiplier before movement and 1 disables refresh", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
  f.call("sm_speed", ["@all", "2"]); assert.equal(p.pawn.velocityModifier, 2);
  for (let i = 0; i < 3; i++) { p.pawn.velocityModifier = 1; f.move(p); assert.equal(p.pawn.velocityModifier, 2); }
  assert.equal(p.pawn.movementServices.maxspeed, 250, "do not multiply a derived cap alongside velocityModifier");
  f.call("sm_speed", ["@all", "1"]); p.pawn.velocityModifier = .8; f.move(p); assert.equal(p.pawn.velocityModifier, .8);
});

test("drug keeps copied original roll, preserves current aim, and expiry clears fade", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); const original = p.pawn.eyeAngles;
  f.call("sm_drug", ["@all", "2"]); f.tick(1000);
  assert.ok(f.fades.some(([, opts]) => opts.color !== 0));
  assert.ok(f.fades.filter(([, opts]) => opts.color !== 0).every(([, opts]) => (opts.color >>> 24) === 128), "Color32 has alpha in the high byte");
  assert.deepEqual(original, { x: 11, y: 22, z: 7 }, "eye-angle snapshots are copied before changing roll");
  p.pawn.eyeAngles.x = 42; p.pawn.eyeAngles.y = 81;
  f.tick(1000);
  assert.deepEqual(f.teleports.at(-1).angles, [42, 81, 7]);
  assert.equal(f.fades.at(-1)[1].flags, 16); assert.equal(f.timers.size, 0);
});

for (const operation of ["fade", "teleport"]) test(`drug reports a failed initial ${operation} without retaining timers`, () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
  if (operation === "fade") f.failFades(); else p.pawn.failTeleport = true;
  f.call("sm_drug", ["@all", "5"]);
  assert.equal(f.replies.at(-1)[1], 0); assert.equal(f.timers.size, 0);
});

test("permanent drug lasts until undrug and reapplying preserves the original roll", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
  f.call("sm_drug", ["@all"]); f.tick(3000); f.call("sm_drug", ["@all", "0"]); f.tick(1000);
  f.call("sm_undrug", ["@all"]);
  assert.equal(p.pawn.eyeAngles.z, 7); assert.equal(f.fades.at(-1)[1].flags, 16); assert.equal(f.timers.size, 0);
  assert.equal(f.replies.at(-1)[1], 1);
});

test("beacon creates a bounded visible expanding ring and toggling removes every entity", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); f.call("sm_beacon", ["@all"]);
  assert.ok(f.beams.length >= 8 && f.beams.length <= 24); const count = f.beams.length;
  const before = f.beams[0].start.x; f.tick(500);
  assert.equal(f.beams.length, count); assert.notEqual(f.beams[0].start.x, before);
  assert.ok(f.beams.every(b => !b.removed));
  f.call("sm_beacon", ["@all"]); assert.ok(f.beams.every(b => b.removed)); assert.equal(f.timers.size, 0);
});

for (const boundary of ["disconnect", "death", "pawn-replacement", "map", "map-end", "unload"]) {
  test(`${boundary} tears down drug, beacon, speed and freeze without touching another pawn`, () => {
    const f = fixture(), p = f.addPlayer(); f.setTargets([p]); const old = p.pawn;
    f.call("sm_speed", ["@all", "2"]); f.call("sm_drug", ["@all", "5"]);
    f.call("sm_beacon", ["@all"]); f.call("sm_freeze", ["@all", "5"]); f.tick(1000);
    let replacement;
    if (boundary === "disconnect") { f.players.delete(1); replacement = f.addPlayer(1); f.api.OnClientDisconnect(p.client); }
    if (boundary === "death") { old.lifeState = 1; f.event("player_death", 1); replacement = { pawn: f.addPawn(p) }; }
    if (boundary === "pawn-replacement") { replacement = { pawn: f.addPawn(p) }; f.api.OnGameFrame(); }
    if (boundary === "map") f.api.OnMapStart("de_next");
    if (boundary === "map-end") f.api.OnMapEnd();
    if (boundary === "unload") f.api.OnPluginEnd();
    const n = f.teleports.length; f.tick(10000); if (replacement) f.move(f.players.get(1));
    assert.equal(f.teleports.length, n); assert.ok(f.beams.every(b => b.removed)); assert.equal(f.timers.size, 0);
    if (replacement) { assert.equal(replacement.pawn.moveType, 2); assert.equal(replacement.pawn.velocityModifier, 1); assert.equal(replacement.pawn.eyeAngles.z, 7); }
  });
}

test("freeze expiry cannot unfreeze a reused slot and explicit unfreeze cancels expiry", async () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]);
  f.call("sm_freeze", ["@all", "1"]); f.players.delete(1); const q = f.addPlayer(1); q.pawn.moveType = 7;
  f.tick(1000); await Promise.resolve(); assert.equal(q.pawn.moveType, 7);
  f.setTargets([q]); f.call("sm_freeze", ["@all", "2"]); f.call("sm_unfreeze", ["@all"]);
  q.pawn.moveType = 7; f.tick(2000); await Promise.resolve(); assert.equal(q.pawn.moveType, 7); assert.equal(f.timers.size, 0);
});

test("failed beacon creation releases partially created world entities", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); f.failBeams(5); f.call("sm_beacon", ["@all"]);
  assert.equal(f.replies.at(-1)[1], 0); assert.ok(f.beams.every(b => b.removed)); assert.equal(f.timers.size, 0);
});

test("timer admission failure rolls back newly enabled effects", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); f.failTimers();
  for (const name of ["sm_drug", "sm_beacon"]) { f.call(name, ["@all", "1"]); assert.equal(f.replies.at(-1)[1], 0); }
  assert.ok(f.beams.every(b => b.removed)); assert.equal(p.pawn.eyeAngles.z, 7); assert.equal(f.timers.size, 0);
});

test("reentrant unload during beam creation removes the just-returned unowned beam", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); f.onBeam(() => f.api.OnPluginEnd());
  f.call("sm_beacon", ["@all"]); assert.ok(f.beams.every(b => b.removed)); assert.equal(f.timers.size, 0);
});

test("reentrant connection replacement while fading cannot rotate the replacement", () => {
  const f = fixture(), p = f.addPlayer(); f.setTargets([p]); f.call("sm_drug", ["@all"]);
  f.onFade(() => { f.players.delete(1); f.addPlayer(1); f.api.OnClientDisconnect(p.client); });
  const before = f.teleports.length; f.tick(1000);
  assert.equal(f.teleports.length, before); assert.equal(f.timers.size, 0);
});

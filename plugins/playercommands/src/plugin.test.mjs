import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { buildSync } from "esbuild";

const source = buildSync({
  entryPoints: [new URL("./plugin.ts", import.meta.url).pathname],
  bundle: true, write: false, platform: "node", format: "cjs", external: ["@s2script/*"],
}).outputFiles[0].text;

function fixture() {
  const commands = new Map(), players = new Map(), targets = new Map(), menu = new Map();
  const replies = [], targetCalls = [], frames = [], events = [];
  let serial = 1, frameFails = false;
  const ref = valid => ({ index: serial, id: serial++, isValid: valid });
  const sdk = {
    command: { admin: (name, flags, handler) => commands.set(name, { flags, handler }) },
    ADMFLAG: { SLAY: 32, CHEATS: 16384, KICK: 4 }, HookResult: { Handled: 2 },
    topmenu: { addTab() {}, addItem: (_tab, item) => menu.set(item.id, item) },
    translations: { load() {} }, Translations: { translate: (_slot, key) => key },
    nextFrame: () => frameFails ? Promise.reject(new Error("AsyncQueueFull")) : new Promise(resolve => frames.push(resolve)),
  };
  const cs2 = {
    CsItem: { AK47: "weapon_ak47", M4A1S: "weapon_m4a1_silencer", USPS: "weapon_usp_silencer", HighExplosive: "weapon_hegrenade", Kevlar: "item_kevlar" },
    Player: {
      // Legacy fromSlot may omit pawnless controllers; admin targets/allConnected include them.
      fromSlot: slot => players.get(slot)?.pawn ? players.get(slot) : null,
      allConnected: () => [...players.values()],
      target(pattern, callerSlot, filterImmunity) {
        targetCalls.push({ pattern, callerSlot, filterImmunity });
        const selected = targets.get(pattern) ?? [];
        return selected.filter(p => !filterImmunity || !p.immune);
      },
    },
    Events: { fire: (...args) => events.push(args) }, pickPlayer() {},
  };
  const context = vm.createContext({ module: { exports: {} }, console, require(name) {
    if (name === "@s2script/sdk") return sdk;
    if (name === "@s2script/cs2") return cs2;
    throw new Error(`Unexpected import ${name}`);
  } });
  vm.runInContext(source, context);
  const api = context.module.exports;
  api.OnPluginStart();
  function pawn(p, health = 100) {
    const body = {
      health, armorValue: 0, valid: true, origin: { x: 10, y: 20, z: 30 }, eyeAngles: { x: 4, y: 5, z: 6 },
      hasDeathInfo: true, deathInfoOrigin: { x: 40, y: 50, z: 60 }, absVelocity: { x: 1, y: 2, z: 3 },
      weapons: [], teleports: [], gives: [], removes: [], giveFails: false,
      get isValid() { return this.valid; },
      setVelocity(x, y, z) { this.absVelocity = { x, y, z }; return true; },
      slay() { this.health = 0; },
      disarm() { if (this.disarmFails) return false; this.weapons = []; return true; },
      giveNamedItem(name) {
        this.gives.push(name);
        if (this.giveFails) return null;
        const gear = name.includes("usp") ? 1 : name.includes("grenade") ? 3 : 0;
        const given = weapon(gear); this.weapons.push(given);
        this.onGive?.(given);
        return given;
      },
      removeWeapon(w) {
        this.removes.push(w); if (w.removeFails) return false;
        this.weapons = this.weapons.filter(item => item !== w); w.valid = false;
        this.onRemove?.(w); return true;
      },
    };
    body.ref = ref(() => body.valid);
    body.ref.teleport = (pos, angles, velocity) => {
      if (!body.valid || body.teleportFails) return false;
      body.teleports.push([Array.from(pos), angles && Array.from(angles), Array.from(velocity)]);
      body.onTeleport?.(pos, angles, velocity);
      return true;
    };
    p.pawn = body;
    return body;
  }
  function weapon(gearSlot) {
    const item = { valid: true, data: { gearSlot }, isValid() { return this.valid; } };
    item.ref = ref(() => item.valid);
    return item;
  }
  function player(slot, health = 100) {
    const p = { slot, userId: 100 + serial, steamId: `7656119800000000${slot}`, teamNum: 2, switches: [], respawns: 0,
      get pawnIsAlive() { return (this.pawn?.health ?? 0) > 0; },
      switchTeam(team) { this.switches.push(team); if (!this.switchFails) this.teamNum = team; this.onSwitch?.(); },
      respawn() {
        this.respawns++; if (this.respawnFails || this.pawnIsAlive) return false;
        if (this.pawn) this.pawn.valid = false;
        pawn(this); this.onRespawn?.(); return true;
      },
      setName(name) { this.playerName = name; return true; }, playerName: `player${slot}`,
    };
    p.ref = ref(() => players.get(slot) === p);
    players.set(slot, p); pawn(p, health); targets.set(`#${slot}`, [p]);
    return p;
  }
  return { api, commands, players, replies, targetCalls, menu, events, player, pawn, weapon,
    targets(pattern, selected) { targets.set(pattern, selected); },
    call(name, args = [], callerSlot = -1) {
      const entry = commands.get(name); assert.ok(entry, `${name} registered`);
      return entry.handler({ callerSlot, arg: n => args[n] ?? "", argInt: (n, fallback) => parseInt(args[n], 10) || fallback,
        argsFrom: n => args.slice(n).join(" "), replyT: (...args) => replies.push(args) });
    },
    failFrame() { frameFails = true; },
    async frame() { for (const resolve of frames.splice(0)) resolve(); await new Promise(resolve => setImmediate(resolve)); },
  };
}

test("new aliases retain command privileges and immunity-filtered target resolution", () => {
  const f = fixture(), p = f.player(1), immune = f.player(2); immune.immune = true;
  f.targets("@all", [p, immune]);
  for (const name of ["sm_health", "sm_hp", "sm_armor", "sm_disarm", "sm_strip", "sm_respawn", "sm_1up", "sm_oneup", "sm_tp", "sm_teleport"]) {
    assert.equal(f.commands.get(name)?.flags, 32, name);
  }
  for (const name of ["sm_team", "sm_spec", "sm_swap"]) assert.equal(f.commands.get(name)?.flags, 4, name);
  for (const name of ["sm_give", "sm_equip", "sm_saveloc"]) assert.equal(f.commands.get(name)?.flags, 16384, name);
  assert.equal(f.call("sm_hp", ["@all", "150"]), 2);
  assert.equal(p.pawn.health, 150); assert.equal(immune.pawn.health, 100);
  assert.deepEqual(f.targetCalls.at(-1), { pattern: "@all", callerSlot: -1, filterImmunity: true });
});

test("health and armor validate entire integer tokens and enforce inclusive ranges", () => {
  const f = fixture(), p = f.player(1);
  for (const [name, bad] of [["sm_health", ["", "NaN", "Infinity", "12abc", "1.5", "0", "1000001", "1e3", "0x10"]], ["sm_armor", ["", "NaN", "Infinity", "4x", "-1", "251", "1.5"]]]) {
    for (const value of bad) { f.call(name, ["#1", value]); assert.equal(p.pawn.health, 100); assert.equal(p.pawn.armorValue, 0); }
  }
  f.call("sm_health", ["#1", "1000000"]); assert.equal(p.pawn.health, 1000000);
  f.call("sm_hp", ["#1", "1"]); assert.equal(p.pawn.health, 1);
  f.call("sm_armor", ["#1", "250"]); assert.equal(p.pawn.armorValue, 250);
  f.call("sm_armor", ["#1", "0"]); assert.equal(p.pawn.armorValue, 0);
});

test("setters skip dead, staged and unreadable pawns and report applied targets", () => {
  const f = fixture(), alive = f.player(1), dead = f.player(2, 0), staged = f.player(3), unreadable = f.player(4);
  staged.pawn.valid = false; unreadable.pawn.armorValue = null;
  f.targets("@all", [alive, dead, staged, unreadable]);
  f.call("sm_armor", ["@all", "75"]);
  assert.equal(alive.pawn.armorValue, 75); assert.equal(dead.pawn.armorValue, 0); assert.equal(staged.pawn.armorValue, 0);
  assert.equal(unreadable.pawn.armorValue, null); assert.equal(f.replies.at(-1)[1], 1);
});

test("disarm aliases preserve failed targets in success counts", () => {
  const f = fixture(), first = f.player(1), second = f.player(2); second.pawn.disarmFails = true;
  first.pawn.weapons.push(f.weapon(0)); second.pawn.weapons.push(f.weapon(1));
  f.targets("@all", [first, second]);
  assert.equal(f.call("sm_strip", ["@all"]), 2);
  assert.equal(first.pawn.weapons.length, 0); assert.equal(second.pawn.weapons.length, 1); assert.equal(f.replies.at(-1)[1], 1);
});

test("give resolves CsItem enum/class/unprefixed aliases and rejects unknown input before inventory mutation", () => {
  const f = fixture(), p = f.player(1);
  for (const input of ["AK47", "weapon_ak47", "ak47", "M4A1S", "m4a1_silencer", "HighExplosive", "item_kevlar", "kevlar"]) f.call("sm_give", ["#1", input]);
  assert.deepEqual(p.pawn.gives, ["weapon_ak47", "weapon_ak47", "weapon_ak47", "weapon_m4a1_silencer", "weapon_m4a1_silencer", "weapon_hegrenade", "item_kevlar", "item_kevlar"]);
  f.call("sm_equip", ["#1", "weapon_arbitrary"]); assert.equal(p.pawn.gives.length, 8); assert.equal(p.pawn.removes.length, 0);
  p.pawn.giveFails = true; f.call("sm_give", ["#1", "ak47"]); assert.equal(f.replies.at(-1)[1], 0);
});

test("equip creates first, replaces only the new weapon gear slot, and preserves inventory on creation failure", () => {
  const f = fixture(), p = f.player(1), rifle = f.weapon(0), pistol = f.weapon(1), grenade = f.weapon(3);
  p.pawn.weapons.push(rifle, pistol, grenade); p.pawn.giveFails = true;
  f.call("sm_equip", ["#1", "ak47"]); assert.equal(p.pawn.removes.length, 0); assert.equal(p.pawn.weapons.length, 3);
  p.pawn.giveFails = false; f.call("sm_equip", ["#1", "ak47"]);
  assert.deepEqual(p.pawn.removes, [rifle]); assert.equal(p.pawn.weapons.includes(pistol), true); assert.equal(p.pawn.weapons.includes(grenade), true);
  assert.equal(p.pawn.weapons.length, 3); assert.equal(f.replies.at(-1)[1], 1);
});

test("equip cannot remove old inventory after a give hook replaces the pawn or invalidates the created item", () => {
  for (const event of ["pawn", "item", "slot"]) {
    const f = fixture(), p = f.player(1), old = p.pawn; old.weapons.push(f.weapon(0));
    old.onGive = given => { if (event === "pawn") f.pawn(p); if (event === "item") given.valid = false; if (event === "slot") given.data.gearSlot = null; };
    f.call("sm_equip", ["#1", "ak47"]);
    assert.equal(old.removes.length, event === "slot" ? 1 : 0, event);
    assert.equal(old.weapons[0].data.gearSlot, 0, "the old rifle remains");
    assert.equal(f.replies.at(-1)[1], 0, event);
  }
});

test("team and respawn operate on dead or pawnless controllers, validate aliases, and count real outcomes", () => {
  const f = fixture(), dead = f.player(1, 0), pawnless = f.player(2, 0), failed = f.player(3, 0);
  pawnless.pawn = null; failed.switchFails = true; failed.respawnFails = true;
  f.targets("@all", [dead, pawnless, failed]);
  f.call("sm_team", ["@all", "ct"]); assert.equal(dead.teamNum, 3); assert.equal(pawnless.teamNum, 3); assert.equal(f.replies.at(-1)[1], 2);
  f.call("sm_respawn", ["@all"]); assert.equal(dead.pawn.health, 100); assert.equal(pawnless.pawn.health, 100); assert.equal(f.replies.at(-1)[1], 2);
  f.call("sm_spec", ["#1"]); assert.equal(dead.teamNum, 1);
  f.call("sm_swap", ["#1"]); assert.equal(dead.teamNum, 1);
  f.call("sm_team", ["#1", "t"]); f.call("sm_swap", ["#1"]); assert.equal(dead.teamNum, 3);
  const count = dead.switches.length; f.call("sm_team", ["#1", "3garbage"]); assert.equal(dead.switches.length, count);
});

test("team switch may replace a pawn synchronously without subsequent writes to either pawn", () => {
  const f = fixture(), p = f.player(1), before = p.pawn;
  p.onSwitch = () => { before.valid = false; f.pawn(p); };
  f.call("sm_team", ["#1", "ct"]); assert.equal(p.teamNum, 3); assert.equal(p.pawn.health, 100); assert.equal(before.health, 100);
});

test("oneup copies death origin before respawn and teleports the new pawn on the following frame", async () => {
  const f = fixture(), p = f.player(1, 0), dead = p.pawn;
  p.onRespawn = () => { dead.deathInfoOrigin.x = 999; };
  assert.equal(f.call("sm_oneup", ["#1"]), 2);
  const spawned = p.pawn; assert.notEqual(spawned, dead); assert.equal(spawned.teleports.length, 0);
  await f.frame(); assert.deepEqual(spawned.teleports, [[[40, 50, 60], null, [0, 0, 0]]]); assert.equal(f.replies.at(-1)[1], 1);
});

test("oneup rejects living players, missing death data and failed respawns", async () => {
  const f = fixture(), alive = f.player(1), noInfo = f.player(2, 0), failed = f.player(3, 0), noPawn = f.player(4, 0);
  noInfo.pawn.hasDeathInfo = false; failed.respawnFails = true; noPawn.pawn = null;
  f.targets("@all", [alive, noInfo, failed, noPawn]); f.call("sm_1up", ["@all"]); await f.frame();
  assert.equal(alive.respawns, 0); assert.equal(noInfo.respawns, 0); assert.equal(noPawn.respawns, 0); assert.equal(failed.pawn.teleports.length, 0); assert.equal(f.replies.at(-1)[1], 0);
});

test("oneup queued work cannot touch a new connection, later life, new map or unloaded plugin", async () => {
  for (const event of ["reconnect", "new-life", "map", "end"]) {
    const f = fixture(), p = f.player(1, 0); f.call("sm_1up", ["#1"]); const spawned = p.pawn;
    if (event === "reconnect") f.player(1);
    if (event === "new-life") f.pawn(p);
    if (event === "map") f.api.OnMapStart("de_nuke");
    if (event === "end") f.api.OnPluginEnd();
    await f.frame(); assert.equal(spawned.teleports.length, 0, event); assert.equal(p.pawn.teleports.length, 0, event);
  }
});

test("teleport copies destination position/view angles and zeroes velocity for each successful target", () => {
  const f = fixture(), first = f.player(1), second = f.player(2), dest = f.player(3), failed = f.player(4);
  failed.pawn.teleportFails = true; f.targets("@all", [first, second, failed]);
  first.pawn.onTeleport = (pos, angles) => { pos[0] = 999; angles[0] = 999; dest.pawn.origin.x = 888; };
  assert.equal(f.call("sm_teleport", ["@all", "#3"]), 2);
  assert.deepEqual(first.pawn.teleports, [[[10, 20, 30], [4, 5, 6], [0, 0, 0]]]);
  assert.deepEqual(second.pawn.teleports, [[[10, 20, 30], [4, 5, 6], [0, 0, 0]]]); assert.equal(f.replies.at(-1)[1], 2);
});

test("teleport rejects missing, ambiguous and unavailable destinations", () => {
  const f = fixture(), p = f.player(1), second = f.player(2); f.targets("ambiguous", [p, second]);
  for (const destination of ["missing", "ambiguous"]) { f.call("sm_tp", ["#1", destination]); assert.equal(p.pawn.teleports.length, 0); }
  second.pawn = null; f.call("sm_tp", ["#1", "#2"]); assert.equal(p.pawn.teleports.length, 0);
});

test("saveloc can overwrite copied transforms and teleport uses the caller's connection-bound saved location", () => {
  const f = fixture(), admin = f.player(1), p = f.player(2);
  assert.equal(f.call("sm_saveloc", [], 1), 2); admin.pawn.origin.x = 90;
  f.call("sm_tp", ["#2"], 1); assert.equal(p.pawn.teleports.at(-1)[0][0], 10);
  f.call("sm_saveloc", [], 1); f.call("sm_tp", ["#2"], 1); assert.equal(p.pawn.teleports.at(-1)[0][0], 90);
  f.call("sm_tp", ["#2"]); assert.equal(p.pawn.teleports.length, 2, "console has no saved location");
});

test("saved locations clear on map, disconnect and unload and never transfer to a replacement connection", () => {
  for (const event of ["map", "disconnect", "reconnect", "end"]) {
    const f = fixture(), admin = f.player(1), p = f.player(2); f.call("sm_saveloc", [], 1);
    if (event === "map") f.api.OnMapStart("de_nuke");
    if (event === "disconnect") f.api.OnClientDisconnect({ slot: 1 });
    if (event === "reconnect") f.player(1);
    if (event === "end") f.api.OnPluginEnd();
    f.call("sm_tp", ["#2"], admin.slot); assert.equal(p.pawn.teleports.length, 0, event);
  }
});

test("existing slap, slay, rename and menu registrations retain behavior", () => {
  const f = fixture(), p = f.player(1);
  f.call("sm_slap", ["#1", "5"]); assert.equal(p.pawn.health, 95);
  f.call("sm_rename", ["#1", "{red}new"]); assert.equal(p.playerName, "rednew"); assert.equal(f.events[0][0], "player_changename");
  f.call("sm_slay", ["#1"]); assert.equal(p.pawn.health, 0);
  assert.equal(f.menu.size, 2);
});

test("a map change during teleport stops the remaining targets from using an old saved location", () => {
  const f = fixture(), admin = f.player(1), first = f.player(2), second = f.player(3);
  f.call("sm_saveloc", [], admin.slot); f.targets("@all", [first, second]);
  first.pawn.onTeleport = () => f.api.OnMapStart("de_nuke");
  f.call("sm_tp", ["@all"], admin.slot);
  assert.equal(first.pawn.teleports.length, 1); assert.equal(second.pawn.teleports.length, 0);
});

test("a map change during oneup teleport stops remaining frame jobs", async () => {
  const f = fixture(), first = f.player(1, 0), second = f.player(2, 0); f.targets("@all", [first, second]);
  f.call("sm_1up", ["@all"]); first.pawn.onTeleport = () => f.api.OnMapStart("de_nuke");
  await f.frame(); assert.equal(first.pawn.teleports.length, 1); assert.equal(second.pawn.teleports.length, 0);
});

test("swap only exchanges T and CT and leaves spectators or unassigned controllers alone", () => {
  const f = fixture(), spectator = f.player(1), unassigned = f.player(2), terrorist = f.player(3), ct = f.player(4);
  spectator.teamNum = 1; unassigned.teamNum = 0; ct.teamNum = 3;
  f.targets("@all", [spectator, unassigned, terrorist, ct]); f.call("sm_swap", ["@all"]);
  assert.equal(spectator.teamNum, 1); assert.equal(unassigned.teamNum, 0); assert.equal(terrorist.teamNum, 3); assert.equal(ct.teamNum, 2);
  assert.equal(f.replies.at(-1)[1], 2);
});

test("generic team selection accepts standard names and rejects server-role aliases", () => {
  const f = fixture(), p = f.player(1);
  for (const name of ["prisoner", "prisoners", "p", "guard", "guards", "g"]) f.call("sm_team", ["#1", name]);
  assert.deepEqual(p.switches, []);
  f.call("sm_team", ["#1", "Counter-Terrorist"]); assert.equal(p.teamNum, 3);
  f.call("sm_team", ["#1", "spectators"]); assert.equal(p.teamNum, 1);
});


test("oneup reports next-frame admission failure without an unhandled rejection or false success", async () => {
  const f = fixture(), p = f.player(1, 0); f.failFrame();
  assert.equal(f.call("sm_1up", ["#1"]), 2);
  await f.frame(); assert.equal(p.pawn.teleports.length, 0);
  assert.deepEqual(f.replies.at(-1), ["Oneup Scheduling Failed"]);
});

test("equip rolls back only the new item when VData cannot identify its gear slot", () => {
  const f = fixture(), p = f.player(1), rifle = f.weapon(0), pistol = f.weapon(1);
  p.pawn.weapons.push(rifle, pistol); let created;
  p.pawn.onGive = given => { given.data = null; created = given; };
  f.call("sm_equip", ["#1", "ak47"]);
  assert.deepEqual(p.pawn.weapons, [rifle, pistol]); assert.deepEqual(p.pawn.removes, [created]);
  assert.equal(f.replies.at(-1)[1], 0);
});

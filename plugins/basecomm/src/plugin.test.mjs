import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";
import { transformSync } from "esbuild";

const source = transformSync(
  readFileSync(new URL("./plugin.ts", import.meta.url), "utf8"),
  { loader: "ts", format: "cjs" },
).code;

const VALID_A = "76561198000000001";
const VALID_B = "76561198000000002";

function fixture() {
  const commands = new Map();
  const menuItems = new Map();
  const players = new Map();
  const listeners = new Map();
  const events = [];
  const replies = [];
  const translated = [];
  const targetCalls = [];
  let service;
  let targets = [];
  let picked = null;

  const Player = {
    target: (pattern, callerSlot, filterImmunity) => {
      targetCalls.push({ pattern, callerSlot, filterImmunity });
      return targets;
    },
    // Mirrors CS2: connection occupancy and pawn availability are independent.
    fromSlot: slot => players.get(slot)?.hasPawn ? players.get(slot) : null,
    allConnected: () => [...players.values()],
  };
  const Clients = {
    fromSlot: slot => players.get(slot)?.client ?? null,
  };
  const sdk = {
    command: {
      admin(name, flags, handler) {
        commands.set(name, { flags, handler });
      },
    },
    topmenu: {
      addTab() {},
      addItem(_tab, item) { menuItems.set(item.id, item); },
    },
    translations: { load() {} },
    ADMFLAG: { CHAT: 32 },
    HookResult: { Continue: 0, Handled: 2 },
    Clients,
    Translations: {
      translate: (slot, key, ...args) => { translated.push([slot, key, ...args]); return `${key}:${args.join(",")}`; },
    },
    publish(name, implementation) {
      assert.equal(name, "@s2script/basecomm");
      service = implementation;
      return {
        emit(event, payload) {
          const copied = structuredClone(payload);
          events.push([event, copied]);
          for (const listener of listeners.get(event) ?? []) {
            listener(structuredClone(copied));
          }
        },
      };
    },
  };
  const cs2 = {
    Player,
    pickPlayer: (_adminSlot, callback) => { if (picked) callback(picked); },
  };
  const context = vm.createContext({
    module: { exports: {} },
    console,
    require(name) {
      if (name === "@s2script/sdk") return sdk;
      if (name === "@s2script/cs2") return cs2;
      throw new Error(`unexpected import ${name}`);
    },
  });
  vm.runInContext(source, context);
  const api = context.module.exports;
  api.OnPluginStart();

  const addPlayer = (steamId, slot) => {
    const client = { slot, steamId, voiceMuted: false };
    const player = {
      hasPawn: true,
      slot,
      steamId,
      userId: 100 + slot,
      playerName: `player-${slot}`,
      hasCommunicationAbuseMute: false,
      client,
    };
    players.set(slot, player);
    return player;
  };
  const call = (name, pattern = "@all", callerSlot = -1) => {
    const registered = commands.get(name);
    assert.ok(registered, `${name} registered`);
    return registered.handler({
      callerSlot,
      arg: index => index === 0 ? pattern : "",
      reply: message => replies.push(message),
    });
  };
  const on = (event, listener) => {
    const current = listeners.get(event) ?? [];
    current.push(listener);
    listeners.set(event, current);
  };
  return {
    api,
    service,
    commands,
    menuItems,
    players,
    events,
    replies,
    translated,
    targetCalls,
    addPlayer,
    call,
    on,
    setTargets(next) { targets = next; },
    setPicked(next) { picked = next; },
  };
}

test("public policy methods accept canonical u64 identities, support offline state, and notify only on transitions", () => {
  const f = fixture();
  assert.equal(f.service.isMuted(VALID_A), false);
  assert.equal(f.service.isGagged(VALID_A), false);
  assert.equal(f.service.setMuted(VALID_A, true), true);
  assert.equal(f.service.setGagged(VALID_A, true), true);
  assert.equal(f.service.isMuted(VALID_A), true);
  assert.equal(f.service.isGagged(VALID_A), true);
  assert.deepEqual(f.events, [
    ["OnClientMuteChanged", { steamId: VALID_A, state: true }],
    ["OnClientGagChanged", { steamId: VALID_A, state: true }],
  ]);

  assert.equal(f.service.setMuted(VALID_A, true), true);
  assert.equal(f.service.setGagged(VALID_A, true), true);
  assert.equal(f.events.length, 2, "unchanged accepted policy emits nothing");

  for (const bad of ["", "0", "01", "+1", "-1", " 1", "1.0", "18446744073709551616", 1, null]) {
    assert.equal(f.service.isMuted(bad), false, `invalid query ${String(bad)}`);
    assert.equal(f.service.isGagged(bad), false, `invalid query ${String(bad)}`);
    assert.equal(f.service.setMuted(bad, true), false, `invalid mute ${String(bad)}`);
    assert.equal(f.service.setGagged(bad, true), false, `invalid gag ${String(bad)}`);
  }
  assert.equal(f.service.setMuted(VALID_B, 1), false);
  assert.equal(f.service.setGagged(VALID_B, "true"), false);
  assert.equal(f.events.length, 2, "invalid calls have no side effects");
});

test("mute state and both engine properties are updated before notification and survive reentrant changes", () => {
  const f = fixture();
  const p = f.addPlayer(VALID_A, 3);
  const observed = [];
  f.on("OnClientMuteChanged", payload => {
    observed.push({
      payload,
      queried: f.service.isMuted(VALID_A),
      voiceMuted: p.client.voiceMuted,
      abuseMute: p.hasCommunicationAbuseMute,
    });
    if (payload.state) f.service.setMuted(VALID_A, false);
  });

  assert.equal(f.service.setMuted(VALID_A, true), false, "outer result reports the listener's current policy");
  assert.deepEqual(observed, [
    { payload: { steamId: VALID_A, state: true }, queried: true, voiceMuted: true, abuseMute: true },
    { payload: { steamId: VALID_A, state: false }, queried: false, voiceMuted: false, abuseMute: false },
  ]);
  assert.equal(f.service.isMuted(VALID_A), false);
  assert.equal(p.client.voiceMuted, false, "outer setter cannot overwrite the reentrant result");
  assert.equal(p.hasCommunicationAbuseMute, false);
});

test("command, silence, and menu paths converge on policy operations with snapshotted identities and honest counts", () => {
  const f = fixture();
  const first = f.addPlayer(VALID_A, 1);
  const second = f.addPlayer(VALID_B, 2);
  const invalid = f.addPlayer("0", 3);
  f.setTargets([first, second, invalid]);
  f.on("OnClientGagChanged", payload => {
    if (payload.steamId === VALID_A && payload.state) {
      second.steamId = "76561198000000999";
    }
  });
  f.on("OnClientMuteChanged", payload => {
    if (payload.steamId === VALID_A && payload.state) {
      // This runs after the gag setter returned true. The combined silence result must query
      // both final policies after the mute notification, rather than trust that stale boolean.
      f.service.setGagged(VALID_A, false);
    }
  });

  assert.equal(f.call("sm_silence"), 2);
  assert.equal(f.service.isGagged(VALID_A), false, "reentrant listener owns the final gag state");
  assert.equal(f.service.isMuted(VALID_A), true);
  assert.equal(f.service.isGagged(VALID_B), true, "later target identity was copied before callbacks");
  assert.equal(f.service.isMuted(VALID_B), true);
  assert.equal(f.service.isGagged("76561198000000999"), false);
  assert.equal(f.service.isMuted("0"), false);
  assert.equal(f.replies.at(-1), "Silenced Player:1", "only identities left in the requested state are counted");
  assert.deepEqual(f.targetCalls.at(-1), { pattern: "@all", callerSlot: -1, filterImmunity: true });
  assert.ok([...f.commands.values()].every(command => command.flags === 32), "all command permission gates stay CHAT");

  second.steamId = VALID_B;
  f.call("sm_unsilence");
  assert.deepEqual(f.targetCalls.at(-1), { pattern: "@all", callerSlot: -1, filterImmunity: false });
  assert.equal(f.service.isGagged(VALID_B), false);
  assert.equal(f.service.isMuted(VALID_B), false);

  const menuTarget = f.addPlayer("76561198000000003", 4);
  f.setPicked(menuTarget);
  const menu = f.menuItems.get("basecomm:gag");
  assert.ok(menu);
  assert.equal(menu.flags, 32);
  menu.onSelect(7);
  assert.equal(f.service.isGagged(menuTarget.steamId), true);
  assert.ok(f.events.some(([name, payload]) => name === "OnClientGagChanged" && payload.steamId === menuTarget.steamId));
});

test("reconnect restores real voice mute and scoreboard state without a new notification", () => {
  const f = fixture();
  assert.equal(f.service.setMuted(VALID_A, true), true);
  const before = f.events.length;
  const p = f.addPlayer(VALID_A, 5);
  f.api.OnClientPutInServer(p.client);
  assert.equal(p.client.voiceMuted, true);
  assert.equal(p.hasCommunicationAbuseMute, true);
  assert.equal(f.events.length, before);
});

test("gagged chat suppression reads canonical policy state", () => {
  const f = fixture();
  f.addPlayer(VALID_A, 4);
  assert.equal(f.api.OnClientSayCommand(4, "hello", false), 0);
  f.service.setGagged(VALID_A, true);
  assert.equal(f.api.OnClientSayCommand(4, "hello", false), 2);
});

test("command callbacks cannot reply to a disconnected or replaced actor after policy notifications", () => {
  for (const replacement of ["absent", "same-steam-new-user", "same-user-new-steam"]) {
    const f = fixture();
    const target = f.addPlayer(VALID_A, 1);
    const actor = f.addPlayer(VALID_B, 2);
    f.setTargets([target]);
    f.on("OnClientGagChanged", () => {
      f.players.delete(2);
      if (replacement === "same-steam-new-user") f.addPlayer(VALID_B, 2).userId = actor.userId + 1;
      if (replacement === "same-user-new-steam") f.addPlayer("76561198000000003", 2);
      f.translated.length = 0;
    });
    f.call("sm_gag", "#101", 2);
    assert.equal(f.service.isGagged(VALID_A), true, "policy still changes");
    assert.deepEqual(f.replies, [], "raw command reply must not reach a replacement actor");
    assert.deepEqual(f.translated, [], "no post-callback translation through the old actor slot");
  }
});

for (const timing of ["before-command", "during-callback"]) test(`BaseComm replies to its same pawnless actor ${timing}`, () => {
  const f = fixture(); const target = f.addPlayer(VALID_A, 1); const actor = f.addPlayer(VALID_B, 2);
  f.setTargets([target]);
  if (timing === "before-command") actor.hasPawn = false;
  f.on("OnClientGagChanged", () => { actor.hasPawn = false; });
  f.call("sm_gag", "#101", 2);
  assert.equal(f.service.isGagged(VALID_A), true);
  assert.deepEqual(f.replies, ["Gagged Player:1"]);
  assert.deepEqual(f.translated.at(-1), [2, "Gagged Player", 1]);
});

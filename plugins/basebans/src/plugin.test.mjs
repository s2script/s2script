import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";
import { transformSync } from "esbuild";

const source = transformSync(readFileSync(new URL("./plugin.ts", import.meta.url), "utf8"), { loader: "ts", format: "cjs" }).code;
const A = "76561198000000001", B = "76561198000000002";
const request = (overrides = {}) => ({ steamId: A, minutes: 5, reason: "test", source: "plugin", actorSteamId: null, ...overrides });

function fixture({ storeMode = "normal" } = {}) {
  const commands = new Map(), items = new Map(), players = new Map(), cache = new Map();
  const errors = [];
  const events = [], order = [], replies = [], translations = [], kicks = [], targetCalls = [], resolves = [], listeners = new Map();
  let service, targets = [], picked, menu;
  let now = 1_800_000_000_000;
  const on = (name, fn) => listeners.set(name, [...(listeners.get(name) ?? []), fn]);
  // Mirrors the host's copied, synchronous fail-open Hook/Notification behavior.
  const dispatch = (name, payload) => {
    order.push(name);
    let result = 0;
    for (const fn of listeners.get(name) ?? []) {
      try { result = Math.max(result, fn(structuredClone(payload)) ?? 0); } catch (error) { errors.push(error); /* host logs and continues */ }
      if (result === 3) break;
    }
    return result;
  };
  const sdk = {
    command: { admin: (name, flags, handler) => commands.set(name, { flags, handler }) },
    topmenu: { addTab() {}, addItem: (_tab, item) => items.set(item.id, item) },
    translations: { load() {} }, ADMFLAG: { BAN: 8, UNBAN: 16, KICK: 4 },
    HookResult: { Continue: 0, Changed: 1, Handled: 2, Stop: 3 },
    MenuStyle: { Center: 1 },
    Menu: class { constructor() { menu = this; } addItem() {} onSelect(fn) { this.select = fn; } display() {} },
    Clients: { fromSlot: slot => players.get(slot)?.client ?? null },
    Translations: { translate: (slot, key, ...args) => { translations.push([slot, key, ...args]); return `${key}:${args.join(",")}`; } },
    Bans: {
      add(sid, minutes, reason) {
        order.push("add");
        if (storeMode === "absent") return;
        const until = minutes > 0 ? Math.floor(now / 1000) + minutes * 60 : 0;
        cache.set(sid, { until: storeMode === "wrong-expiry" ? until + 60 : until, reason: storeMode === "wrong-reason" ? "other" : reason });
        // Real Bans.add mutates cache first; config_write's failure status is ignored.
        order.push(storeMode === "disk-failure" ? "write-failed" : "write-attempt");
      },
      get(sid) { order.push("get"); return cache.get(sid) ?? null; },
      remove(sid) { order.push("remove"); return cache.delete(sid); },
    },
    publish(name, methods) {
      assert.equal(name, "@s2script/basebans"); service = methods;
      return { dispatch, emit(name, payload) { events.push([name, structuredClone(payload)]); dispatch(name, payload); } };
    },
  };
  const Player = {
    allConnected: () => [...players.values()],
    fromSlot: slot => players.get(slot) ?? null,
    fromUserId(uid) { resolves.push(uid); return [...players.values()].find(p => p.userId === uid) ?? null; },
    target(pattern, slot, immunity) { targetCalls.push([pattern, slot, immunity]); return targets; },
  };
  const context = vm.createContext({ module: { exports: {} }, console, Date: { now: () => now }, require(name) {
    if (name === "@s2script/sdk") return sdk;
    if (name === "@s2script/cs2") return { Player, pickPlayer: (_slot, fn) => picked && fn(picked) };
    throw new Error(name);
  } });
  vm.runInContext(source, context);
  const api = context.module.exports;
  api.OnPluginStart();
  function addPlayer(sid = A, slot = 1, userId = 101) {
    const client = { slot, steamId: sid, isBot: sid === "0", chat: text => replies.push([text]), kickWithReason: text => { order.push("kick"); kicks.push([sid, slot, text]); } };
    const p = { steamId: sid, slot, userId, playerName: `name-${sid}`, client, kick: client.kickWithReason };
    players.set(slot, p); return p;
  }
  return { errors, api, service, commands, items, players, cache, events, order, replies, translations, kicks, targetCalls, resolves, on, addPlayer,
    setTargets(p) { targets = p; }, setPicked(p) { picked = p; }, getMenu: () => menu, setNow(value) { now = value; },
    call(name, args, callerSlot = -1) { return commands.get(name).handler({ callerSlot, arg: i => args[i] ?? "", argInt: i => Number(args[i]) | 0, argsFrom: i => args.slice(i).join(" "), replyT: (...args) => replies.push(args) }); },
  };
}

test("publishes validated offline ban/unban operations before hooks or store effects", () => {
  const f = fixture();
  assert.ok(f.service, "BaseBans publishes its service");
  const invalid = [null, {}, ...["", "0", "01", "+1", "-1", " 1", "1.0", "18446744073709551616", 1].map(steamId => request({ steamId })),
    ...[-1, .5, NaN, Infinity, Number.MAX_SAFE_INTEGER, 150119987579000].map(minutes => request({ minutes })),
    request({ actorSteamId: "0" }), request({ actorSteamId: "01" }), request({ actorSteamId: "18446744073709551616" }), request({ actorSteamId: 1 }), request({ actorSteamId: undefined }),
    request({ source: "console" }), request({ reason: null })];
  for (const bad of invalid) assert.deepEqual(structuredClone(f.service.ban(bad)), { recorded: false, result: 0 });
  for (const bad of [null, {}, { steamId: "0" }, { steamId: "01" }]) assert.equal(f.service.unban(bad), false);
  assert.deepEqual(f.order, [], "invalid requests must not dispatch, read/write store, or kick");
  assert.deepEqual(structuredClone(f.service.ban(request({ minutes: 0, reason: "", steamId: "18446744073709551615" }))), { recorded: true, result: 0 });
  assert.equal(f.events[0][1].until, 0);
});

for (const result of [0, 1, 2, 3]) test(`HookResult ${result} preserves decision and only Continue/Changed record and kick`, () => {
  const f = fixture(); f.addPlayer();
  f.on("OnBanRequested", p => { p.reason = "mutation must not patch"; return result; });
  assert.deepEqual(structuredClone(f.service.ban(request())), { recorded: result < 2, result });
  assert.equal(f.events.length, result < 2 ? 1 : 0);
  assert.equal(f.kicks.length, result < 2 ? 1 : 0);
  if (result < 2) {
    assert.deepEqual(f.events[0], ["OnBanRecorded", { request: request(), until: 1_800_000_300 }]);
    assert.deepEqual(f.order, ["OnBanRequested", "add", "write-attempt", "get", "OnBanRecorded", "kick"]);
  } else assert.deepEqual(f.order, ["OnBanRequested"]);
});

test("request listener exceptions are advisory fail-open; notification observes cache before kick", () => {
  const f = fixture(); f.addPlayer();
  f.on("OnBanRequested", () => { throw Error("listener failure"); });
  let observed = false;
  f.on("OnBanRecorded", ({ request: r, until }) => { assert.deepEqual(f.cache.get(r.steamId), { until, reason: r.reason }); assert.equal(f.kicks.length, 0); observed = true; });
  assert.equal(f.service.ban(request()).recorded, true);
  assert.equal(f.kicks.length, 1);
  assert.equal(observed, true);
  assert.equal(f.errors.length, 1);
  assert.equal(f.errors[0].message, "listener failure");
});

for (const storeMode of ["absent", "wrong-expiry", "wrong-reason", "disk-failure"]) test(`cache readback determines recorded for ${storeMode}`, () => {
  const f = fixture({ storeMode }); f.addPlayer();
  const recorded = storeMode === "disk-failure";
  assert.equal(f.service.ban(request()).recorded, recorded);
  assert.equal(f.events.length, recorded ? 1 : 0);
  assert.equal(f.kicks.length, recorded ? 1 : 0);
});

test("identical preexisting cache value is observable but does not prove a fresh write", () => {
  const f = fixture({ storeMode: "absent" });
  f.cache.set(A, { until: 1_800_000_300, reason: "test" });
  assert.equal(f.service.ban(request()).recorded, true);
});

for (const boundary of ["OnBanRequested", "OnBanRecorded"]) test(`slot reuse during ${boundary} cannot kick replacement or read stale target`, () => {
  const f = fixture(); const p = f.addPlayer(); f.setTargets([p]);
  f.on(boundary, () => {
    f.players.delete(1); f.addPlayer(B, 1, p.userId);
    for (const key of ["slot", "steamId", "playerName", "userId"]) Object.defineProperty(p, key, { get() { throw Error(`stale target ${key}`); } });
  });
  assert.equal(f.call("sm_ban", ["#101", "5", "test"]), 2);
  assert.equal(f.kicks.length, 0);
  assert.equal(f.events.length, 1);
  assert.equal(f.replies.at(-1)[0], "Ban Success");
  assert.equal(f.replies.at(-1)[1], `name-${A}`, "display name copied before callbacks");
  assert.ok(!f.translations.some(([slot]) => slot === 1), "no target translation through stale slot");
  assert.ok(f.resolves.includes(101), "kick identity re-resolved");
});

test("API snapshots live identity before hook; a later connection to the same SteamID is not kicked", () => {
  const f = fixture(); f.addPlayer();
  f.on("OnBanRequested", () => { f.players.clear(); f.addPlayer(A, 7, 202); });
  assert.equal(f.service.ban(request()).recorded, true);
  assert.equal(f.kicks.length, 0);
});

test("command/menu/API share requests; addban stays record-only; command targeting and permissions remain", () => {
  const f = fixture(); const p = f.addPlayer(); const actor = f.addPlayer(B, 2, 102); f.setTargets([p]);
  const requests = []; f.on("OnBanRequested", r => { requests.push(r); return 0; });
  f.call("sm_ban", ["#101", "5", "test"], 2);
  assert.deepEqual(requests[0], request({ source: "command", actorSteamId: B }));
  assert.deepEqual(f.targetCalls[0], ["#101", 2, true]);
  assert.equal(f.commands.get("sm_ban").flags, 8); assert.equal(f.commands.get("sm_unban").flags, 16);
  f.call("sm_addban", [A, "5", "test"]);
  assert.deepEqual(requests[1], request({ source: "command" }));
  assert.equal(f.kicks.length, 1, "addban does not kick a connected matching identity");
  f.setPicked(p); f.items.get("basebans:ban").onSelect(2); f.getMenu().select({ info: "30" });
  assert.deepEqual(requests[2], request({ minutes: 30, reason: "Banned by admin", source: "menu", actorSteamId: B }));
  assert.equal(f.items.get("basebans:ban").flags, 8);
  assert.equal(f.kicks.length, 2);
  assert.equal(actor.steamId, B);
});

test("commands report intercepted and failed outcomes honestly, reject overflow and invalid identity", () => {
  for (const mode of ["intercepted", "absent"]) {
    const f = fixture({ storeMode: mode }); const p = f.addPlayer(); f.setTargets([p]);
    if (mode === "intercepted") f.on("OnBanRequested", () => 2);
    for (const [name, args] of [["sm_ban", ["#101", "5"]], ["sm_addban", [A, "5"]]]) {
      f.call(name, args);
      assert.equal(f.replies.at(-1)[0], mode === "intercepted" ? "Ban Intercepted" : "Ban Record Failed");
    }
  }
  const f = fixture(); f.setTargets([f.addPlayer()]);
  for (const args of [["#101", "9007199254740991"], ["#101", "-1"], ["#101", ""]]) f.call("sm_ban", args);
  for (const args of [["0", "1"], ["01", "1"], [A, "9007199254740991"]]) f.call("sm_addban", args);
  f.call("sm_unban", ["0"]);
  assert.deepEqual(f.order, []);
  f.call("sm_addban", [A, "4294967296"]);
  assert.equal(f.events[0][1].request.minutes, 4294967296, "do not narrow valid minutes with argInt");
});

test("unban emits only for cache removal; reconnect enforcement never emits a record notification", () => {
  const f = fixture(); f.service.ban(request());
  const p = f.addPlayer(); f.api.OnClientConnected(p.client);
  assert.equal(f.events.length, 1); assert.equal(f.kicks.length, 1);
  assert.equal(f.service.unban({ steamId: A }), true); assert.equal(f.service.unban({ steamId: A }), false);
  assert.deepEqual(f.events[1], ["OnBanRemoved", { steamId: A }]); assert.equal(f.events.length, 2);
  f.api.OnClientConnected(p.client); assert.equal(f.kicks.length, 1);
  f.cache.set(A, { until: 1, reason: "expired" }); f.api.OnClientConnected(p.client); assert.equal(f.kicks.length, 1);
});

test("command notification callbacks cannot translate or reply through a replaced actor slot", () => {
  for (const name of ["sm_ban", "sm_addban", "sm_unban"]) {
    const f = fixture(); const target = f.addPlayer(); f.addPlayer(B, 2, 102); f.setTargets([target]);
    f.cache.set(A, { until: 0, reason: "old" });
    f.on(name === "sm_unban" ? "OnBanRemoved" : "OnBanRecorded", () => {
      f.players.delete(2); f.addPlayer("76561198000000003", 2, 102);
      f.translations.length = 0;
    });
    f.call(name, name === "sm_ban" ? ["#101", "5"] : name === "sm_addban" ? [A, "5"] : [A], 2);
    assert.equal(f.replies.length, 0, "the command reply helper captures a raw slot, so a replaced caller must receive nothing");
    assert.ok(!f.translations.some(([slot]) => slot === 2), "no translation for a replaced actor");
  }
});

test("menu duration selection retains actor ownership and a copied target identity", () => {
  const f = fixture(); const target = f.addPlayer(); f.addPlayer(B, 2, 102); f.setPicked(target);
  f.items.get("basebans:ban").onSelect(2);
  f.players.delete(2); f.addPlayer("76561198000000003", 2, 102);
  f.getMenu().select({ info: "5" });
  assert.deepEqual(f.order, [], "a replacement admin cannot confirm another connection's menu");
  f.players.delete(2); f.addPlayer(B, 2, 102);
  f.items.get("basebans:ban").onSelect(2);
  f.players.delete(1); f.addPlayer("76561198000000003", 1, 101);
  Object.defineProperty(target, "steamId", { get() { throw Error("retained target used"); } });
  f.getMenu().select({ info: "5" });
  assert.equal(f.events[0][1].request.steamId, A);
  assert.equal(f.events[0][1].request.actorSteamId, B);
  assert.equal(f.kicks.length, 0, "offline record uses the copied target, never its replacement");
});

for (const result of [2, 3]) test(`menu reports intercepted HookResult ${result} without recording or kicking`, () => {
  const f = fixture(); f.setPicked(f.addPlayer()); f.addPlayer(B, 2, 102);
  f.on("OnBanRequested", () => result);
  f.items.get("basebans:ban").onSelect(2); f.getMenu().select({ info: "5" });
  assert.deepEqual(f.order, ["OnBanRequested"]);
  assert.equal(f.replies[0][0], "Ban Intercepted:");
});

test("menu notification cannot send failure feedback through a replaced actor", () => {
  const f = fixture(); f.setPicked(f.addPlayer()); f.addPlayer(B, 2, 102);
  f.on("OnBanRequested", () => {
    f.players.delete(2); f.addPlayer("76561198000000003", 2, 102); f.translations.length = 0;
    return 2;
  });
  f.items.get("basebans:ban").onSelect(2); f.getMenu().select({ info: "5" });
  assert.equal(f.replies.length, 0);
  assert.deepEqual(f.translations, []);
});

test("offline API identity is not acquired from a connection created during the request hook", () => {
  const f = fixture(); f.on("OnBanRequested", () => { f.addPlayer(); });
  assert.equal(f.service.ban(request()).recorded, true);
  assert.equal(f.kicks.length, 0);
});

test("request callbacks may advance wall time; cache expiry must match this add call", () => {
  const f = fixture();
  f.on("OnBanRequested", () => { f.setNow(1_800_000_100_000); });
  assert.equal(f.service.ban(request()).recorded, true);
  assert.equal(f.events[0][1].until, 1_800_000_400);
});

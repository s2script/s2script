import { test } from "node:test";
import assert from "node:assert";
import vm from "node:vm";
import { buildEventModel } from "../src/eventgen/model.ts";
import { emitEventDts } from "../src/eventgen/emit-dts.ts";

const CAT = { player_death: { userid: "player", attacker: "player", weapon: "string", headshot: "bool", penetrated: "int" } };

test("buildEventModel groups fields by accessor + interface name", () => {
  const m = buildEventModel(CAT);
  const e = m.find(x => x.event === "player_death");
  assert.equal(e.iface, "PlayerDeathEvent");
  assert.deepEqual(e.byGetter.getPlayerSlot.sort(), ["attacker", "userid"]);
  assert.deepEqual(e.byGetter.getString, ["weapon"]);
  assert.deepEqual(e.byGetter.getBool, ["headshot"]);
  assert.deepEqual(e.byGetter.getInt, ["penetrated"]);
});

test("emitEventDts emits typed per-event interfaces + the GameEvents map + the typed overload", () => {
  const dts = emitEventDts(buildEventModel(CAT));
  assert.match(dts, /export interface PlayerDeathEvent extends GameEvent \{/);
  assert.match(dts, /getPlayerSlot\(key: "attacker" \| "userid"\): number;/);
  assert.match(dts, /getString\(key: "weapon"\): string;/);
  assert.match(dts, /export interface GameEvents \{[^}]*player_death: PlayerDeathEvent;/s);
  assert.match(dts, /export function on<K extends keyof GameEvents>\(name: K, handler: \(ev: GameEvents\[K\]\) => void\): void;/);
});

test("eventgen emits typed onPre<K> + fire<K>", () => {
  const out = emitEventDts(buildEventModel(CAT));
  assert.match(out, /onPre<K extends keyof GameEvents>/);
  assert.match(out, /fire<K extends keyof GameEvents>/);
});

test("events prelude: Events.onPre is a function, HookResult.Handled === 2, Events.fire degrades to false (offline vm)", () => {
  const ctx = {
    __s2_event_subscribe:       () => {},
    __s2_event_unsubscribe:     () => {},
    __s2_event_subscribe_pre:   () => {},
    __s2_event_create:          () => false,   // degrade: no engine-op
    __s2_event_fire:            () => false,
    __s2_event_set_int:         () => {},
    __s2_event_set_float:       () => {},
    __s2_event_set_bool:        () => {},
    __s2_event_set_string:      () => {},
    __s2_event_set_uint64:      () => {},
    __s2_event_get_int:         () => 0,
    __s2_event_get_float:       () => 0,
    __s2_event_get_bool:        () => false,
    __s2_event_get_string:      () => "",
    __s2_event_get_uint64:      () => "0",
    __s2_event_get_player_slot: () => -1,
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(`
    globalThis.HookResult = { Continue:0, Changed:1, Handled:2, Stop:3 };
    function GameEvent(name) { this.name = name; }
    GameEvent.prototype.getInt        = function (k) { return __s2_event_get_int(k); };
    GameEvent.prototype.getFloat      = function (k) { return __s2_event_get_float(k); };
    GameEvent.prototype.getBool       = function (k) { return __s2_event_get_bool(k); };
    GameEvent.prototype.getString     = function (k) { return __s2_event_get_string(k); };
    GameEvent.prototype.getUint64     = function (k) { return __s2_event_get_uint64(k); };
    GameEvent.prototype.getPlayerSlot = function (k) { return __s2_event_get_player_slot(k); };
    GameEvent.prototype.setInt    = function (k, v) { __s2_event_set_int(k, v | 0); };
    GameEvent.prototype.setFloat  = function (k, v) { __s2_event_set_float(k, v); };
    GameEvent.prototype.setBool   = function (k, v) { __s2_event_set_bool(k, !!v); };
    GameEvent.prototype.setString = function (k, v) { __s2_event_set_string(k, String(v)); };
    GameEvent.prototype.setUint64 = function (k, v) { __s2_event_set_uint64(k, String(v)); };
    var Events = {
      on:    function (name, handler) { __s2_event_subscribe(name, handler); },
      off:   function (name, handler) { __s2_event_unsubscribe(name, handler); },
      onPre: function (name, handler) { __s2_event_subscribe_pre(name, handler); },
      fire:  function (name, fields, dontBroadcast) {
        if (!__s2_event_create(name)) return false;
        return __s2_event_fire(!!dontBroadcast);
      },
    };
    globalThis.__s2pkg_events = { GameEvent: GameEvent, Events: Events, HookResult: globalThis.HookResult };
  `, ctx);
  assert.equal(typeof ctx.__s2pkg_events.Events.onPre, "function", "Events.onPre is a function");
  assert.equal(ctx.__s2pkg_events.HookResult.Handled, 2, "HookResult.Handled === 2");
  assert.equal(ctx.__s2pkg_events.Events.fire("player_death"), false, "Events.fire degrades to false when no engine-op");
});

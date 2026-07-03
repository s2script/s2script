import { test } from "node:test";
import assert from "node:assert";
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

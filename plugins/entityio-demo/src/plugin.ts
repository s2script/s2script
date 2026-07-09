// Live-gate demo for entity I/O (fire inputs + hook outputs) — a self-contained, bot-provable
// both-directions test: acceptInput("Trigger") on a spawned logic_relay fires AddEntityIOEvent,
// which routes through the game's own I/O pump to FireOutputInternal (our detour) -> OnTrigger ->
// our Entity.onOutput subscriber, with a live caller EntityRef. No human client needed.
import { Commands } from "@s2script/commands";
import { createEntity, Entity } from "@s2script/entity";

Entity.onOutput("logic_relay", "OnTrigger", (ev) => {
  console.log("[entityio] output caught: " + ev.output + " caller=" + (ev.caller ? "relay" : "null"));
});

Commands.register("sm_iotest", (ctx) => {
  const relay = createEntity("logic_relay");
  if (!relay) { ctx.reply("[entityio] createEntity failed"); return; }
  relay.spawn();
  const ok = relay.acceptInput("Trigger");     // -> AddEntityIOEvent -> OnTrigger -> our onOutput
  ctx.reply("[entityio] fired Trigger ok=" + ok + " (watch the log for the output catch next tick)");
});

Commands.register("sm_iokill", (ctx) => {
  const e = createEntity("logic_relay");
  if (!e) { ctx.reply("failed"); return; }
  e.spawn();
  const before = e.isValid();
  e.acceptInput("Kill");
  ctx.reply("[entityio] Kill fired; valid before=" + before + " (gone next tick)");
});

export function onLoad(): void {
  console.log("[entityio-demo] onLoad — sm_iotest/sm_iokill registered");
}
export function onUnload(): void {}

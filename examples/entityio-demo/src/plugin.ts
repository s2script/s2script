// Live-gate demo for entity I/O (fire inputs + hook outputs) — a self-contained, bot-provable
// both-directions test: acceptInput("Trigger") on a spawned logic_relay fires AddEntityIOEvent,
// which routes through the game's own I/O pump to FireOutputInternal (our detour) -> OnTrigger ->
// our Entity.onOutput subscriber, with a live caller EntityRef. No human client needed.
import { plugin } from "@s2script/sdk/plugin";
import { createEntity } from "@s2script/sdk/entity";

export default plugin((ctx) => {
  ctx.entities.onOutput("logic_relay", "OnTrigger", (ev) => {
    const callerValid = !!(ev.caller && ev.caller.isValid());
    const activatorValid = !!(ev.activator && ev.activator.isValid());
    console.log("[entityio] output caught: " + ev.output +
      " caller=" + (ev.caller ? ("EntityRef(valid=" + callerValid + ")") : "null") +
      " activator=" + (ev.activator ? ("EntityRef(valid=" + activatorValid + ")") : "null"));
  });

  ctx.commands.register("sm_iotest", (cmd) => {
    const relay = createEntity("logic_relay");
    if (!relay) { cmd.reply("[entityio] createEntity failed"); return; }
    relay.spawn();
    // Pass the relay itself as both activator and caller so the output hook actually receives a
    // live, non-null CEntityInstance* to pack -> the mux decodes it into a live EntityRef. (Firing
    // with no activator/caller — as a bare acceptInput("Trigger") does — genuinely produces null
    // pActivator/pCaller in FireOutputInternal; that's not a marshalling gap, just an unexercised
    // path, so exercise it here.)
    const ok = relay.acceptInput("Trigger", "", relay, relay);  // -> AddEntityIOEvent -> OnTrigger -> our onOutput
    cmd.reply("[entityio] fired Trigger ok=" + ok + " (watch the log for the output catch next tick)");
  });

  ctx.commands.register("sm_iokill", (cmd) => {
    const e = createEntity("logic_relay");
    if (!e) { cmd.reply("failed"); return; }
    e.spawn();
    const before = e.isValid();
    e.acceptInput("Kill");
    cmd.reply("[entityio] Kill fired; valid before=" + before + " (gone next tick)");
  });

  console.log("[entityio-demo] onLoad — sm_iotest/sm_iokill registered");
});

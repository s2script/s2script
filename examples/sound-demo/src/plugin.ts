import { Commands } from "@s2script/sdk/commands";
import { Sound } from "@s2script/sdk/sound";
import { Pawn, Sounds } from "@s2script/cs2";

export function onLoad(): void {
  // Precache: fires at map load / mapchange. add() -> true proves the live manifest AddResource
  // path end-to-end (the file itself need not exist for the gate — the engine tolerates a
  // missing resource; a REAL custom .vsndevts playing is the deferred human-client test).
  Sound.onPrecache((ctx) => {
    const ok = ctx.add("soundevents/soundevents_s2script_demo.vsndevts");
    console.log(`[sound-demo] onPrecache fired — add() -> ${ok}`);
  });

  // sm_playsound [name] [slot]: with a slot — emit from that slot's pawn to that slot only
  // (exercises the serial-gated source + explicit recipients; a bot slot is skipped shim-side but
  // the engine is still CALLED with an empty filter -> a real EmitSound to nobody, the shim logs
  // "EmitSound ... recipients=0 -> guid=G"). Without — a worldspawn global broadcast to all valid
  // clients (on a bots server the default enumeration is the bot slots -> also all bot-skipped ->
  // still a real engine call).
  Commands.register("sm_playsound", (ctx) => {
    const name = ctx.args[0] || Sounds.Ping;
    if (ctx.args.length > 1) {
      const slot = parseInt(ctx.args[1], 10);
      const pawn = Pawn.forSlot(Number.isNaN(slot) ? -1 : slot);
      if (!pawn) {
        ctx.reply(`[sound-demo] no pawn at slot ${ctx.args[1]}`);
        return;
      }
      const guid = pawn.emitSound(name, { recipients: [slot] });
      ctx.reply(`[sound-demo] emitSound('${name}') from slot ${slot} -> guid=${guid}`);
    } else {
      const guid = Sound.emit(name);
      ctx.reply(`[sound-demo] Sound.emit('${name}') broadcast -> guid=${guid}`);
    }
  });

  console.log("[sound-demo] onLoad — sm_playsound registered");
}

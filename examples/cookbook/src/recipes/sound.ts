import { Sound, command, HookResult } from "@s2script/sdk";
import type { PrecacheContext } from "@s2script/sdk";
import { Pawn, Sounds } from "@s2script/cs2";

/**
 * Sounds must be registered during the precache window — emitting an
 * unprecached sound is silently dropped. Sound.emit broadcasts from the world;
 * pawn.emitSound plays from an entity and can target specific recipients.
 */
export const name = "sound";
export const describe = "precache and emit a sound (sm_sound [name] [slot])";

export function OnPrecache(pc: PrecacheContext): void {
  const ok = pc.add("soundevents/soundevents_s2script_demo.vsndevts");
  console.log(`[cookbook] precache add() -> ${ok}`);
}

export function OnPluginStart(): void {
  command("sm_sound", (cmd) => {
    const name = cmd.args[0] || Sounds.Ping;
    // With a slot: emit from that slot's pawn, to that slot only.
    if (cmd.args.length > 1) {
      const slot = parseInt(cmd.args[1], 10);
      const pawn = Pawn.forSlot(Number.isNaN(slot) ? -1 : slot);
      if (!pawn) { cmd.reply(`no pawn at slot ${cmd.args[1]}`); return HookResult.Handled; }
      cmd.reply(`emitSound('${name}') from slot ${slot} -> guid=${pawn.emitSound(name, { recipients: [slot] })}`);
      return HookResult.Handled;
    }
    // Without: a global broadcast from the world.
    cmd.reply(`Sound.emit('${name}') broadcast -> guid=${Sound.emit(name)}`);
    return HookResult.Handled;
  });
}

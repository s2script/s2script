import { ADMFLAG, HookResult, command } from "@s2script/sdk";
import type { CommandHandler, CommandInvocation, PhraseKey } from "@s2script/sdk";
import { Player } from "@s2script/cs2";
import { giveItem, integer, livePawn, replyFor, resolveItem, stillOwnsPawn } from "./actions";
import { registerTeleportCommands } from "./teleport";

function aliases(names: string[], flags: number, handler: CommandHandler): void {
  for (const name of names) command.admin(`sm_${name}`, flags, handler);
}

function targets(cmd: CommandInvocation, usage: PhraseKey): Player[] {
  if (!cmd.arg(0)) { cmd.replyT(usage); return []; }
  const selected = Player.target(cmd.arg(0), cmd.callerSlot, true);
  if (selected.length === 0) cmd.replyT("No matching players");
  return selected;
}

function parseTeam(input: string): number | null {
  const value = input.toLowerCase();
  if (["s", "spec", "spectator", "spectators", "1"].includes(value)) return 1;
  if (["t", "te", "terrorist", "terrorists", "2"].includes(value)) return 2;
  if (["c", "ct", "counterterrorist", "counter-terrorist", "3"].includes(value)) return 3;
  return null;
}

export function registerAdminCommands(): void {
  for (const [names, field, min, max, usage, success] of [
    [["health", "hp"], "health", 1, 1_000_000, "Usage Health", "Health Set"],
    [["armor"], "armorValue", 0, 250, "Usage Armor", "Armor Set"],
  ] as const) {
    aliases([...names], ADMFLAG.SLAY, cmd => {
      const amount = integer(cmd.arg(1), min, max);
      if (!cmd.arg(0) || amount === null) { cmd.replyT(usage); return HookResult.Handled; }
      const selected = targets(cmd, usage);
      if (!selected.length) return HookResult.Handled;
      const reply = replyFor(cmd);
      let count = 0;
      for (const player of selected) {
        const pawn = livePawn(player);
        if (!pawn || pawn[field] === null) continue;
        pawn[field] = amount;
        if (stillOwnsPawn(player, pawn) && pawn[field] === amount) count++;
      }
      reply(success, count, amount);
      return HookResult.Handled;
    });
  }

  aliases(["disarm", "strip"], ADMFLAG.SLAY, cmd => {
    const selected = targets(cmd, "Usage Disarm");
    if (!selected.length) return HookResult.Handled;
    const reply = replyFor(cmd);
    let count = 0;
    for (const player of selected) {
      const pawn = livePawn(player);
      if (pawn && pawn.disarm() && stillOwnsPawn(player, pawn)) count++;
    }
    reply("Disarmed Players", count);
    return HookResult.Handled;
  });

  for (const equip of [false, true]) aliases([equip ? "equip" : "give"], ADMFLAG.CHEATS, cmd => {
    const usage = equip ? "Usage Equip" : "Usage Give";
    if (!cmd.arg(0) || !cmd.arg(1)) { cmd.replyT(usage); return HookResult.Handled; }
    const item = resolveItem(cmd.arg(1));
    if (!item) { cmd.replyT("Invalid Item"); return HookResult.Handled; }
    const selected = targets(cmd, usage);
    if (!selected.length) return HookResult.Handled;
    const reply = replyFor(cmd);
    let count = 0;
    for (const player of selected) if (giveItem(player, item, equip)) count++;
    reply(equip ? "Equipped Players" : "Given Item", count, item);
    return HookResult.Handled;
  });

  aliases(["respawn"], ADMFLAG.SLAY, cmd => {
    const selected = targets(cmd, "Usage Respawn");
    if (!selected.length) return HookResult.Handled;
    const reply = replyFor(cmd);
    let count = 0;
    for (const player of selected) if (player.ref.isValid() && player.respawn()) count++;
    reply("Respawned Players", count);
    return HookResult.Handled;
  });

  for (const mode of ["team", "spec", "swap"] as const) aliases([mode], ADMFLAG.KICK, cmd => {
    const usage = mode === "team" ? "Usage Team" : mode === "spec" ? "Usage Spec" : "Usage Swap";
    const requestedTeam = mode === "team" ? parseTeam(cmd.arg(1)) : 1;
    if (!cmd.arg(0) || requestedTeam === null) { cmd.replyT(usage); return HookResult.Handled; }
    const selected = targets(cmd, usage);
    if (!selected.length) return HookResult.Handled;
    const reply = replyFor(cmd);
    let count = 0;
    for (const player of selected) {
      if (!player.ref.isValid()) continue;
      if (mode === "swap" && player.teamNum !== 2 && player.teamNum !== 3) continue;
      const team = mode === "swap" ? (player.teamNum === 2 ? 3 : 2) : requestedTeam;
      player.switchTeam(team);
      // SwitchTeam can synchronously respawn/fire other plugins. Only inspect the controller.
      if (player.ref.isValid() && player.teamNum === team) count++;
    }
    reply("Changed Team", count);
    return HookResult.Handled;
  });

  registerTeleportCommands();
}

import { Hud, command, HookResult } from "@s2script/sdk";
import { HideHud, Player } from "@s2script/cs2";

/**
 * Hide parts of a player's native HUD.
 *
 * `Hud` owns the pawn's `m_iHideHUD` field: each plugin holds its own bits, the pawn shows the OR of
 * all of them plus the game's own, and the mask follows the player across respawns and spectating.
 * Unloading this recipe gives back exactly the bits it hid.
 *
 *   sm_hidehud <target> <radar|health|crosshair|money|timer|chat|bits>   hide one part (or raw bits)
 *   sm_showhud <target> [part|bits]                                      show it again (no arg = all of ours)
 *   sm_hudmask <target>                                                  this plugin's bits vs everyone's
 */
export const name = "hud";
export const describe = "hide native HUD parts per player (sm_hidehud / sm_showhud / sm_hudmask)";

const PARTS: Record<string, number> = {
  radar: HideHud.Radar, health: HideHud.Health, crosshair: HideHud.Crosshair,
  money: HideHud.Money, timer: HideHud.RoundTimer, chat: HideHud.Chat,
};

function parse(arg: string): number | null {
  const named = PARTS[arg.toLowerCase()];
  if (named !== undefined) return named;
  const n = Number(arg);
  return Number.isInteger(n) && n > 0 && n <= 0xFFFFFFFF ? n : null;
}

export function OnPluginStart(): void {
  command("sm_hidehud", (cmd) => {
    const bits = parse(cmd.arg(1));
    const targets = Player.target(cmd.arg(0), cmd.callerSlot);
    if (bits === null || targets.length === 0) {
      cmd.reply(`[cookbook] usage: sm_hidehud <target> <${Object.keys(PARTS).join("|")}|bits>`);
      return HookResult.Handled;
    }
    for (const p of targets) Hud.hide(p.slot, bits);
    cmd.reply(`[cookbook] hud: hid 0x${bits.toString(16)} for ${targets.length} player(s)`);
    return HookResult.Handled;
  });

  command("sm_showhud", (cmd) => {
    const targets = Player.target(cmd.arg(0), cmd.callerSlot);
    const bits = cmd.argCount > 1 ? parse(cmd.arg(1)) : null;
    // A part that does not parse is a typo, not "everything": refuse instead of resetting.
    if (targets.length === 0 || (cmd.argCount > 1 && bits === null)) {
      cmd.reply(`[cookbook] usage: sm_showhud <target> [${Object.keys(PARTS).join("|")}|bits]`);
      return HookResult.Handled;
    }
    for (const p of targets) {
      if (bits === null) Hud.reset(p.slot); else Hud.show(p.slot, bits);
    }
    cmd.reply(`[cookbook] hud: restored ${bits === null ? "everything this plugin hid" : `0x${bits.toString(16)}`} for ${targets.length} player(s)`);
    return HookResult.Handled;
  });

  command("sm_hudmask", (cmd) => {
    for (const p of Player.target(cmd.arg(0), cmd.callerSlot)) {
      cmd.reply(`[cookbook] hud: slot ${p.slot}: ours=0x${Hud.hidden(p.slot).toString(16)} all plugins=0x${Hud.hiddenByAll(p.slot).toString(16)}`);
    }
    return HookResult.Handled;
  });
}

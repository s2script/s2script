import { Menu, MenuStyle, command, HookResult } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

function showMenu(slot: number, style: MenuStyle): void {
  const m = new Menu("s2script Menu Demo");
  m.style = style;
  m.freezePlayer = style === MenuStyle.Center;   // freeze on the center HUD demo only
  m.addItem("hp", "Heal to 100");
  m.addItem("noclip", "Toggle Noclip");
  m.addItem("disabled", "Coming soon", { disabled: true });
  for (let i = 1; i <= 8; i++) m.addItem("x" + i, "Extra option " + i);   // force pagination
  m.onSelect(e => {
    console.log(`[cookbook] menu select slot=${e.slot} item=${e.item} info=${e.info}`);
    const p = Player.fromSlot(e.slot);
    const pawn = p && p.pawn;
    if (!pawn) return;
    if (e.info === "hp") pawn.health = 100;                                     // heal to full
    else if (e.info === "noclip") pawn.moveType = pawn.moveType === 7 ? 2 : 7;  // MoveType_t: NOCLIP=7 <-> WALK=2
  });
  m.onCancel(e => { console.log(`[cookbook] menu cancel slot=${e.slot} reason=${e.reason}`); });
  m.display(slot, 30);
}

/**
 * Menu paginates automatically. On CS2 both styles paint the same hudkit
 * sheet — click a row. This also probes pawn movementServices on a bot
 * every couple of seconds — opt-in via `sm_menudemo verbose` (off by default), same
 * reasoning as recipes/damage.ts defaulting its effect off: a subscription
 * this cookbook registers unconditionally at load must not spam the console
 * on its own.
 */
let verbose = false;
let frames = 0;

export const name = "menu";
export const describe = "show a center or chat menu (sm_menudemo / sm_menudemo_chat / sm_menudemo verbose)";

export function OnGameFrame(): void {
  if (!verbose) return;
  if (++frames % 128 !== 0) return;
  const p = Player.fromSlot(0); if (!p) return;
  const pawn = p.pawn; if (!pawn) return;
  console.log(`[cookbook] menu frame=${frames} bot0 movementServices=${pawn.movementServices ? "live" : "null"}`);
}

export function OnPluginStart(): void {
  command("sm_menudemo", cmd => {
    if (cmd.arg(0) === "verbose") {
      verbose = !verbose;
      cmd.reply(`menu verbose frame-probe logging = ${verbose ? "on" : "off"}`);
      return HookResult.Handled;
    }
    if (cmd.callerSlot < 0) { cmd.reply("run in-game"); return HookResult.Handled; }
    showMenu(cmd.callerSlot, MenuStyle.Center);
    cmd.reply("center menu shown — click a row");
    return HookResult.Handled;
  });
  command("sm_menudemo_chat", cmd => {
    if (cmd.callerSlot < 0) { cmd.reply("run in-game"); return HookResult.Handled; }
    showMenu(cmd.callerSlot, MenuStyle.Chat);
    cmd.reply("chat-style menu shown — on CS2 this is the same HUD sheet");
    return HookResult.Handled;
  });
}

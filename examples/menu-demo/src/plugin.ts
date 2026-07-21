import { plugin } from "@s2script/sdk/plugin";
import { Menu, MenuStyle } from "@s2script/sdk/menu";
import { Player } from "@s2script/cs2";

function showMenu(slot: number, style: MenuStyle): void {
  const m = new Menu("s2script Menu Demo");
  m.style = style;
  m.freezePlayer = style === MenuStyle.Center;   // freeze movement while the WASD menu is open (nav still works)
  m.addItem("hp", "Heal to 100");
  m.addItem("noclip", "Toggle Noclip");
  m.addItem("disabled", "Coming soon", { disabled: true });
  for (let i = 1; i <= 8; i++) m.addItem("x" + i, "Extra option " + i);   // force pagination
  m.onSelect(e => {
    console.log(`[menu-demo] select slot=${e.slot} item=${e.item} info=${e.info}`);
    const p = Player.fromSlot(e.slot);
    const pawn = p && p.pawn;
    if (!pawn) return;
    if (e.info === "hp") pawn.health = 100;                                     // heal to full
    else if (e.info === "noclip") pawn.moveType = pawn.moveType === 7 ? 2 : 7;  // MoveType_t: NOCLIP=7 <-> WALK=2
  });
  m.onCancel(e => { console.log(`[menu-demo] cancel slot=${e.slot} reason=${e.reason}`); });
  m.display(slot, 30);
}

export default plugin((ctx) => {
  ctx.commands.register("sm_menu", cmd => {
    if (cmd.callerSlot < 0) { cmd.reply("run in-game"); return; }
    showMenu(cmd.callerSlot, MenuStyle.Center);
    cmd.reply("center menu shown — W/S to move, E to select");
  });
  ctx.commands.register("sm_chatmenu", cmd => {
    if (cmd.callerSlot < 0) { cmd.reply("run in-game"); return; }
    showMenu(cmd.callerSlot, MenuStyle.Chat);
    cmd.reply("chat menu shown — type the number");
  });

  // Prove the WASD input primitive live: log a bot's button mask changing (bots press buttons).
  let frames = 0;
  ctx.server.onGameFrame(() => {
    if (++frames % 128 !== 0) return;               // ~ every 2s
    const p = Player.fromSlot(0); if (!p) return;
    const pawn = p.pawn; if (!pawn) return;
    // read the same button mask the center renderer uses (offsets resolved in pawn.js are internal;
    // here we just confirm the pawn/movementServices is live by logging a nav field)
    console.log(`[menu-demo] frame=${frames} bot0 movementServices=${pawn.movementServices ? "live" : "null"}`);
  });
});

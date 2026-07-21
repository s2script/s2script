// Live-gate demo for the item surface (giveNamedItem/weapons/stripWeapons/dropActiveWeapon) on
// CS2 2000870. Player.target()/Player.allConnected() return 0 players on this build — the
// client-list offsets (NetworkServerService/NetworkGameServer/ServerSideClient) are stale (the
// same documented regression trace-demo routes around; see examples/trace-demo/src/plugin.ts).
// So these commands do NOT do SM-style name/target resolution here — they act on every live
// pawn found via Pawn.forSlot (schema-based, self-healing), which is exactly what "@all" would
// have resolved to anyway. Revert to Player.target once the client-list offsets are regenerated
// for the current CS2 patch.
import { plugin } from "@s2script/sdk/plugin";
import { CsItem, Pawn } from "@s2script/cs2";

const MAX_SLOTS = 12;

function livePawns(): Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> {
  const out: Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> = [];
  for (let slot = 0; slot < MAX_SLOTS; slot++) {
    const pawn = Pawn.forSlot(slot);
    if (pawn) out.push({ slot, pawn });
  }
  return out;
}

export default plugin((ctx) => {
  ctx.commands.register("sm_give", (cmd) => {
    const weapon = cmd.args[0] || CsItem.AK47;
    let n = 0;
    for (const { pawn } of livePawns()) {
      const w = pawn.giveNamedItem(weapon);
      if (w && w.isValid()) n++;
    }
    cmd.reply("[items] gave " + weapon + " to " + n + " player(s)");
  });
  ctx.commands.register("sm_weapons", (cmd) => {
    for (const { slot, pawn } of livePawns()) {
      cmd.reply("[items] " + slot + " has " + pawn.weapons.length + " weapon(s)");
    }
  });
  ctx.commands.register("sm_strip", (cmd) => {
    let n = 0;
    for (const { pawn } of livePawns()) { if (pawn.stripWeapons()) n++; }
    cmd.reply("[items] stripped " + n + " player(s)");
  });
  ctx.commands.register("sm_drop", (cmd) => {
    let n = 0;
    for (const { pawn } of livePawns()) { if (pawn.dropActiveWeapon()) n++; }
    cmd.reply("[items] dropped for " + n + " player(s)");
  });
});

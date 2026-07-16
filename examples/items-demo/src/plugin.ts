// Live-gate demo for the item surface (giveNamedItem/weapons/stripWeapons/dropActiveWeapon) on
// CS2 2000870. Player.target()/Player.allConnected() return 0 players on this build — the
// client-list offsets (NetworkServerService/NetworkGameServer/ServerSideClient) are stale (the
// same documented regression trace-demo routes around; see examples/trace-demo/src/plugin.ts).
// So these commands do NOT do SM-style name/target resolution here — they act on every live
// pawn found via Pawn.forSlot (schema-based, self-healing), which is exactly what "@all" would
// have resolved to anyway. Revert to Player.target once the client-list offsets are regenerated
// for the current CS2 patch.
import { Commands } from "@s2script/sdk/commands";
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

Commands.register("sm_give", (ctx) => {
  const weapon = ctx.args[0] || CsItem.AK47;
  let n = 0;
  for (const { pawn } of livePawns()) {
    const w = pawn.giveNamedItem(weapon);
    if (w && w.isValid()) n++;
  }
  ctx.reply("[items] gave " + weapon + " to " + n + " player(s)");
});
Commands.register("sm_weapons", (ctx) => {
  for (const { slot, pawn } of livePawns()) {
    ctx.reply("[items] " + slot + " has " + pawn.weapons.length + " weapon(s)");
  }
});
Commands.register("sm_strip", (ctx) => {
  let n = 0;
  for (const { pawn } of livePawns()) { if (pawn.stripWeapons()) n++; }
  ctx.reply("[items] stripped " + n + " player(s)");
});
Commands.register("sm_drop", (ctx) => {
  let n = 0;
  for (const { pawn } of livePawns()) { if (pawn.dropActiveWeapon()) n++; }
  ctx.reply("[items] dropped for " + n + " player(s)");
});

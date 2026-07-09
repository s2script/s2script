import { Commands } from "@s2script/commands";
import { Player, CsItem } from "@s2script/cs2";

Commands.register("sm_give", (ctx) => {
  const t = Player.target(ctx.args[0] || "", ctx.callerSlot);
  const weapon = ctx.args[1] || CsItem.AK47;
  let n = 0;
  for (const p of t) { const pawn = p.pawn; if (!pawn) continue; const w = pawn.giveNamedItem(weapon); if (w && w.isValid()) n++; }
  ctx.reply("[items] gave " + weapon + " to " + n + " player(s)");
});
Commands.register("sm_weapons", (ctx) => {
  const t = Player.target(ctx.args[0] || "", ctx.callerSlot);
  for (const p of t) { const pawn = p.pawn; if (!pawn) continue; ctx.reply("[items] " + p.slot + " has " + pawn.weapons.length + " weapon(s)"); }
});
Commands.register("sm_strip", (ctx) => {
  const t = Player.target(ctx.args[0] || "", ctx.callerSlot);
  let n = 0; for (const p of t) { const pawn = p.pawn; if (pawn && pawn.stripWeapons()) n++; }
  ctx.reply("[items] stripped " + n + " player(s)");
});
Commands.register("sm_drop", (ctx) => {
  const t = Player.target(ctx.args[0] || "", ctx.callerSlot);
  let n = 0; for (const p of t) { const pawn = p.pawn; if (pawn && pawn.dropActiveWeapon()) n++; }
  ctx.reply("[items] dropped for " + n + " player(s)");
});

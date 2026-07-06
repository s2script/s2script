import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Player } from "@s2script/cs2";

// Slice 6.3 — sm_slap <target> [damage] (ADMFLAG.SLAY). Reliable damage (a direct health write, clamped >= 1)
// plus a best-effort velocity knockback (may be reset by physics next tick; the slice doesn't depend on it).
export function onLoad(): void {
  Commands.registerAdmin("sm_slap", ADMFLAG.SLAY, (ctx) => {
    const targetStr = ctx.arg(0);
    if (!targetStr) { ctx.reply("Usage: sm_slap <target> [damage]"); return; }
    const damage = Math.max(0, ctx.argInt(1, 0));
    const targets = Player.target(targetStr, ctx.callerSlot);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    let n = 0;
    for (const p of targets) {
      const pawn = p.pawn;
      if (!pawn) continue;
      const hpBefore = pawn.health;
      if (hpBefore !== null && damage > 0) pawn.health = Math.max(1, hpBefore - damage);
      const v = pawn.absVelocity;                                   // best-effort knockback
      const nudge = (n % 2 === 0) ? 200 : -200;
      if (v) pawn.setVelocity(v.x + nudge, v.y + nudge, v.z + 300);
      console.log("[playercommands] sm_slap slot=" + p.slot + " hpBefore=" + hpBefore + " hpAfter=" + pawn.health);
      n++;
    }
    ctx.reply("[SM] Slapped " + n + " player" + (n === 1 ? "" : "s") + " for " + damage + " damage.");
  });

  console.log("[playercommands] onLoad — slap registered");
}

export function onUnload(): void { console.log("[playercommands] onUnload"); }

import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Player, Events, pickPlayer } from "@s2script/cs2";
import { TopMenu } from "@s2script/topmenu";

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

  // Slice 6.14 — sm_slay <target> (ADMFLAG.SLAY). Kills each matched player's pawn via CommitSuicide.
  // A null pawn (dead/absent) is skipped; the native is serial-gated and no-ops on a stale ref.
  Commands.registerAdmin("sm_slay", ADMFLAG.SLAY, (ctx) => {
    const targetStr = ctx.arg(0);
    if (!targetStr) { ctx.reply("Usage: sm_slay <target>"); return; }
    const targets = Player.target(targetStr, ctx.callerSlot);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    let n = 0;
    for (const p of targets) {
      const pawn = p.pawn;
      if (!pawn) continue;
      pawn.slay();
      console.log("[playercommands] sm_slay slot=" + p.slot);
      n++;
    }
    ctx.reply("[SM] Slayed " + n + " player" + (n === 1 ? "" : "s") + ".");
  });

  // Slice 6.14 — sm_rename <target> <newname> (ADMFLAG.SLAY). Single-target only (reject ambiguous multi).
  // Strips control chars (< 0x20), bounds to 127 bytes, writes m_iszPlayerName, then fires player_changename.
  Commands.registerAdmin("sm_rename", ADMFLAG.SLAY, (ctx) => {
    const targetStr = ctx.arg(0);
    const rawName = ctx.argsFrom(1).trim();
    if (!targetStr || !rawName) { ctx.reply("Usage: sm_rename <target> <newname>"); return; }
    const targets = Player.target(targetStr, ctx.callerSlot);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    if (targets.length > 1) {
      ctx.reply("[SM] Ambiguous target — matched " + targets.length + " players. Use #userid or full name.");
      return;
    }
    const p = targets[0];
    // Strip control chars (< 0x20) and bound to 127 bytes.
    const newname = rawName.replace(/[\x00-\x1F]/g, "").slice(0, 127);
    if (!newname) { ctx.reply("[SM] Invalid name (empty after sanitization)."); return; }
    const oldname = p.playerName ?? "";
    if (!p.setName(newname)) { ctx.reply("[SM] Rename failed (player became unavailable)."); return; }
    // Best-effort: fire player_changename so other plugins and clients learn of the rename (only after a real write).
    Events.fire("player_changename", { userid: p.userId, oldname, newname });
    console.log("[playercommands] sm_rename slot=" + p.slot + " '" + oldname + "' -> '" + newname + "'");
    ctx.reply("[SM] Renamed " + oldname + " to " + newname + ".");
  });

  // adminmenu — Slap/Slay proof items, same ADMFLAG as their text commands, via pickPlayer.
  TopMenu.addItem("Player Commands", { id: "playercommands:slap", name: "Slap", flags: ADMFLAG.SLAY,
    onSelect: adminSlot => pickPlayer(adminSlot, t => { const p = t.pawn; if (p) p.health = Math.max(1, (p.health ?? 1) - 5); }) });
  TopMenu.addItem("Player Commands", { id: "playercommands:slay", name: "Slay", flags: ADMFLAG.SLAY,
    onSelect: adminSlot => pickPlayer(adminSlot, t => { const p = t.pawn; if (p) p.slay(); }) });

  console.log("[playercommands] onLoad — slap/slay/rename registered");
}

export function onUnload(): void { console.log("[playercommands] onUnload"); }

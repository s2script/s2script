import { Commands } from "@s2script/commands";
import { Chat } from "@s2script/chat";
import { Admin, ADMFLAG } from "@s2script/admin";
import { Player } from "@s2script/cs2";

// Slice 6.2 live gate — admin-gated commands. sm_say is now registered via Commands.registerAdmin with
// ADMFLAG.CHAT: the server console / rcon is root (always allowed); an in-game player needs the CHAT flag
// (from admins.json or a runtime Admin.add), else it replies "You do not have access." Chat.toAll delivers
// the SayText2 chat message (6.1c). Admin cache = host-global (file admins.json ⊕ runtime), from @s2script/admin.
export function onLoad(): void {
  Commands.registerAdmin("sm_say", ADMFLAG.CHAT, (ctx) => {
    const msg = ctx.argString.trim();
    if (!msg) { ctx.reply("Usage: sm_say <message>"); return; }
    Chat.toAll("[SM] " + msg);
    console.log("[basecommands] sm_say by slot=" + ctx.callerSlot + " msg=" + msg);
  });

  // 6.3 — sm_kick <target> [reason] (ADMFLAG.KICK). Resolves the SM target string (#userid/name/@all/@me)
  // and disconnects each match via the engine KickClient. Server console / rcon is root.
  Commands.registerAdmin("sm_kick", ADMFLAG.KICK, (ctx) => {
    const parts = ctx.argString.trim().split(/\s+/).filter(Boolean);
    const targetStr = parts[0];
    if (!targetStr) { ctx.reply("Usage: sm_kick <target> [reason]"); return; }
    const reason = parts.slice(1).join(" ") || "Kicked by admin";
    const targets = Player.target(targetStr, ctx.callerSlot);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    // Destructive-command safety (SM COMMAND_FILTER_NO_MULTI): an ambiguous NAME matching >1 player kicks
    // nobody — @all / #userid stay the explicit multi/precise selectors; an exact name still resolves to 1.
    if (targets.length > 1 && targetStr[0] !== "@" && targetStr[0] !== "#") {
      ctx.reply("[SM] Multiple players match '" + targetStr + "' — be more specific (or use @all)."); return;
    }
    let n = 0;
    for (const p of targets) {
      console.log("[basecommands] sm_kick slot=" + p.slot + " name=" + p.playerName + " reason=" + reason);
      p.kick(reason);
      n++;
    }
    ctx.reply("[SM] Kicked " + n + " player" + (n === 1 ? "" : "s") + ".");
  });

  // 6.3 — sm_slap <target> [damage] (ADMFLAG.SLAY). Reliable damage (a direct health write, clamped >= 1)
  // plus a best-effort velocity knockback (may be reset by physics next tick; the slice doesn't depend on it).
  Commands.registerAdmin("sm_slap", ADMFLAG.SLAY, (ctx) => {
    const parts = ctx.argString.trim().split(/\s+/).filter(Boolean);
    const targetStr = parts[0];
    if (!targetStr) { ctx.reply("Usage: sm_slap <target> [damage]"); return; }
    const damage = parts[1] ? Math.max(0, parseInt(parts[1], 10) || 0) : 0;
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
      console.log("[basecommands] sm_slap slot=" + p.slot + " hpBefore=" + hpBefore + " hpAfter=" + pawn.health);
      n++;
    }
    ctx.reply("[SM] Slapped " + n + " player" + (n === 1 ? "" : "s") + " for " + damage + " damage.");
  });

  // 6.2 live-gate diagnostic: prove the admin cache works live (rcon-verifiable, no human client needed).
  Admin.add("76561199000000009", ADMFLAG.KICK | ADMFLAG.CHAT);   // runtime tier
  const t = Admin.get("76561199000000009");
  console.log("[basecommands] admin diag: runtime-add hasKick=" + (t ? String(t.hasFlags(ADMFLAG.KICK)) : "null")
    + " hasBan=" + (t ? String(t.hasFlags(ADMFLAG.BAN)) : "null"));
  console.log("[basecommands] admin diag: slot0=" + (Admin.forSlot(0) ? "admin" : "not-admin (bot/steamid=0)"));
  console.log("[basecommands] onLoad — sm_say registered (registerAdmin ADMFLAG.CHAT)");
}

export function onUnload(): void { console.log("[basecommands] onUnload"); }

import { Commands } from "@s2script/sdk/commands";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Player, Events, pickPlayer } from "@s2script/cs2";
import { TopMenu } from "@s2script/sdk/topmenu";

// Shared player actions — ONE implementation each, driven by both the text command and the adminmenu
// item (two UIs over one action, never a re-implementation). Each returns whether it applied (a null
// pawn / dead player -> false, skipped).

// sm_slap: reliable damage (a direct health write, clamped >= 1) + a best-effort velocity knockback
// (may be reset by physics next tick; not depended on). damage 0 = knockback only.
function slapPlayer(p: Player, damage: number): boolean {
  const pawn = p.pawn;
  if (!pawn) return false;
  const hpBefore = pawn.health;
  if (hpBefore !== null && damage > 0) pawn.health = Math.max(1, hpBefore - damage);
  const v = pawn.absVelocity;
  if (v) pawn.setVelocity(v.x + 200, v.y + 200, v.z + 300);
  console.log("[playercommands] slap slot=" + p.slot + " dmg=" + damage + " hp " + hpBefore + " -> " + pawn.health);
  return true;
}

// sm_slay: kill the pawn via CommitSuicide (serial-gated native, no-ops on a stale ref).
function slayPlayer(p: Player): boolean {
  const pawn = p.pawn;
  if (!pawn) return false;
  pawn.slay();
  console.log("[playercommands] slay slot=" + p.slot);
  return true;
}

// adminmenu: run an action on a picked player, then RE-OPEN the picker so the admin stays in the menu
// (act on multiple players) until they pick Exit — SM admin-menu behavior. Event-driven, not recursive:
// each pick fires once, re-displays, and waits for the next input; Exit is an onCancel (no re-open).
function pickLoop(adminSlot: number, action: (t: Player) => void): void {
  pickPlayer(adminSlot, t => { action(t); pickLoop(adminSlot, action); });
}

export function onLoad(): void {
  // Slice 6.3 — sm_slap <target> [damage] (ADMFLAG.SLAY).
  Commands.registerAdmin("sm_slap", ADMFLAG.SLAY, (ctx) => {
    const targetStr = ctx.arg(0);
    if (!targetStr) { ctx.reply("Usage: sm_slap <target> [damage]"); return; }
    const damage = Math.max(0, ctx.argInt(1, 0));
    const targets = Player.target(targetStr, ctx.callerSlot, true);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    let n = 0;
    for (const p of targets) if (slapPlayer(p, damage)) n++;
    ctx.reply("[SM] Slapped " + n + " player" + (n === 1 ? "" : "s") + " for " + damage + " damage.");
  });

  // Slice 6.14 — sm_slay <target> (ADMFLAG.SLAY).
  Commands.registerAdmin("sm_slay", ADMFLAG.SLAY, (ctx) => {
    const targetStr = ctx.arg(0);
    if (!targetStr) { ctx.reply("Usage: sm_slay <target>"); return; }
    const targets = Player.target(targetStr, ctx.callerSlot, true);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    let n = 0;
    for (const p of targets) if (slayPlayer(p)) n++;
    ctx.reply("[SM] Slayed " + n + " player" + (n === 1 ? "" : "s") + ".");
  });

  // Slice 6.14 — sm_rename <target> <newname> (ADMFLAG.SLAY). Single-target only (reject ambiguous multi).
  Commands.registerAdmin("sm_rename", ADMFLAG.SLAY, (ctx) => {
    const targetStr = ctx.arg(0);
    const rawName = ctx.argsFrom(1).trim();
    if (!targetStr || !rawName) { ctx.reply("Usage: sm_rename <target> <newname>"); return; }
    const targets = Player.target(targetStr, ctx.callerSlot, true);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    if (targets.length > 1) {
      ctx.reply("[SM] Ambiguous target — matched " + targets.length + " players. Use #userid or full name.");
      return;
    }
    const p = targets[0];
    const newname = rawName.replace(/[\x00-\x1F]/g, "").slice(0, 127);
    if (!newname) { ctx.reply("[SM] Invalid name (empty after sanitization)."); return; }
    const oldname = p.playerName ?? "";
    if (!p.setName(newname)) { ctx.reply("[SM] Rename failed (player became unavailable)."); return; }
    Events.fire("player_changename", { userid: p.userId, oldname, newname });
    console.log("[playercommands] sm_rename slot=" + p.slot + " '" + oldname + "' -> '" + newname + "'");
    ctx.reply("[SM] Renamed " + oldname + " to " + newname + ".");
  });

  // adminmenu items — the SAME action functions the text commands use (no re-implementation). pickLoop
  // keeps the picker open (act on multiple players) until Exit.
  TopMenu.addItem("Player Commands", { id: "playercommands:slap", name: "Slap", flags: ADMFLAG.SLAY,
    onSelect: adminSlot => pickLoop(adminSlot, t => slapPlayer(t, 5)) });   // menu default: 5 damage + knockback
  TopMenu.addItem("Player Commands", { id: "playercommands:slay", name: "Slay", flags: ADMFLAG.SLAY,
    onSelect: adminSlot => pickLoop(adminSlot, t => slayPlayer(t)) });

  console.log("[playercommands] onLoad — slap/slay/rename registered");
}

export function onUnload(): void { console.log("[playercommands] onUnload"); }

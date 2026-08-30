import { command, hook, translations, ADMFLAG, Translations } from "@s2script/sdk";
import { Player, Events, pickPlayer } from "@s2script/cs2";

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

export function OnPluginStart(): void {
  translations.load("playercommands", "common");

  // Slice 6.3 — sm_slap <target> [damage] (ADMFLAG.SLAY).
  command.admin("sm_slap", ADMFLAG.SLAY, (cmd) => {
    const targetStr = cmd.arg(0);
    if (!targetStr) { cmd.replyT("Usage Slap"); return; }
    const damage = Math.max(0, cmd.argInt(1, 0));
    const targets = Player.target(targetStr, cmd.callerSlot, true);
    if (targets.length === 0) { cmd.replyT("No matching players"); return; }
    let n = 0;
    for (const p of targets) if (slapPlayer(p, damage)) n++;
    cmd.replyT(n === 1 ? "Slapped Player" : "Slapped Players", n, damage);
  });

  // Slice 6.14 — sm_slay <target> (ADMFLAG.SLAY).
  command.admin("sm_slay", ADMFLAG.SLAY, (cmd) => {
    const targetStr = cmd.arg(0);
    if (!targetStr) { cmd.replyT("Usage Slay"); return; }
    const targets = Player.target(targetStr, cmd.callerSlot, true);
    if (targets.length === 0) { cmd.replyT("No matching players"); return; }
    let n = 0;
    for (const p of targets) if (slayPlayer(p)) n++;
    cmd.replyT(n === 1 ? "Slayed Player" : "Slayed Players", n);
  });

  // Slice 6.14 — sm_rename <target> <newname> (ADMFLAG.SLAY). Single-target only (reject ambiguous multi).
  command.admin("sm_rename", ADMFLAG.SLAY, (cmd) => {
    const targetStr = cmd.arg(0);
    const rawName = cmd.argsFrom(1).trim();
    if (!targetStr || !rawName) { cmd.replyT("Usage Rename"); return; }
    const targets = Player.target(targetStr, cmd.callerSlot, true);
    if (targets.length === 0) { cmd.replyT("No matching players"); return; }
    if (targets.length > 1) {
      cmd.replyT("Rename Ambiguous Target", targets.length);
      return;
    }
    const p = targets[0];
    // Strip control chars AND braces. The brace strip isn't for injection (a name never reaches the
    // colour-expanding funnel) — it's so "Renamed" echoes the SAME string that got set: translate's
    // {1}/{2} substitution (__s2_tr_format, core/js/prelude.js) strips braces from every argument as
    // collateral, so a raw "{green}X" here would be SET as-is but ECHOED as "greenX" — a stripped-here
    // name keeps the stored and reported values identical.
    const newname = rawName.replace(/[\x00-\x1F{}]/g, "").slice(0, 127);
    if (!newname) { cmd.replyT("Invalid Rename"); return; }
    const oldname = p.playerName ?? "";
    if (!p.setName(newname)) { cmd.replyT("Rename Failed"); return; }
    Events.fire("player_changename", { userid: p.userId, oldname, newname });
    console.log("[playercommands] sm_rename slot=" + p.slot + " '" + oldname + "' -> '" + newname + "'");
    cmd.replyT("Renamed", oldname, newname);
  });

  // adminmenu items — the SAME action functions the text commands use (no re-implementation). pickLoop
  // keeps the picker open (act on multiple players) until Exit.
  // `name` is a static field set once here, before any admin has opened the menu, so — same as
  // basecommands' "Change Map Item" — it can only resolve at the server default language (-1), not
  // per-viewer.
  hook.topmenu.addItem("Player Commands", { id: "playercommands:slap", name: Translations.translate(-1, "Slap Item"), flags: ADMFLAG.SLAY,
    onSelect: adminSlot => pickLoop(adminSlot, t => slapPlayer(t, 5)) });   // menu default: 5 damage + knockback
  hook.topmenu.addItem("Player Commands", { id: "playercommands:slay", name: Translations.translate(-1, "Slay Item"), flags: ADMFLAG.SLAY,
    onSelect: adminSlot => pickLoop(adminSlot, t => slayPlayer(t)) });

  console.log("[playercommands] onLoad — slap/slay/rename registered");
}

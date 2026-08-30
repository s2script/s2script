import { hook, command, translations, Admin, ADMFLAG, Server, Plugins, Menu, MenuStyle, Translations } from "@s2script/sdk";
import type { Command, DamageInfo } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

// adminmenu — Change Map proof item (Server Commands, ADMFLAG.CHANGEMAP), a curated map picker filtered
// by Server.isMapValid so an uninstalled map never shows.
const MAP_CHOICES = ["de_dust2", "de_inferno", "de_mirage", "de_nuke", "de_ancient", "de_anubis"];

/** SM flag letters by bit index: a..n are 1<<0..1<<13, z is ROOT (1<<14). */
const FLAG_LETTERS = "abcdefghijklmn";

/** printf `%-<width>.<maxLen>s`: truncate to maxLen, then left-align to width. */
function pad(text: string, width: number, maxLen: number): string {
  return text.slice(0, maxLen).padEnd(width);
}

/**
 * SM's flag rendering ladder (basecommands/who.sp): no flags -> "none", ROOT -> "root" (ROOT
 * implies everything, so listing letters would be noise), otherwise the letter string.
 */
function flagString(admin: { flags: number } | null, slot: number): string {
  const flags = admin?.flags ?? 0;
  if (flags === 0) return Translations.translate(slot, "Flag None");
  if ((flags & ADMFLAG.ROOT) !== 0) return Translations.translate(slot, "Flag Root");

  let out = "";
  for (let i = 0; i < FLAG_LETTERS.length; i++) {
    if ((flags & (1 << i)) !== 0) out += FLAG_LETTERS[i];
  }
  return out.length > 0 ? out : Translations.translate(slot, "Flag None");
}

/** `sm_who <target>` — SM's PerformWho: one line per resolved player, in the caller's own channel. */
function whoOne(cmd: Command, pattern: string): void {
  const matches = Player.target(pattern, cmd.callerSlot);
  if (matches.length === 0) { cmd.replyT("No matching players"); return; }

  for (const p of matches) {
    const admin = Admin.forSlot(p.slot);
    const name = p.playerName ?? "";
    if (admin === null) { cmd.replyT("Not An Admin", name); continue; }

    const groups = admin.groups.length > 0 ? admin.groups.join(",") : "";
    if (groups.length > 0) cmd.replyT("Who Access With Groups", name, groups, flagString(admin, cmd.callerSlot));
    else cmd.replyT("Who Access", name, flagString(admin, cmd.callerSlot));
  }
}

/**
 * Slice 6.2 live gate — admin-gated commands. Admin cache = host-global (file admins.json ⊕ runtime),
 * from @s2script/admin. sm_say has moved to @s2script/basechat.
 */
export function OnPluginStart(): void {
  // Own set FIRST, common SECOND: within each of translate's two passes (client language, then
  // English) the first hit wins, so this order makes a plugin's own phrase beat a shared one at
  // the same tier.
  translations.load("basecommands", "common");

  command.admin("sm_kick", ADMFLAG.KICK, kick);
  command.admin("sm_map", ADMFLAG.CHANGEMAP, map);
  command.admin("sm_who", ADMFLAG.GENERIC, who);
  command.admin("sm_reloadadmins", ADMFLAG.BAN, reloadAdmins);
  command.admin("sm_rcon", ADMFLAG.RCON, rcon);
  command.admin("sm_exec", ADMFLAG.CONFIG, execCfg);
  command.admin("sm_cvar", ADMFLAG.CONVARS, cvar);
  // 6.12 — PUBLIC command (command(), not command.admin): `sm`/`version`/`credits`/`plugins list`
  // are available to everyone. Mutating `plugins load|unload|reload` is gated inline (ROOT).
  command("sm", sm);

  hook.damage(halve);

  // 6.2 live-gate diagnostic: prove the admin cache works live (rcon-verifiable, no human client needed).
  Admin.add("76561199000000009", ADMFLAG.KICK | ADMFLAG.CHAT);   // runtime tier
  const diagAdmin = Admin.get("76561199000000009");
  console.log("[basecommands] admin diag: runtime-add hasKick="
    + (diagAdmin ? String(diagAdmin.hasFlags(ADMFLAG.KICK)) : "null")
    + " hasBan=" + (diagAdmin ? String(diagAdmin.hasFlags(ADMFLAG.BAN)) : "null"));
  console.log("[basecommands] admin diag: slot0=" + (Admin.forSlot(0) ? "admin" : "not-admin (bot/steamid=0)"));

  // The category string "Server Commands" is a cross-plugin matching key (adminmenu's itemsFor
  // compares it by exact equality against every plugin's addItem category) — it stays untranslated,
  // same reasoning as adminmenu's own category constants. `name` is a static field set once here,
  // before any admin has opened the menu, so — unlike the sub-menu title below, which is built
  // fresh per onSelect and can use the calling admin's own language — it can only ever resolve at
  // the server default language (-1); still an operator-configurable string via
  // translations/basecommands.phrases.json, just not a per-viewer one.
  hook.topmenu.addItem("Server Commands", { id: "basecommands:map", name: Translations.translate(-1, "Change Map Item"), flags: ADMFLAG.CHANGEMAP,
    onSelect: adminSlot => {
      const m = new Menu(Translations.translate(adminSlot, "Change Map Title"));
      m.style = MenuStyle.Center;
      m.freezePlayer = true;   // WASD nav — keep the admin frozen through the sub-menu
      for (const mapName of MAP_CHOICES) if (Server.isMapValid(mapName)) m.addItem(mapName, mapName);
      m.onSelect(e => { Server.command("changelevel " + e.info); });
      m.display(adminSlot, 30);
    } });

  console.log("[basecommands] onLoad — kick/map/who/rcon/exec/cvar/sm registered");
}

// 6.3 — sm_kick <target> [reason] (ADMFLAG.KICK). Resolves the SM target string (#userid/name/@all/@me)
// and disconnects each match via the engine KickClient. Server console / rcon is root.
function kick(cmd: Command): void {
  const targetStr = cmd.arg(0);
  if (!targetStr) { cmd.replyT("Usage Kick"); return; }
  const customReason = cmd.argsFrom(1);
  const targets = Player.target(targetStr, cmd.callerSlot, true);
  if (targets.length === 0) { cmd.replyT("No matching players"); return; }
  // Destructive-command safety (SM COMMAND_FILTER_NO_MULTI): an ambiguous NAME matching >1 player kicks
  // nobody — @all / #userid stay the explicit multi/precise selectors; an exact name still resolves to 1.
  if (targets.length > 1 && targetStr[0] !== "@" && targetStr[0] !== "#") {
    cmd.replyT("Kick Ambiguous Target", targetStr); return;
  }
  let n = 0;
  for (const p of targets) {
    // The kick reason lands in the ENGINE disconnect UI, not the chat/console funnel, so it is
    // translated for the TARGET (p.slot), not the admin — and never carries a colour tag (nothing
    // expands it there).
    const reason = customReason || Translations.translate(p.slot, "Kick Reason Default");
    console.log("[basecommands] sm_kick slot=" + p.slot + " name=" + p.playerName + " reason=" + reason);
    p.kick(reason);
    n++;
  }
  cmd.replyT(n === 1 ? "Kicked Player" : "Kicked Players", n);
}

// 6.4 — sm_map <mapname> (ADMFLAG.CHANGEMAP). Sanitizes the name (injection guard, we build a
// "changelevel <map>" string), rejects an invalid map cleanly, then changes level via @s2script/server.
function map(cmd: Command): void {
  const mapName = cmd.arg(0);
  if (!mapName) { cmd.replyT("Usage Map"); return; }
  if (!/^[A-Za-z0-9_]+$/.test(mapName)) { cmd.replyT("Invalid Map Name"); return; }
  if (!Server.isMapValid(mapName)) { cmd.replyT("Map Not Valid", mapName); return; }
  console.log("[basecommands] sm_map -> changelevel " + mapName + " by slot=" + cmd.callerSlot);
  cmd.replyT("Changing Map", mapName);
  Server.command("changelevel " + mapName);
}

// 6.5 — sm_who [#userid|name] (ADMFLAG.GENERIC): SourceMod-parity admin listing.
//
// Mirrors basecommands/who.sp: the table goes to the CONSOLE with SM's exact column widths
// ("%2d. %-24.23s %-18.17s %s"), a chat caller gets "See console for output.", and a target
// argument prints the single-player form instead. Flag rendering follows SM's rule ladder:
// no flags -> "none", ROOT -> "root", else the letter string.
//
// ONE deviation, unavoidable: SM's middle column is the admin's *username* from admins.cfg.
// s2script has no username on AdminInfo (steamId/flags/immunity/groups), so that column carries
// the group list, which is the nearest analog and what an admin actually wants to see.
function who(cmd: Command): void {
  const target = cmd.arg(0);
  if (target) { whoOne(cmd, target); return; }

  // replyToConsole has no replyT variant — translate for the caller, then force it to console.
  const name = Translations.translate(cmd.callerSlot, "Who Header Name");
  const groups = Translations.translate(cmd.callerSlot, "Who Header Groups");
  const access = Translations.translate(cmd.callerSlot, "Who Header Access");
  cmd.replyToConsole("    " + pad(name, 24, 23) + " " + pad(groups, 18, 17) + " " + access);

  for (const p of Player.allConnected()) {
    const admin = Admin.forSlot(p.slot);
    const g = admin && admin.groups.length > 0 ? admin.groups.join(",") : "-";
    cmd.replyToConsole(
      String(p.slot + 1).padStart(2) + ". " +
      pad(p.playerName ?? "", 24, 23) + " " + pad(g, 18, 17) + " " + flagString(admin, cmd.callerSlot),
    );
  }

  // SM only nudges when the answer went somewhere the caller isn't looking.
  if (cmd.replySource === "chat") cmd.replyT("See Console For Output");
}

// sm_reloadadmins (ADMFLAG.BAN) — SM parity (basecommands.sp:75). Re-reads admins.json into the
// FILE tier, clearing the old file entries first. The runtime tier (Admin.add, e.g. permissions a
// plugin derives from an external source) is untouched, so this reloads the static admin list
// without revoking anything granted programmatically.
function reloadAdmins(cmd: Command): void {
  Admin.reload();
  console.log("[basecommands] sm_reloadadmins by slot=" + cmd.callerSlot);
  cmd.replyT("Admin Cache Reloaded");
}

// 6.5 — sm_rcon <command> (ADMFLAG.RCON): a deliberate full server-command passthrough (highest-trust flag).
function rcon(cmd: Command): void {
  const c = cmd.argString.trim();
  if (!c) { cmd.replyT("Usage Rcon"); return; }
  console.log("[basecommands] sm_rcon by slot=" + cmd.callerSlot + " cmd=" + c);
  Server.command(c);
  cmd.replyT("Rcon Command Sent");
}

// 6.5 — sm_exec <cfgfile> (ADMFLAG.CONFIG): exec a server config. Sanitize the filename (we build "exec <file>").
function execCfg(cmd: Command): void {
  const file = cmd.arg(0);
  if (!file) { cmd.replyT("Usage Exec"); return; }
  if (!/^[A-Za-z0-9_./-]+$/.test(file) || file.indexOf("..") !== -1) { cmd.replyT("Invalid Config Name"); return; }
  console.log("[basecommands] sm_exec by slot=" + cmd.callerSlot + " file=" + file);
  Server.command("exec " + file);
  cmd.replyT("Executing Config", file);
}

// 6.7 — sm_cvar <name> [value] (ADMFLAG.CVARS). No value → GET (reply the value); with a value → SET
// (via the console) then read back. Name sanitized (we build a console command for SET).
function cvar(cmd: Command): void {
  const name = cmd.arg(0);
  if (!name || !/^[A-Za-z0-9_]+$/.test(name)) { cmd.replyT("Usage Cvar"); return; }
  if (cmd.argCount < 2) { cmd.replyT("Cvar Value", name, Server.getCvar(name)); return; }  // GET
  const value = cmd.argsFrom(1);
  // setCvar writes through ICvar (not the console), so `;` is data not a second command.
  // { and } are still rejected because "Cvar Set" echoes `value` through translate's {2}
  // substitution and __s2_tr_format strips braces from substituted args.
  if (/[\r\n{}]/.test(value)) { cmd.replyT("Invalid Cvar Value"); return; }
  console.log("[basecommands] sm_cvar SET " + name + " = " + value + " by slot=" + cmd.callerSlot);
  if (!Server.setCvar(name, value)) { cmd.replyT("Invalid Cvar Value"); return; }
  cmd.replyT("Cvar Set", name, Server.getCvar(name) || value);
}

// 6.12 — the `sm` command family (SM parity). PUBLIC command: `sm`/`version`/`credits`/`plugins list`
// are available to everyone (informational, exactly like SM). Only the MUTATING subcommands
// `plugins load|unload|reload` require ROOT — gated inline below (SM gates plugin management
// per-subcommand, not the whole `sm` command). Console (callerSlot < 0) is root.
//
// 6.11b — chat triggers (!cmd / /cmd) are handled in the core Host_Say detour; 6.11c — CONSOLE
// commands via the ISource2GameClients::ClientCommand hook. Every registered command is reachable
// from chat AND the client console with the speaker as the caller, with NO per-plugin wiring.
function sm(cmd: Command): void {
  const sub = cmd.arg(0).toLowerCase();
  if (!sub || sub === "version" || sub === "credits") {
    cmd.replyT("Sm Version");
    cmd.replyT("Sm Repo");
    return;
  }
  if (sub === "plugins") {
    const action = cmd.arg(1).toLowerCase();
    if (!action || action === "list") {
      const list = Plugins.list();
      cmd.replyT("Plugins Header", list.length);
      list.forEach((p, i) => cmd.replyT(p.loaded ? "Plugin List Row Running" : "Plugin List Row Unloaded", i + 1, p.id));
      return;
    }
    // Mutating plugin ops require ROOT. Server console is always root; a player needs the ROOT flag.
    const isRoot = cmd.callerSlot < 0 || (() => { const a = Admin.forSlot(cmd.callerSlot); return !!a && a.hasFlags(ADMFLAG.ROOT); })();
    if (!isRoot) { cmd.replyT("No access"); return; }
    const id = cmd.arg(2);
    if (!id) { cmd.replyT("Usage Sm Plugins"); return; }
    if (action === "unload") { cmd.replyT(Plugins.unload(id) ? "Unloading Plugin" : "Plugin Not Loaded", id); return; }
    if (action === "reload") { cmd.replyT(Plugins.reload(id) ? "Reloading Plugin" : "Plugin Not Found", id); return; }
    if (action === "load")   { cmd.replyT(Plugins.load(id)   ? "Loading Plugin"   : "Plugin Not Unloaded", id); return; }
    cmd.replyT("Usage Sm Plugins");
    return;
  }
  cmd.replyT("Sm Unknown Subcommand", sub);
}

// 6.6 — damage pre-hook (SDKHooks-equivalent). Logs the damage/attacker/type; halves damage as a demo of
// in-place modify. Fires on real bullet damage; also proven via the shim's first-frame synthetic self-test.
function halve(info: DamageInfo): void {
  const atk = info.attacker;
  const vic = info.victim;
  console.log("[basecommands] damage onPre: damage=" + info.damage + " type=" + info.damageType
    + " victim=" + (vic ? vic.index + "/" + vic.id : "none")
    + " attacker=" + (atk ? atk.index + "/" + atk.id : "none"));
  info.damage = info.damage / 2;   // modify: halve the damage (set to 0 would block)
}

import {
  command,
  topmenu,
  translations,
  Admin,
  ADMFLAG,
  Server,
  Plugins,
  Menu,
  MenuStyle,
  Translations,
  HookResult,
} from "@s2script/sdk";
import type { Command, HookResultValue } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

const MAP_CHOICES = ["de_dust2", "de_inferno", "de_mirage", "de_nuke", "de_ancient", "de_anubis"];

export function OnPluginStart(): void {
  translations.load("basecommands", "common");

  command.admin("sm_kick", ADMFLAG.KICK, kick);
  command.admin("sm_map", ADMFLAG.CHANGEMAP, map);
  command.admin("sm_who", ADMFLAG.GENERIC, who);
  command.admin("sm_reloadadmins", ADMFLAG.BAN, reloadAdmins);
  command.admin("sm_rcon", ADMFLAG.RCON, rcon);
  command.admin("sm_exec", ADMFLAG.CONFIG, execCfg);
  command.admin("sm_cvar", ADMFLAG.CONVARS, cvar);
  command("sm", sm);

  // Category string is a cross-plugin key (adminmenu matches it by equality). Untranslated on purpose.
  topmenu.addItem("Server Commands", {
    id: "basecommands:map",
    name: Translations.translate(-1, "Change Map Item"),
    flags: ADMFLAG.CHANGEMAP,
    onSelect: (adminSlot) => {
      const m = new Menu(Translations.translate(adminSlot, "Change Map Title"));
      m.style = MenuStyle.Center;
      m.freezePlayer = true;
      for (const mapName of MAP_CHOICES) if (Server.isMapValid(mapName)) m.addItem(mapName, mapName);
      m.onSelect((e) => { Server.command("changelevel " + e.info); });
      m.display(adminSlot, 30);
    },
  });
}

function kick(cmd: Command): HookResultValue {
  const targetStr = cmd.arg(0);
  if (!targetStr) { cmd.replyT("Usage Kick"); return HookResult.Handled; }
  const customReason = cmd.argsFrom(1);
  const targets = Player.target(targetStr, cmd.callerSlot, true);
  if (targets.length === 0) { cmd.replyT("No matching players"); return HookResult.Handled; }
  // COMMAND_FILTER_NO_MULTI: an ambiguous name kicks nobody. @all / #userid stay explicit.
  if (targets.length > 1 && targetStr[0] !== "@" && targetStr[0] !== "#") {
    cmd.replyT("Kick Ambiguous Target", targetStr);
    return HookResult.Handled;
  }
  let n = 0;
  for (const p of targets) {
    const reason = customReason || Translations.translate(p.slot, "Kick Reason Default");
    p.kick(reason);
    n++;
  }
  cmd.replyT(n === 1 ? "Kicked Player" : "Kicked Players", n);
  return HookResult.Handled;
}

function map(cmd: Command): HookResultValue {
  const mapName = cmd.arg(0);
  if (!mapName) { cmd.replyT("Usage Map"); return HookResult.Handled; }
  if (!/^[A-Za-z0-9_]+$/.test(mapName)) { cmd.replyT("Invalid Map Name"); return HookResult.Handled; }
  if (!Server.isMapValid(mapName)) { cmd.replyT("Map Not Valid", mapName); return HookResult.Handled; }
  cmd.replyT("Changing Map", mapName);
  Server.command("changelevel " + mapName);
  return HookResult.Handled;
}

/** SM who.sp letters: a..n = 1<<0..1<<13. ROOT (1<<14) prints as "root", not the letter soup. */
const FLAG_LETTERS = "abcdefghijklmn";

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

function pad(text: string, width: number, maxLen: number): string {
  return text.slice(0, maxLen).padEnd(width);
}

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

function who(cmd: Command): HookResultValue {
  const target = cmd.arg(0);
  if (target) { whoOne(cmd, target); return HookResult.Handled; }

  // SM's middle column is the admin username from admins.cfg. We have groups, not usernames.

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

  if (cmd.replySource === "chat") cmd.replyT("See Console For Output");
  return HookResult.Handled;
}

function reloadAdmins(cmd: Command): HookResultValue {
  Admin.reload();
  cmd.replyT("Admin Cache Reloaded");
  return HookResult.Handled;
}

function rcon(cmd: Command): HookResultValue {
  const c = cmd.argString.trim();
  if (!c) { cmd.replyT("Usage Rcon"); return HookResult.Handled; }
  Server.command(c);
  cmd.replyT("Rcon Command Sent");
  return HookResult.Handled;
}

function execCfg(cmd: Command): HookResultValue {
  const file = cmd.arg(0);
  if (!file) { cmd.replyT("Usage Exec"); return HookResult.Handled; }
  if (!/^[A-Za-z0-9_./-]+$/.test(file) || file.indexOf("..") !== -1) {
    cmd.replyT("Invalid Config Name");
    return HookResult.Handled;
  }
  Server.command("exec " + file);
  cmd.replyT("Executing Config", file);
  return HookResult.Handled;
}

function cvar(cmd: Command): HookResultValue {
  const name = cmd.arg(0);
  if (!name || !/^[A-Za-z0-9_]+$/.test(name)) { cmd.replyT("Usage Cvar"); return HookResult.Handled; }
  if (cmd.argCount < 2) { cmd.replyT("Cvar Value", name, Server.getCvar(name)); return HookResult.Handled; }
  const value = cmd.argsFrom(1);
  // setCvar writes through ICvar, so `;` is data. `{`/`}` would collide with translate substitution.
  if (/[\r\n{}]/.test(value)) { cmd.replyT("Invalid Cvar Value"); return HookResult.Handled; }
  if (!Server.setCvar(name, value)) { cmd.replyT("Invalid Cvar Value"); return HookResult.Handled; }
  cmd.replyT("Cvar Set", name, Server.getCvar(name) || value);
  return HookResult.Handled;
}

function sm(cmd: Command): HookResultValue {
  const sub = cmd.arg(0).toLowerCase();
  if (!sub || sub === "version" || sub === "credits") {
    cmd.replyT("Sm Version");
    cmd.replyT("Sm Repo");
    return HookResult.Handled;
  }
  if (sub === "plugins") {
    const action = cmd.arg(1).toLowerCase();
    if (!action || action === "list") {
      const list = Plugins.list();
      cmd.replyT("Plugins Header", list.length);
      list.forEach((p, i) => cmd.replyT(p.loaded ? "Plugin List Row Running" : "Plugin List Row Unloaded", i + 1, p.id));
      return HookResult.Handled;
    }
    const isRoot = cmd.callerSlot < 0 || (() => {
      const a = Admin.forSlot(cmd.callerSlot);
      return !!a && a.hasFlags(ADMFLAG.ROOT);
    })();
    if (!isRoot) { cmd.replyT("No access"); return HookResult.Handled; }
    const id = cmd.arg(2);
    if (!id) { cmd.replyT("Usage Sm Plugins"); return HookResult.Handled; }
    if (action === "unload") { cmd.replyT(Plugins.unload(id) ? "Unloading Plugin" : "Plugin Not Loaded", id); return HookResult.Handled; }
    if (action === "reload") { cmd.replyT(Plugins.reload(id) ? "Reloading Plugin" : "Plugin Not Found", id); return HookResult.Handled; }
    if (action === "load")   { cmd.replyT(Plugins.load(id)   ? "Loading Plugin"   : "Plugin Not Unloaded", id); return HookResult.Handled; }
    cmd.replyT("Usage Sm Plugins");
    return HookResult.Handled;
  }
  cmd.replyT("Sm Unknown Subcommand", sub);
  return HookResult.Handled;
}

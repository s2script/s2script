import { plugin } from "@s2script/sdk/plugin";
import { Admin, ADMFLAG } from "@s2script/sdk/admin";
import { Player } from "@s2script/cs2";
import { Server } from "@s2script/sdk/server";
import { Plugins } from "@s2script/sdk/plugins";
import { Menu, MenuStyle } from "@s2script/sdk/menu";

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
function flagString(admin: { flags: number } | null): string {
  const flags = admin?.flags ?? 0;
  if (flags === 0) return "none";
  if ((flags & ADMFLAG.ROOT) !== 0) return "root";

  let out = "";
  for (let i = 0; i < FLAG_LETTERS.length; i++) {
    if ((flags & (1 << i)) !== 0) out += FLAG_LETTERS[i];
  }
  return out.length > 0 ? out : "none";
}

/** `sm_who <target>` — SM's PerformWho: one line per resolved player, in the caller's own channel. */
function whoOne(cmd: { arg(n: number): string; reply(m: string): void; callerSlot: number }, pattern: string): void {
  const matches = Player.target(pattern, cmd.callerSlot);
  if (matches.length === 0) { cmd.reply("[SM] No matching client was found."); return; }

  for (const p of matches) {
    const admin = Admin.forSlot(p.slot);
    const name = p.playerName ?? "";
    if (admin === null) { cmd.reply(`[SM] "${name}" is not an admin.`); continue; }

    const groups = admin.groups.length > 0 ? admin.groups.join(",") : "";
    cmd.reply(
      groups.length > 0
        ? `[SM] "${name}" is logged in as "${groups}" with access: ${flagString(admin)}`
        : `[SM] "${name}" has access: ${flagString(admin)}`,
    );
  }
}

// Slice 6.2 live gate — admin-gated commands. Admin cache = host-global (file admins.json ⊕ runtime),
// from @s2script/admin. sm_say has moved to @s2script/basechat.
export default plugin((ctx) => {
  // 6.3 — sm_kick <target> [reason] (ADMFLAG.KICK). Resolves the SM target string (#userid/name/@all/@me)
  // and disconnects each match via the engine KickClient. Server console / rcon is root.
  ctx.commands.registerAdmin("sm_kick", ADMFLAG.KICK, (cmd) => {
    const targetStr = cmd.arg(0);
    if (!targetStr) { cmd.reply("Usage: sm_kick <target> [reason]"); return; }
    const reason = cmd.argsFrom(1) || "Kicked by admin";
    const targets = Player.target(targetStr, cmd.callerSlot, true);
    if (targets.length === 0) { cmd.reply("[SM] No matching players."); return; }
    // Destructive-command safety (SM COMMAND_FILTER_NO_MULTI): an ambiguous NAME matching >1 player kicks
    // nobody — @all / #userid stay the explicit multi/precise selectors; an exact name still resolves to 1.
    if (targets.length > 1 && targetStr[0] !== "@" && targetStr[0] !== "#") {
      cmd.reply("[SM] Multiple players match '" + targetStr + "' — be more specific (or use @all)."); return;
    }
    let n = 0;
    for (const p of targets) {
      console.log("[basecommands] sm_kick slot=" + p.slot + " name=" + p.playerName + " reason=" + reason);
      p.kick(reason);
      n++;
    }
    cmd.reply("[SM] Kicked " + n + " player" + (n === 1 ? "" : "s") + ".");
  });

  // 6.4 — sm_map <mapname> (ADMFLAG.CHANGEMAP). Sanitizes the name (injection guard, we build a
  // "changelevel <map>" string), rejects an invalid map cleanly, then changes level via @s2script/server.
  ctx.commands.registerAdmin("sm_map", ADMFLAG.CHANGEMAP, (cmd) => {
    const map = cmd.arg(0);
    if (!map) { cmd.reply("Usage: sm_map <mapname>"); return; }
    if (!/^[A-Za-z0-9_]+$/.test(map)) { cmd.reply("[SM] Invalid map name."); return; }
    if (!Server.isMapValid(map)) { cmd.reply("[SM] '" + map + "' is not a valid map."); return; }
    console.log("[basecommands] sm_map -> changelevel " + map + " by slot=" + cmd.callerSlot);
    cmd.reply("[SM] Changing map to " + map + "…");
    Server.command("changelevel " + map);
  });

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
  ctx.commands.registerAdmin("sm_who", ADMFLAG.GENERIC, (cmd) => {
    const target = cmd.arg(0);
    if (target) { whoOne(cmd, target); return; }

    cmd.replyToConsole("    " + pad("Name", 24, 23) + " " + pad("Groups", 18, 17) + " Admin access");

    for (const p of Player.allConnected()) {
      const admin = Admin.forSlot(p.slot);
      const groups = admin && admin.groups.length > 0 ? admin.groups.join(",") : "-";
      cmd.replyToConsole(
        String(p.slot + 1).padStart(2) + ". " +
        pad(p.playerName ?? "", 24, 23) + " " + pad(groups, 18, 17) + " " + flagString(admin),
      );
    }

    // SM only nudges when the answer went somewhere the caller isn't looking.
    if (cmd.replySource === "chat") cmd.reply("[SM] See console for output.");
  });

  // sm_reloadadmins (ADMFLAG.BAN) — SM parity (basecommands.sp:75). Re-reads admins.json into the
  // FILE tier, clearing the old file entries first. The runtime tier (Admin.add, e.g. permissions a
  // plugin derives from an external source) is untouched, so this reloads the static admin list
  // without revoking anything granted programmatically.
  ctx.commands.registerAdmin("sm_reloadadmins", ADMFLAG.BAN, (cmd) => {
    Admin.reload();
    console.log("[basecommands] sm_reloadadmins by slot=" + cmd.callerSlot);
    cmd.reply("[SM] Admin cache reloaded.");
  });

  // 6.5 — sm_rcon <command> (ADMFLAG.RCON): a deliberate full server-command passthrough (highest-trust flag).
  ctx.commands.registerAdmin("sm_rcon", ADMFLAG.RCON, (cmd) => {
    const c = cmd.argString.trim();
    if (!c) { cmd.reply("Usage: sm_rcon <command>"); return; }
    console.log("[basecommands] sm_rcon by slot=" + cmd.callerSlot + " cmd=" + c);
    Server.command(c);
    cmd.reply("[SM] Command sent.");
  });

  // 6.5 — sm_exec <cfgfile> (ADMFLAG.CONFIG): exec a server config. Sanitize the filename (we build "exec <file>").
  ctx.commands.registerAdmin("sm_exec", ADMFLAG.CONFIG, (cmd) => {
    const file = cmd.arg(0);
    if (!file) { cmd.reply("Usage: sm_exec <cfgfile>"); return; }
    if (!/^[A-Za-z0-9_./-]+$/.test(file) || file.indexOf("..") !== -1) { cmd.reply("[SM] Invalid config name."); return; }
    console.log("[basecommands] sm_exec by slot=" + cmd.callerSlot + " file=" + file);
    Server.command("exec " + file);
    cmd.reply("[SM] Executing " + file + ".");
  });

  // 6.6 — damage pre-hook (SDKHooks-equivalent). Logs the damage/attacker/type; halves damage as a demo of
  // in-place modify. Fires on real bullet damage; also proven via the shim's first-frame synthetic self-test.
  ctx.entities.onDamage((info) => {
    const atk = info.attacker;
    const vic = info.victim;
    console.log("[basecommands] damage onPre: damage=" + info.damage + " type=" + info.damageType
      + " victim=" + (vic ? vic.index + "/" + vic.id : "none")
      + " attacker=" + (atk ? atk.index + "/" + atk.id : "none"));
    info.damage = info.damage / 2;   // modify: halve the damage (set to 0 would block)
  });

  // 6.7 — sm_cvar <name> [value] (ADMFLAG.CVARS). No value → GET (reply the value); with a value → SET
  // (via the console) then read back. Name sanitized (we build a console command for SET).
  ctx.commands.registerAdmin("sm_cvar", ADMFLAG.CONVARS, (cmd) => {
    const name = cmd.arg(0);
    if (!name || !/^[A-Za-z0-9_]+$/.test(name)) { cmd.reply("Usage: sm_cvar <name> [value]"); return; }
    if (cmd.argCount < 2) { cmd.reply("[SM] " + name + " = " + Server.getCvar(name)); return; }  // GET
    const value = cmd.argsFrom(1);
    // SECURITY: setCvar concatenates into a server console command, which splits on ';'. Reject the
    // console-injection chars so an ADMFLAG.CONVARS admin can't escalate to arbitrary server commands
    // (e.g. `sm_cvar x "0; sv_cheats 1"`); quote the value so a legit multi-word string cvar is one token.
    if (/[;"\r\n]/.test(value)) { cmd.reply("[SM] Invalid cvar value (no ; or quotes)."); return; }
    console.log("[basecommands] sm_cvar SET " + name + " = " + value + " by slot=" + cmd.callerSlot);
    Server.setCvar(name, '"' + value + '"');
    // NOTE: Server.command queues the set for next frame, so an immediate getCvar reads the OLD value —
    // echo the requested value instead of a stale read-back.
    cmd.reply("[SM] " + name + " set to " + value);
  });

  // 6.11b — chat triggers (!cmd / /cmd) are handled in the core Host_Say detour; 6.11c — CONSOLE commands
  // via the ISource2GameClients::ClientCommand hook. Every registered command (sm_say, sm_kick, sm, …) is
  // reachable from chat AND the client console with the speaker as the caller, with NO per-plugin wiring.

  // 6.12 — the `sm` command family (SM parity). PUBLIC command (ctx.commands.register, not registerAdmin):
  // `sm`/`version`/`credits`/`plugins list` are available to everyone (informational, exactly like SM).
  // Only the MUTATING subcommands `plugins load|unload|reload` require ROOT — gated inline below (SM
  // gates plugin management per-subcommand, not the whole `sm` command). Console (callerSlot < 0) is root.
  ctx.commands.register("sm", (cmd) => {
    const sub = cmd.arg(0).toLowerCase();
    if (!sub || sub === "version" || sub === "credits") {
      cmd.reply("[SM] s2script 0.1.0 — a TypeScript plugin framework for Source 2 / CS2, by Gabriel Hirakawa.");
      cmd.reply("[SM] github.com/s2script/s2script");
      return;
    }
    if (sub === "plugins") {
      const action = cmd.arg(1).toLowerCase();
      if (!action || action === "list") {
        const list = Plugins.list();
        cmd.reply("[SM] Plugins (" + list.length + "):");
        list.forEach((p, i) => cmd.reply("  " + (i + 1) + ' "' + p.id + '" ' + (p.loaded ? "(running)" : "(unloaded)")));
        return;
      }
      // Mutating plugin ops require ROOT. Server console is always root; a player needs the ROOT flag.
      const isRoot = cmd.callerSlot < 0 || (() => { const a = Admin.forSlot(cmd.callerSlot); return !!a && a.hasFlags(ADMFLAG.ROOT); })();
      if (!isRoot) { cmd.reply("[SM] You do not have access to this command."); return; }
      const id = cmd.arg(2);
      if (!id) { cmd.reply("Usage: sm plugins <list|load|unload|reload> [id]"); return; }
      if (action === "unload") { cmd.reply(Plugins.unload(id) ? "[SM] Unloading '" + id + "'…" : "[SM] Not a loaded plugin: " + id); return; }
      if (action === "reload") { cmd.reply(Plugins.reload(id) ? "[SM] Reloading '" + id + "'…" : "[SM] No such plugin: " + id); return; }
      if (action === "load")   { cmd.reply(Plugins.load(id)   ? "[SM] Loading '" + id + "'…"   : "[SM] Plugin is not unloaded: " + id); return; }
      cmd.reply("Usage: sm plugins <list|load|unload|reload> [id]");
      return;
    }
    cmd.reply("[SM] Unknown sub-command '" + sub + "'. Try: sm plugins list");
  });

  // 6.2 live-gate diagnostic: prove the admin cache works live (rcon-verifiable, no human client needed).
  Admin.add("76561199000000009", ADMFLAG.KICK | ADMFLAG.CHAT);   // runtime tier
  const t = Admin.get("76561199000000009");
  console.log("[basecommands] admin diag: runtime-add hasKick=" + (t ? String(t.hasFlags(ADMFLAG.KICK)) : "null")
    + " hasBan=" + (t ? String(t.hasFlags(ADMFLAG.BAN)) : "null"));
  console.log("[basecommands] admin diag: slot0=" + (Admin.forSlot(0) ? "admin" : "not-admin (bot/steamid=0)"));

  ctx.topmenu.addItem("Server Commands", { id: "basecommands:map", name: "Change Map", flags: ADMFLAG.CHANGEMAP,
    onSelect: adminSlot => {
      const m = new Menu("Change Map");
      m.style = MenuStyle.Center;
      m.freezePlayer = true;   // WASD nav — keep the admin frozen through the sub-menu
      for (const map of MAP_CHOICES) if (Server.isMapValid(map)) m.addItem(map, map);
      m.onSelect(e => { Server.command("changelevel " + e.info); });
      m.display(adminSlot, 30);
    } });

  console.log("[basecommands] onLoad — kick/map/who/rcon/exec/cvar/sm registered");
});

import { plugin } from "@s2script/sdk/plugin";
import { Admin, ADMFLAG } from "@s2script/sdk/admin";
import { Player } from "@s2script/cs2";
import { Server } from "@s2script/sdk/server";
import { Plugins } from "@s2script/sdk/plugins";
import { Menu, MenuStyle } from "@s2script/sdk/menu";

// adminmenu — Change Map proof item (Server Commands, ADMFLAG.CHANGEMAP), a curated map picker filtered
// by Server.isMapValid so an uninstalled map never shows.
const MAP_CHOICES = ["de_dust2", "de_inferno", "de_mirage", "de_nuke", "de_ancient", "de_anubis"];

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

  // 6.5 — sm_who (ADMFLAG.GENERIC): list connected players + their admin status (Player.allConnected + Admin.forSlot).
  ctx.commands.registerAdmin("sm_who", ADMFLAG.GENERIC, (cmd) => {
    const players = Player.allConnected();
    cmd.reply("[SM] Players (" + players.length + "):");
    for (const p of players) {
      const a = Admin.forSlot(p.slot);
      const adminStr = a ? "yes(flags=0x" + a.flags.toString(16) + ")" : "no";
      cmd.reply("  #" + p.userId + " " + p.playerName + " slot=" + p.slot + " steamid=" + p.steamId + " admin=" + adminStr);
    }
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

import { Commands } from "@s2script/commands";
import { Admin, ADMFLAG } from "@s2script/admin";
import { Player } from "@s2script/cs2";
import { Server } from "@s2script/server";
import { Damage } from "@s2script/damage";
import { Plugins } from "@s2script/plugins";

// Slice 6.2 live gate — admin-gated commands. Admin cache = host-global (file admins.json ⊕ runtime),
// from @s2script/admin. sm_say has moved to @s2script/basechat.
export function onLoad(): void {
  // 6.3 — sm_kick <target> [reason] (ADMFLAG.KICK). Resolves the SM target string (#userid/name/@all/@me)
  // and disconnects each match via the engine KickClient. Server console / rcon is root.
  Commands.registerAdmin("sm_kick", ADMFLAG.KICK, (ctx) => {
    const targetStr = ctx.arg(0);
    if (!targetStr) { ctx.reply("Usage: sm_kick <target> [reason]"); return; }
    const reason = ctx.argsFrom(1) || "Kicked by admin";
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
      console.log("[basecommands] sm_slap slot=" + p.slot + " hpBefore=" + hpBefore + " hpAfter=" + pawn.health);
      n++;
    }
    ctx.reply("[SM] Slapped " + n + " player" + (n === 1 ? "" : "s") + " for " + damage + " damage.");
  });

  // 6.4 — sm_map <mapname> (ADMFLAG.CHANGEMAP). Sanitizes the name (injection guard, we build a
  // "changelevel <map>" string), rejects an invalid map cleanly, then changes level via @s2script/server.
  Commands.registerAdmin("sm_map", ADMFLAG.CHANGEMAP, (ctx) => {
    const map = ctx.arg(0);
    if (!map) { ctx.reply("Usage: sm_map <mapname>"); return; }
    if (!/^[A-Za-z0-9_]+$/.test(map)) { ctx.reply("[SM] Invalid map name."); return; }
    if (!Server.isMapValid(map)) { ctx.reply("[SM] '" + map + "' is not a valid map."); return; }
    console.log("[basecommands] sm_map -> changelevel " + map + " by slot=" + ctx.callerSlot);
    ctx.reply("[SM] Changing map to " + map + "…");
    Server.command("changelevel " + map);
  });

  // 6.5 — sm_who (ADMFLAG.GENERIC): list connected players + their admin status (Player.allConnected + Admin.forSlot).
  Commands.registerAdmin("sm_who", ADMFLAG.GENERIC, (ctx) => {
    const players = Player.allConnected();
    ctx.reply("[SM] Players (" + players.length + "):");
    for (const p of players) {
      const a = Admin.forSlot(p.slot);
      const adminStr = a ? "yes(flags=0x" + a.flags.toString(16) + ")" : "no";
      ctx.reply("  #" + p.userId + " " + p.playerName + " slot=" + p.slot + " steamid=" + p.steamId + " admin=" + adminStr);
    }
  });

  // 6.5 — sm_rcon <command> (ADMFLAG.RCON): a deliberate full server-command passthrough (highest-trust flag).
  Commands.registerAdmin("sm_rcon", ADMFLAG.RCON, (ctx) => {
    const cmd = ctx.argString.trim();
    if (!cmd) { ctx.reply("Usage: sm_rcon <command>"); return; }
    console.log("[basecommands] sm_rcon by slot=" + ctx.callerSlot + " cmd=" + cmd);
    Server.command(cmd);
    ctx.reply("[SM] Command sent.");
  });

  // 6.5 — sm_exec <cfgfile> (ADMFLAG.CONFIG): exec a server config. Sanitize the filename (we build "exec <file>").
  Commands.registerAdmin("sm_exec", ADMFLAG.CONFIG, (ctx) => {
    const file = ctx.arg(0);
    if (!file) { ctx.reply("Usage: sm_exec <cfgfile>"); return; }
    if (!/^[A-Za-z0-9_./-]+$/.test(file) || file.indexOf("..") !== -1) { ctx.reply("[SM] Invalid config name."); return; }
    console.log("[basecommands] sm_exec by slot=" + ctx.callerSlot + " file=" + file);
    Server.command("exec " + file);
    ctx.reply("[SM] Executing " + file + ".");
  });

  // 6.6 — damage pre-hook (SDKHooks-equivalent). Logs the damage/attacker/type; halves damage as a demo of
  // in-place modify. Fires on real bullet damage; also proven via the shim's first-frame synthetic self-test.
  Damage.onPre((info) => {
    const atk = info.attacker;
    const vic = info.victim;
    console.log("[basecommands] damage onPre: damage=" + info.damage + " type=" + info.damageType
      + " victim=" + (vic ? vic.index + "/" + vic.serial : "none")
      + " attacker=" + (atk ? atk.index + "/" + atk.serial : "none"));
    info.damage = info.damage / 2;   // modify: halve the damage (set to 0 would block)
  });

  // 6.7 — sm_cvar <name> [value] (ADMFLAG.CVARS). No value → GET (reply the value); with a value → SET
  // (via the console) then read back. Name sanitized (we build a console command for SET).
  Commands.registerAdmin("sm_cvar", ADMFLAG.CONVARS, (ctx) => {
    const name = ctx.arg(0);
    if (!name || !/^[A-Za-z0-9_]+$/.test(name)) { ctx.reply("Usage: sm_cvar <name> [value]"); return; }
    if (ctx.argCount < 2) { ctx.reply("[SM] " + name + " = " + Server.getCvar(name)); return; }  // GET
    const value = ctx.argsFrom(1);
    // SECURITY: setCvar concatenates into a server console command, which splits on ';'. Reject the
    // console-injection chars so an ADMFLAG.CONVARS admin can't escalate to arbitrary server commands
    // (e.g. `sm_cvar x "0; sv_cheats 1"`); quote the value so a legit multi-word string cvar is one token.
    if (/[;"\r\n]/.test(value)) { ctx.reply("[SM] Invalid cvar value (no ; or quotes)."); return; }
    console.log("[basecommands] sm_cvar SET " + name + " = " + value + " by slot=" + ctx.callerSlot);
    Server.setCvar(name, '"' + value + '"');
    // NOTE: Server.command queues the set for next frame, so an immediate getCvar reads the OLD value —
    // echo the requested value instead of a stale read-back.
    ctx.reply("[SM] " + name + " set to " + value);
  });

  // 6.11b — chat triggers (!cmd / /cmd) are handled in the core Host_Say detour; 6.11c — CONSOLE commands
  // via the ISource2GameClients::ClientCommand hook. Every registered command (sm_say, sm_kick, sm, …) is
  // reachable from chat AND the client console with the speaker as the caller, with NO per-plugin wiring.

  // 6.12 — the `sm` command family (SM parity). PUBLIC command (Commands.register, not registerAdmin):
  // `sm`/`version`/`credits`/`plugins list` are available to everyone (informational, exactly like SM).
  // Only the MUTATING subcommands `plugins load|unload|reload` require ROOT — gated inline below (SM
  // gates plugin management per-subcommand, not the whole `sm` command). Console (callerSlot < 0) is root.
  Commands.register("sm", (ctx) => {
    const sub = ctx.arg(0).toLowerCase();
    if (!sub || sub === "version" || sub === "credits") {
      ctx.reply("[SM] s2script 0.1.0 — a TypeScript plugin framework for Source 2 / CS2, by Gabriel Hirakawa.");
      ctx.reply("[SM] github.com/GabeHirakawa/s2script");
      return;
    }
    if (sub === "plugins") {
      const action = ctx.arg(1).toLowerCase();
      if (!action || action === "list") {
        const list = Plugins.list();
        ctx.reply("[SM] Plugins (" + list.length + "):");
        list.forEach((p, i) => ctx.reply("  " + (i + 1) + ' "' + p.id + '" ' + (p.loaded ? "(running)" : "(unloaded)")));
        return;
      }
      // Mutating plugin ops require ROOT. Server console is always root; a player needs the ROOT flag.
      const isRoot = ctx.callerSlot < 0 || (() => { const a = Admin.forSlot(ctx.callerSlot); return !!a && a.hasFlags(ADMFLAG.ROOT); })();
      if (!isRoot) { ctx.reply("[SM] You do not have access to this command."); return; }
      const id = ctx.arg(2);
      if (!id) { ctx.reply("Usage: sm plugins <list|load|unload|reload> [id]"); return; }
      if (action === "unload") { ctx.reply(Plugins.unload(id) ? "[SM] Unloading '" + id + "'…" : "[SM] Not a loaded plugin: " + id); return; }
      if (action === "reload") { ctx.reply(Plugins.reload(id) ? "[SM] Reloading '" + id + "'…" : "[SM] No such plugin: " + id); return; }
      if (action === "load")   { ctx.reply(Plugins.load(id)   ? "[SM] Loading '" + id + "'…"   : "[SM] Plugin is not unloaded: " + id); return; }
      ctx.reply("Usage: sm plugins <list|load|unload|reload> [id]");
      return;
    }
    ctx.reply("[SM] Unknown sub-command '" + sub + "'. Try: sm plugins list");
  });

  // 6.2 live-gate diagnostic: prove the admin cache works live (rcon-verifiable, no human client needed).
  Admin.add("76561199000000009", ADMFLAG.KICK | ADMFLAG.CHAT);   // runtime tier
  const t = Admin.get("76561199000000009");
  console.log("[basecommands] admin diag: runtime-add hasKick=" + (t ? String(t.hasFlags(ADMFLAG.KICK)) : "null")
    + " hasBan=" + (t ? String(t.hasFlags(ADMFLAG.BAN)) : "null"));
  console.log("[basecommands] admin diag: slot0=" + (Admin.forSlot(0) ? "admin" : "not-admin (bot/steamid=0)"));
  console.log("[basecommands] onLoad — kick/slap/map/who/rcon/exec/cvar registered");
}

export function onUnload(): void { console.log("[basecommands] onUnload"); }

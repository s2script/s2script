import { ADMFLAG, command, HookResult } from "@s2script/sdk";

/**
 * `command` / `command.server` / `command.admin` differ only in WHO may reach the handler —
 * nothing in the handler itself decides access:
 *
 *   command(name, fn)              — any connected client, plus the server console.
 *   command.server(name, fn)       — server console / rcon only (SM's server-only commands); a
 *                                    client typing it in their own console never reaches it.
 *   command.admin(name, flags, fn) — gated by an ADMFLAG bitmask, checked by the HOST before the
 *                                    handler runs at all: fail-safe default-deny. A caller
 *                                    missing the flag (or with no admin entry) is refused with
 *                                    no code in this file making that decision — see
 *                                    plugins/adminhelp, whose sm_help is command.admin-gated on
 *                                    ADMFLAG.GENERIC exactly like sm_adminflags_gated below. The server
 *                                    console always passes an admin gate (SM parity).
 *
 * Owned handlers return HookResult.Handled (SM Plugin_Handled), usage errors included.
 *
 * ADMFLAG's bits are SourceMod-parity: GENERIC is the baseline "is an admin" flag;
 * KICK/BAN/SLAY/etc. are narrower per-action flags a real command would pick instead.
 */
export const name = "admin";
export const describe = "command vs command.server vs command.admin (sm_adminflags / sm_adminflags_server / sm_adminflags_gated)";

export function OnPluginStart(): void {
  command("sm_adminflags", (cmd) => {
    cmd.reply("sm_adminflags: anyone can run this (command()). Now try sm_adminflags_server from " +
      "an in-game console (refused) vs the SERVER console (works), and sm_adminflags_gated as a non-admin " +
      "(refused, no code here decided that).");
    return HookResult.Handled;
  });

  command.server("sm_adminflags_server", (cmd) => {
    cmd.reply("sm_adminflags_server: reached the handler — this command only exists for the server console/rcon.");
    return HookResult.Handled;
  });

  command.admin("sm_adminflags_gated", ADMFLAG.GENERIC, (cmd) => {
    cmd.reply("sm_adminflags_gated: you passed the ADMFLAG.GENERIC gate (or you're the server console).");
    return HookResult.Handled;
  });
}

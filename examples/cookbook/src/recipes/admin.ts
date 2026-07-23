import type { Recipe } from "../recipe.ts";
import { ADMFLAG } from "@s2script/sdk/admin";

/**
 * `ctx.commands` has three registration methods that differ only in WHO may reach the handler —
 * nothing in the handler itself decides access:
 *
 *   register(name, fn)             — any connected client, plus the server console.
 *   registerServer(name, fn)       — server console / rcon only (SM's server-only commands); a
 *                                     client typing it in their own console never reaches it.
 *   registerAdmin(name, flags, fn) — gated by an ADMFLAG bitmask, checked by the HOST before the
 *                                     handler runs at all: fail-safe default-deny. A caller
 *                                     missing the flag (or with no admin entry) is refused with
 *                                     no code in this file making that decision — see
 *                                     plugins/adminhelp, whose sm_help is registerAdmin-gated on
 *                                     ADMFLAG.GENERIC exactly like sm_adminflags_gated below. The server
 *                                     console always passes an admin gate (SM parity).
 *
 * ADMFLAG's bits are SourceMod-parity (see @s2script/sdk/admin): GENERIC is the baseline "is an
 * admin" flag; KICK/BAN/SLAY/etc. are narrower per-action flags a real command would pick instead.
 */
export const adminRecipe: Recipe = {
  name: "admin",
  describe: "register vs registerServer vs registerAdmin (sm_adminflags / sm_adminflags_server / sm_adminflags_gated)",
  register(ctx) {
    ctx.commands.register("sm_adminflags", (cmd) => {
      cmd.reply("sm_adminflags: anyone can run this (ctx.commands.register). Now try sm_adminflags_server from " +
        "an in-game console (refused) vs the SERVER console (works), and sm_adminflags_gated as a non-admin " +
        "(refused, no code here decided that).");
    });

    ctx.commands.registerServer("sm_adminflags_server", (cmd) => {
      cmd.reply("sm_adminflags_server: reached the handler — this command only exists for the server console/rcon.");
    });

    ctx.commands.registerAdmin("sm_adminflags_gated", ADMFLAG.GENERIC, (cmd) => {
      cmd.reply("sm_adminflags_gated: you passed the ADMFLAG.GENERIC gate (or you're the server console).");
    });
  },
};

import type { Recipe } from "../recipe.ts";
import { Server } from "@s2script/sdk/server";
import { Clients } from "@s2script/sdk/clients";
import { Player } from "@s2script/cs2";

/**
 * Server ties together three primitives: a plugin-owned ConVar (register at
 * load; idempotent, and the value persists across reloads — read it back
 * through Server.getCvar), the OnMapStart lifecycle event (boot-loaded
 * plugins see the first map's fire; a changelevel fires it again), and the
 * engine-generic Clients list cross-referenced against the CS2 Player
 * wrapper.
 */
export const serverRecipe: Recipe = {
  name: "server",
  describe: "a registered cvar, OnMapStart, and the connected client list (cb_server)",
  register(ctx) {
    const ok = Server.registerCvar("s2_demo_mode", {
      type: "int", default: 42, help: "cookbook clientlist/convar/mapstart demo cvar", min: 0, max: 100,
    });
    console.log(`[cookbook] server: registerCvar s2_demo_mode -> ${ok} value=${Server.getCvar("s2_demo_mode")}`);

    ctx.server.onMapStart((map) => {
      console.log(`[cookbook] server: onMapStart: ${map}`);
    });

    ctx.commands.register("cb_server", (cmd) => {
      const cs = Clients.all();
      cmd.reply(`[cookbook] server: clients=${cs.length} players=${Player.allConnected().length} map=${Server.mapName}`);
      for (const c of cs) {
        const back = Player.fromUserId(c.userId);
        cmd.reply(`  slot=${c.slot} name=${c.name} userId=${c.userId} signon=${c.signonState} ` +
                  `steamid=${c.steamId} fromUserId->slot=${back ? back.slot : -1}`);
      }
    });
  },
};

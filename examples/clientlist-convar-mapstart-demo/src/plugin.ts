import { Commands } from "@s2script/sdk/commands";
import { Server } from "@s2script/sdk/server";
import { Clients } from "@s2script/sdk/clients";
import { Player } from "@s2script/cs2";

export function onLoad(): void {
  // Feature 2: FakeConVar — register at load; read back through the 6.7 cvar_get path.
  const ok = Server.registerCvar("s2_demo_mode", {
    type: "int", default: 42, help: "clientlist-convar-mapstart demo cvar", min: 0, max: 100,
  });
  console.log(`[cl-demo] registerCvar s2_demo_mode -> ${ok} value=${Server.getCvar("s2_demo_mode")}`);

  // Feature 3: OnMapStart — boot-loaded plugins see the first map's fire; changelevel fires again.
  Server.onMapStart((map) => {
    console.log(`[cl-demo] onMapStart: ${map}`);
  });

  // Feature 1: the client list through the refactored ops (engine-generic Clients + CS2 Player).
  Commands.register("sm_clients", (ctx) => {
    const cs = Clients.all();
    ctx.reply(`[cl-demo] clients=${cs.length} players=${Player.allConnected().length} map=${Server.mapName}`);
    for (const c of cs) {
      const back = Player.fromUserId(c.userId);
      ctx.reply(`  slot=${c.slot} name=${c.name} userId=${c.userId} signon=${c.signonState} ` +
                `steamid=${c.steamId} fromUserId->slot=${back ? back.slot : -1}`);
    }
  });

  console.log("[cl-demo] onLoad — sm_clients registered");
}

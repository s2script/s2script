// Lifecycle stimulus only. No raw native pointer or new engine binding API.
import { Server, command, Clients } from "@s2script/sdk";
import { Player } from "@s2script/cs2";
import { REVISION, TOKEN } from "./build_identity";
let generation = 0;
let run = "";
let operation = 0;
function record(event: string, facts: Record<string, unknown> = {}): void {
  console.log(JSON.stringify({ kind: "engine-function-script", event, source: REVISION, token: TOKEN, run, generation, facts }));
}
export function OnPluginStart(): void {
  Server.registerCvar("s2_engine_fixture_generation", { type: "int", default: 0 });
  for (const name of ["active", "slot", "pawn", "scope", "operation"]) {
    Server.registerCvar(`s2_engine_fixture_${name}`, { type: "int", default: -1 });
    Server.setCvar(`s2_engine_fixture_${name}`, "-1");
  }
  generation = Number(Server.getCvar("s2_engine_fixture_generation")) + 1;
  Server.setCvar("s2_engine_fixture_generation", String(generation));
  command("s2_engine_accept", cmd => {
    if (cmd.callerSlot >= 0) return;
    const op = cmd.arg(0), requested = cmd.arg(1);
    if (!/^[A-Za-z0-9_-]+$/.test(requested) || TOKEN === "unbuilt") { cmd.reply("unbound or unbuilt fixture"); return; }
    if (op === "arm") {
      run = requested;
      record("arm");
      Server.command(`s2_engine_probe arm ${run} ${generation} ${TOKEN}`);
    } else if (requested === run && op === "acquire") {
      const bot = Clients.all().find(client => client.isBot && client.isValid() && client.name === `s2fn_${run}`);
      const pawn = bot ? Player.fromSlot(bot.slot)?.pawn : null;
      if (!bot || !pawn?.isValid) { record("acquire-pending", { reason: "real bot pawn required" }); return; }
      Server.setCvar("s2_engine_fixture_slot", String(bot.slot));
      Server.setCvar("s2_engine_fixture_pawn", String(pawn.ref.index));
      Server.setCvar("s2_engine_fixture_operation", String(++operation));
      Server.setCvar("s2_engine_fixture_scope", "1");
      Server.setCvar("s2_engine_fixture_active", String(generation));
      try {
        const item = pawn.giveNamedItem("weapon_decoy");
        record("acquire-stimulus", { operation, botSlot: bot.slot, pawn: pawn.ref.index, itemCreated: item !== null });
        if (item) pawn.removeWeapon(item);
      } finally {
        Server.setCvar("s2_engine_fixture_active", "-1");
        Server.setCvar("s2_engine_fixture_scope", "-1");
      }
      Server.command(`s2_engine_probe acquire-results ${run}`);
    } else cmd.reply("usage: s2_engine_accept arm|acquire <run>");
  });
  record("loaded");
}
export function OnPluginEnd(): void {
  record("unloaded");
  if (run) Server.command(`s2_engine_probe retire ${run} ${generation}`);
}

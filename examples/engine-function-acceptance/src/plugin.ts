// Lifecycle stimulus only. No raw native pointer or new engine binding API.
import { Server, command, Clients, type Client } from "@s2script/sdk";
import { Player } from "@s2script/cs2";
import { REVISION, TOKEN } from "./build_identity";
let generation = 0;
let run = "";
let operation = 0;
let captured: Client | null = null;
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
      run = requested; captured = null;
      record("arm");
      Server.command(`s2_engine_probe arm ${run} ${generation} ${TOKEN}`);
    } else if (requested === run && op === "capture") {
      const slot = Number(Server.getCvar("s2_engine_owned_slot"));
      const userId = Number(Server.getCvar("s2_engine_owned_userid"));
      const candidate = slot >= 0 ? Clients.fromSlot(slot) : null;
      captured = candidate?.isValid() && candidate.isBot && candidate.ip === "" && candidate.signonState === 6 && userId >= 0 && candidate.userId === userId ? candidate : null;
      record(captured ? "bot-captured" : "capture-refused", { slot, userId });
    } else if (requested === run && op === "acquire") {
      const slot = Number(Server.getCvar("s2_engine_owned_slot"));
      const userId = Number(Server.getCvar("s2_engine_owned_userid"));
      const bot = captured;
      const pawn = bot ? Player.fromSlot(bot.slot)?.pawn : null;
      if (!bot || !bot.isValid() || bot.slot !== slot || !bot.isBot || bot.ip !== "" || bot.signonState !== 6 || userId < 0 || bot.userId !== userId || !pawn?.isValid) { record("acquire-pending", { reason: "real bot pawn required" }); return; }
      Server.setCvar("s2_engine_fixture_slot", String(bot.slot));
      Server.setCvar("s2_engine_fixture_pawn", String(pawn.ref.index));
      Server.setCvar("s2_engine_fixture_operation", String(++operation));
      Server.setCvar("s2_engine_fixture_scope", "1");
      Server.setCvar("s2_engine_fixture_active", String(generation));
      try {
        const item = pawn.giveNamedItem("weapon_decoy");
        record("acquire-stimulus", { operation, botSlot: bot.slot, userId: bot.userId, pawn: pawn.ref.index, itemCreated: item !== null });
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

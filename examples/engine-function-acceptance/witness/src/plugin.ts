// Independent idle owner B observes the public package surface while owner A
// is synchronously inside giveNamedItem. No private pointer/native JS API.
import { Server, command, Clients } from "@s2script/sdk";
import { items, type CanAcquireView } from "@s2script/cs2";
import { REVISION, TOKEN } from "./build_identity";
let run = "";
let generation = 0;
let witnessGeneration = 0;
let sequence = 0;
function record(event: string, facts: Record<string, unknown> = {}): void {
  console.log(JSON.stringify({ kind: "engine-function-witness", event, source: REVISION, token: TOKEN, run, generation, witnessGeneration, witnessSequence: ++sequence, facts }));
}
function observe(event: string, view: CanAcquireView): void {
  if (!run || Number(Server.getCvar("s2_engine_fixture_active")) !== generation) return;
  const slot = Number(Server.getCvar("s2_engine_fixture_slot"));
  const bot = Clients.all().find(client => client.slot === slot && client.isBot && client.isValid() && client.name === `s2fn_${run}`);
  if (!bot || view.player?.slot !== slot) return;
  const id = Number(Server.getCvar("s2_engine_fixture_scope"));
  const operation = Number(Server.getCvar("s2_engine_fixture_operation"));
  if ((id !== 1 && id !== 2) || operation <= 0) { record("invalid-scope"); return; }
  record(event, { id, operation, slot, pawn: Number(Server.getCvar("s2_engine_fixture_pawn")), method: view.method, defIndex: view.defIndex, result: view.result, skipped: view.skipped });
}
export function OnPluginStart(): void {
  Server.registerCvar("s2_engine_witness_generation", { type: "int", default: 0 });
  witnessGeneration = Number(Server.getCvar("s2_engine_witness_generation")) + 1;
  Server.setCvar("s2_engine_witness_generation", String(witnessGeneration));
  command("s2_engine_witness", cmd => {
    if (cmd.callerSlot >= 0) return;
    const requested = cmd.arg(1), next = Number(cmd.arg(2));
    if (cmd.arg(0) !== "arm" || !/^[A-Za-z0-9_-]+$/.test(requested) || cmd.arg(3) !== TOKEN || TOKEN === "unbuilt" || !Number.isSafeInteger(next) || next <= generation || next !== Number(Server.getCvar("s2_engine_fixture_generation"))) return;
    run = requested; generation = next; record("armed");
  });
  items.onCanAcquire(view => observe("acquire-pre", view));
  items.onCanAcquirePost(view => observe("acquire-post", view));
  record("loaded");
}
export function OnPluginEnd(): void { record("unloaded"); }

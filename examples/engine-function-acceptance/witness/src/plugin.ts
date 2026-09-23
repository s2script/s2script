// Independent idle owner B observes the public package surface while owner A
// is synchronously inside giveNamedItem. No private pointer/native JS API.
import { Server, command, Clients, type Client } from "@s2script/sdk";
import { items, Player, type CanAcquireView } from "@s2script/cs2";
import { REVISION, TOKEN } from "./build_identity";
let run = "";
let generation = 0;
let witnessGeneration = 0;
let sequence = 0;
function record(event: string, facts: Record<string, unknown> = {}): void {
  console.log(JSON.stringify({ kind: "engine-function-witness", event, source: REVISION, token: TOKEN, run, generation, witnessGeneration, witnessSequence: ++sequence, facts }));
}
let owned: Client | null = null;
let ownedUserId = -1;
let baseline: Client[] = [];
let creating = false;
let attempted = false;
let deadline = 0;
let refused = false;
let saved: Record<string, string> = {};
function validBot(client: Client | null): client is Client {
  return client !== null && client.isValid() && client.isBot && client.ip === "" && client.signonState === 6 && client.userId >= 0;
}
function ownedValid(): boolean { return validBot(owned) && owned.userId === ownedUserId; }
function publish(): void {
  Server.setCvar("s2_engine_owned_slot", !refused && ownedValid() ? String(owned?.slot) : "-1");
  Server.setCvar("s2_engine_owned_userid", !refused && ownedValid() ? String(ownedUserId) : "-1");
}
function refuse(reason: string): void { refused = true; creating = false; publish(); record("bot-refused", { reason }); }
function poll(): void {
  if (refused) return;
  if (baseline.some(client => !client.isValid())) { refuse("baseline connection changed"); return; }
  if (!owned && creating) {
    const added = Clients.all().filter(client => client.isValid() && !baseline.some(prior => prior.slot === client.slot));
    if (added.length > 1 || added.some(client => !client.isBot || client.ip !== "")) { refuse("new client identity ambiguous"); return; }
    if (added.length === 1 && validBot(added[0])) {
      owned = added[0]; ownedUserId = owned.userId; creating = false; publish();
      record("bot-owned", { slot: owned.slot, userId: ownedUserId });
    } else if (Date.now() > deadline) { refuse("bot creation did not produce one full-sign-on fakeclient"); return; }
  }
  if (owned && !ownedValid()) { refuse("owned connection became stale"); return; }
  if (ownedValid() && Player.fromSlot(owned!.slot)?.pawn?.isValid) record("bot-ready", { slot: owned!.slot, userId: ownedUserId });
}
function cleanup(): void {
  if (!attempted) return;
  const unchanged = baseline.every(client => client.isValid());
  const extras = Clients.all().filter(client => client.isValid() && !baseline.some(prior => prior.slot === client.slot && prior.isValid()));
  const safeOwned = unchanged && ownedValid() && extras.length === 1 && extras[0].slot === owned!.slot && extras[0].userId === ownedUserId;
  const safeAbsent = unchanged && !creating && !owned && extras.length === 0;
  if (safeOwned) owned!.kick("engine function fixture complete");
  if (safeOwned || safeAbsent) {
    // bot_add_ct adjusts quota itself. Restore only when doing so cannot select
    // an unidentified extra/baseline client for removal.
    let restored = true;
    for (const [name, value] of Object.entries(saved)) if (!Server.setCvar(name, value)) restored = false;
    saved = {}; attempted = false;
    record(restored ? "bot-cleaned" : "bot-cleanup-refused", { slot: owned?.slot ?? -1, userId: ownedUserId, kicked: safeOwned, settingsRestored: restored });
  } else record("bot-cleanup-refused", { reason: "ambiguous or stale identity; settings retained for operator recovery" });
  creating = false; owned = null; ownedUserId = -1; publish();
}
function observe(event: string, view: CanAcquireView): void {
  if (refused || !run || Number(Server.getCvar("s2_engine_fixture_active")) !== generation) return;
  const slot = Number(Server.getCvar("s2_engine_fixture_slot"));
  if (!ownedValid() || owned!.slot !== slot || view.player?.slot !== slot || view.player.userId !== ownedUserId) return;
  const id = Number(Server.getCvar("s2_engine_fixture_scope"));
  const operation = Number(Server.getCvar("s2_engine_fixture_operation"));
  if ((id !== 1 && id !== 2) || operation <= 0) { record("invalid-scope"); return; }
  record(event, { id, operation, slot, pawn: Number(Server.getCvar("s2_engine_fixture_pawn")), method: view.method, defIndex: view.defIndex, result: view.result, skipped: view.skipped });
}
export function OnPluginStart(): void {
  Server.registerCvar("s2_engine_witness_generation", { type: "int", default: 0 });
  witnessGeneration = Number(Server.getCvar("s2_engine_witness_generation")) + 1;
  Server.setCvar("s2_engine_witness_generation", String(witnessGeneration));
  for (const name of ["slot", "userid"]) Server.registerCvar(`s2_engine_owned_${name}`, { type: "int", default: -1 });
  publish();
  command("s2_engine_witness", cmd => {
    if (cmd.callerSlot >= 0) return;
    const requested = cmd.arg(1), next = Number(cmd.arg(2));
    if (!/^[A-Za-z0-9_-]+$/.test(requested) || cmd.arg(3) !== TOKEN || TOKEN === "unbuilt" || !Number.isSafeInteger(next)) return;
    const op = cmd.arg(0);
    if (op === "arm" && next > generation && next === Number(Server.getCvar("s2_engine_fixture_generation")) && (!run || run === requested)) {
      run = requested; generation = next; record("armed"); return;
    }
    if (requested !== run || next !== generation) return;
    if (op === "create" && !attempted) {
      attempted = true; baseline = Clients.all();
      if (baseline.some(client => !client.isValid() || client.userId < 0 || client.signonState !== 6)) { refuse("baseline identity incomplete"); return; }
      for (const name of ["bot_quota", "bot_quota_mode", "bot_join_after_player", "mp_limitteams"]) {
        saved[name] = Server.getCvar(name);
        if (saved[name] === "" || saved[name] === "<type>") { refuse("bot settings unavailable"); return; }
      }
      // A normal, settled quota prevents background quota fills from being
      // mistaken for this operation's single newly created client.
      if (saved.bot_quota_mode !== "normal" || Number(saved.bot_quota) !== baseline.filter(client => client.isBot && client.ip === "").length) { refuse("baseline bot quota is not settled normal mode"); return; }
      if (!Server.setCvar("bot_join_after_player", "0") || !Server.setCvar("mp_limitteams", "0")) { refuse("bot settings could not be applied"); return; }
      creating = true; deadline = Date.now() + 30000;
      Server.command("bot_add_ct"); record("bot-create-requested");
    } else if (op === "poll") poll();
    else if (op === "cleanup") cleanup();
  });
  items.onCanAcquire(view => observe("acquire-pre", view));
  items.onCanAcquirePost(view => observe("acquire-post", view));
  record("loaded");
}
export function OnPluginEnd(): void { cleanup(); record("unloaded"); }

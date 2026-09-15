// khook-acceptance — JS fixture for KHook suite A. NOT a shipped plugin.
//
// Uses only public APIs. Protocol: s2_khook_accept runtime, or prepare|collect|report|teardown <run_id>.
// `report` is read-only. Competing command() registration for the probe token is forbidden;
// only command.onClientCommand observes it. The probe observes the real engine fallback original.
//
// R6: SDKHooks entity/phase/reuse/map/reload plus voice/transmit/recipient collection.
// Human rows stay pending. Never invent a human pass.
import {
  Clients,
  Entity,
  Events,
  HookResult,
  SDKHook,
  SDKHookType,
  SDKUnhook,
  Server,
  Transmit,
  Voice,
  command,
  createEntity,
  hook,
  previous,
} from "@s2script/sdk";
import type { Client, EntityRef, HookResultValue } from "@s2script/sdk";
import { Player } from "@s2script/cs2";
import { KHOOK_FIXTURE_REVISION, KHOOK_FIXTURE_TOKEN } from "./build_identity";

const DEFAULT_TOKEN_CMD = "s2khook_cc_entry";
const CONTINUE_TOKEN = "s2khook-continue";
const HANDLED_TOKEN = "s2khook-handled";
const CTRL_MISSING = "s2khook-ctrl-missing";
const CTRL_FLIP_CONTINUE = "s2khook-ctrl-flip-continue";
const CTRL_FLIP_HANDLED = "s2khook-ctrl-flip-handled";
const MASK_EVENT = "player_changename";
const REUSE_MAX_ATTEMPTS = 64;

type Result = "pass" | "fail" | "pending";

interface Rec {
  case: string;
  subcheck: string;
  producer: "js";
  result: Result;
  expected: Record<string, unknown>;
  actual: Record<string, unknown>;
  evidence: string;
}

interface PhaseSnap {
  pre: number;
  post: number;
}

interface PersistState {
  runId: string;
  oldIndex: number;
  oldId: number;
  mapName: string;
  unloaded: boolean;
  instance: number;
  deliveredA: number;
  deliveredB: number;
  artifactIdentity: string;
  sourceRevision: string;
  records: Rec[];
}

let runId = "";
let runBound = false;
let artifactIdentity = "";
let frozenRevision = KHOOK_FIXTURE_REVISION;
let tokenCommand = DEFAULT_TOKEN_CMD;
const terminal = new Map<string, Rec>();
let collected = false;
let stored: Rec[] = [];
let hookEnabled = true;
let flipped = false;

let frames = 0;
let clientsConnected = 0;
let jsContinue = 0;
let jsHandled = 0;
let jsHandledUnsuppressed = 0;
let lastSlot = -1;
let lastUserId = -1;
let lastSteamId = "";

let instance = 1;
let r6Frames = 0;
let maskMode: "off" | "subset" | "all" = "off";

let entA: EntityRef | null = null;
let entB: EntityRef | null = null;
let phaseEnt: EntityRef | null = null;
let reuseEnt: EntityRef | null = null;
let transmitEnt: EntityRef | null = null;
let owned: EntityRef[] = [];
let spawnA = false;
let spawnB = false;
let deliveredA = 0;
let deliveredB = 0;
let filterHooked = false;

let phasePre = 0;
let phasePost = 0;
let phaseStage = 0;
let preHooked = false;
let postHooked = false;
let selfUnsubArmed = false;
let phaseSnaps: Record<string, PhaseSnap> = {};
let reuseHooked = false;

let identityIndex = -1;
let identityId = -1;
let identityPersisted = false;
let reuseAttempts = 0;
let reuseNewId = -1;
let reuseStale = 0;
let reuseDone = false;
let mapAtPrepare = "";
let mapEnded = false;
let preMapCallbacks = 0;
let postMapCallbacks = 0;
let sawReload = false;
let handoff: PersistState | undefined;
let reloadTarget: EntityRef | null = null;
let reloadPre = 0;
let reloadPost = 0;
let voiceApplied = false;
let voiceSpeaker = -1;
let voiceAllowed = -1;
let voiceDenied = -1;
let transmitPolicy = false;
let maskSubsetSeen = false;
let maskAllSeen = false;
let handledSetRecipients = false;
let clientActions: string[] = [];

function sourceRevision(): string { return frozenRevision; }

function fixtureIdentityReady(): boolean {
  return /^[a-f0-9]{40}$/.test(KHOOK_FIXTURE_REVISION) && /^[a-f0-9]{64}$/.test(KHOOK_FIXTURE_TOKEN);
}

function bindArtifact(value: string): boolean {
  if (!/^[a-f0-9]{64}$/.test(value) || (artifactIdentity && artifactIdentity !== value)) return false;
  artifactIdentity = value;
  setCvar("s2_khook_accept_artifact", value);
  return true;
}

function emit(rec: Rec): string {
  return JSON.stringify({
    schema: 1,
    suite: "A",
    run_id: runId || "",
    source_revision: sourceRevision(),
    artifact_identity: artifactIdentity,
    case: rec.case,
    subcheck: rec.subcheck,
    producer: "js",
    result: (!artifactIdentity || !fixtureIdentityReady()) && rec.result === "pass" ? "pending" : rec.result,
    expected: rec.expected,
    actual: rec.actual,
    evidence: rec.evidence,
  });
}

function push(
  cse: string,
  sub: string,
  result: Result,
  expected: Record<string, unknown>,
  actual: Record<string, unknown>,
  evidence: string,
): void {
  const key = cse + ":" + sub;
  const rec: Rec = { case: cse, subcheck: sub, producer: "js", result, expected, actual, evidence };
  const prior = terminal.get(key);
  if (result === "fail" || (!prior && result === "pass")) terminal.set(key, rec);
  stored.push(terminal.get(key) || rec);
}

function pending(cse: string, sub: string, expected: Record<string, unknown>, why: string): void {
  push(cse, sub, "pending", expected, {}, why);
}

function setCvar(name: string, value: string): void {
  Server.setCvar(name, value);
}

function persistOutsidePlugin(): void {
  setCvar("s2_khook_accept_old_index", String(identityIndex));
  setCvar("s2_khook_accept_old_id", String(identityId));
  setCvar("s2_khook_accept_map", mapAtPrepare);
  setCvar("s2_khook_accept_instance", String(instance));
  setCvar("s2_khook_accept_map_ended", mapEnded ? "1" : "0");
}

function cleanupOwned(): void {
  if (phaseEnt && phaseEnt.isValid()) {
    if (preHooked) SDKUnhook(phaseEnt, SDKHookType.Touch, onPhasePre);
    if (postHooked) SDKUnhook(phaseEnt, SDKHookType.TouchPost, onPhasePost);
  }
  if (entA && entA.isValid() && filterHooked) {
    SDKUnhook(entA, SDKHookType.Touch, onFilterTouch);
  }
  if (reuseEnt && reuseEnt.isValid() && reuseHooked) {
    SDKUnhook(reuseEnt, SDKHookType.Touch, onReuseTouch);
  }
  if (transmitEnt && transmitEnt.isValid()) {
    Transmit.reset(transmitEnt);
    SDKUnhook(transmitEnt, SDKHookType.SetTransmit, onSetTransmit);
  }
  Voice.resetAll();
  Transmit.resetAll();
  for (const e of owned) {
    if (e && e.isValid()) e.remove();
  }
  owned = [];
  entA = null;
  entB = null;
  phaseEnt = null;
  reuseEnt = null;
  transmitEnt = null;
  filterHooked = false;
  reuseHooked = false;
  preHooked = false;
  postHooked = false;
}

function remember(ent: EntityRef | null): EntityRef | null {
  if (ent) owned.push(ent);
  return ent;
}

function realClients(): Client[] {
  return Clients.all().filter((c) => c && c.isValid() && !c.isBot);
}

function onFilterTouch(entity: EntityRef, _other: EntityRef | null): void {
  if (mapEnded) {
    postMapCallbacks += 1;
    return;
  }
  preMapCallbacks += 1;
  if (entA && entity.index === entA.index && entity.id === entA.id) deliveredA += 1;
  if (entB && entity.index === entB.index && entity.id === entB.id) deliveredB += 1;
}

function onPhasePre(entity: EntityRef, _other: EntityRef | null): void {
  phasePre += 1;
  if (selfUnsubArmed) {
    SDKUnhook(entity, SDKHookType.Touch, onPhasePre);
    preHooked = false;
  }
}

function onPhasePost(_entity: EntityRef, _other: EntityRef | null): void {
  phasePost += 1;
}

function onSetTransmit(_entity: EntityRef, client: Client): HookResultValue | void {
  if (!client || !client.isValid()) return;
  const humans = realClients();
  if (humans.length >= 2 && client.slot === humans[1].slot) {
    return HookResult.Handled;
  }
  return HookResult.Continue;
}

function onReuseTouch(entity: EntityRef, _other: EntityRef | null): void {
  if (identityPersisted && identityId >= 0 && entity.index === identityIndex && entity.id !== identityId) {
    reuseStale += 1;
  }
}

function namedTrigger(name: string): EntityRef | null {
  for (const e of Entity.findByClass("trigger_push")) {
    if (e && e.isValid() && e.name === name) return e;
  }
  return null;
}

function adoptOrCreate(name: string): EntityRef | null {
  const found = namedTrigger(name);
  if (found) return found;
  return remember(createEntity("trigger_push", { targetname: name }));
}

function snapPhase(name: string): void {
  phaseSnaps[name] = { pre: phasePre, post: phasePost };
  phasePre = 0;
  phasePost = 0;
}

function writePhaseStage(): void {
  setCvar("s2_khook_accept_phase_stage", String(phaseStage));
}

function advancePhase(): void {
  if (!phaseEnt || !phaseEnt.isValid() || phaseStage < 0 || phaseStage > 5) return;
  let ack: { run_id: string; entity_index: number; stage: number; original: number };
  try { ack = JSON.parse(Server.getCvar("s2_khook_accept_phase_ack")); } catch { return; }
  if (ack.run_id !== runId || ack.entity_index !== phaseEnt.index || ack.stage !== phaseStage) return;
  // A native acknowledgement is published only after the real virtual invocation
  // returned. Zero callbacks after an acknowledged call is evidence, not waiting.
  const names = ["subscribe_pre_post", "remove_pre", "remove_post", "self_unsubscribe", "self_unsubscribe_second", "final_unsubscribe"];
  snapPhase(names[phaseStage]);
  if (phaseStage === 0) {
    SDKUnhook(phaseEnt, SDKHookType.Touch, onPhasePre); preHooked = false;
  } else if (phaseStage === 1) {
    preHooked = SDKHook(phaseEnt, SDKHookType.Touch, onPhasePre);
    SDKUnhook(phaseEnt, SDKHookType.TouchPost, onPhasePost); postHooked = false;
  } else if (phaseStage === 2) {
    postHooked = SDKHook(phaseEnt, SDKHookType.TouchPost, onPhasePost);
    selfUnsubArmed = true;
  } else if (phaseStage === 3) {
    selfUnsubArmed = false;
  } else if (phaseStage === 4) {
    if (preHooked) SDKUnhook(phaseEnt, SDKHookType.Touch, onPhasePre);
    if (postHooked) SDKUnhook(phaseEnt, SDKHookType.TouchPost, onPhasePost);
    preHooked = postHooked = false;
  }
  phaseStage += 1;
  writePhaseStage();
}

function spawnPair(): void {
  entA = adoptOrCreate("s2khook_ent_a");
  entB = adoptOrCreate("s2khook_ent_b");
  phaseEnt = adoptOrCreate("s2khook_phase");
  reuseEnt = adoptOrCreate("s2khook_reuse");
  spawnA = !!(entA && entA.isValid());
  spawnB = !!(entB && entB.isValid());
  if (spawnA && entA) {
    filterHooked = SDKHook(entA, SDKHookType.Touch, onFilterTouch);
  }
  if (reuseEnt && reuseEnt.isValid()) {
    identityIndex = reuseEnt.index;
    identityId = reuseEnt.id;
    identityPersisted = true;
    reuseHooked = SDKHook(reuseEnt, SDKHookType.Touch, onReuseTouch);
  }
  if (phaseEnt && phaseEnt.isValid()) {
    preHooked = SDKHook(phaseEnt, SDKHookType.Touch, onPhasePre);
    postHooked = SDKHook(phaseEnt, SDKHookType.TouchPost, onPhasePost);
    phaseStage = preHooked && postHooked ? 0 : -1;
  } else {
    phaseStage = -1;
  }
  writePhaseStage();
  setCvar("s2_khook_accept_ent_a", spawnA && entA ? String(entA.index) : "-1");
  setCvar("s2_khook_accept_ent_b", spawnB && entB ? String(entB.index) : "-1");
  setCvar("s2_khook_accept_phase_ent", phaseEnt && phaseEnt.isValid() ? String(phaseEnt.index) : "-1");
  setCvar("s2_khook_accept_reuse_ent", reuseEnt && reuseEnt.isValid() ? String(reuseEnt.index) : "-1");
}

function trySlotReuse(): void {
  if (!identityPersisted || identityIndex < 0) return;
  const oldId = identityId;
  if (reuseEnt && reuseEnt.isValid()) reuseEnt.remove();
  for (let i = 0; i < REUSE_MAX_ATTEMPTS; i++) {
    reuseAttempts = i + 1;
    const n = createEntity("trigger_push", { targetname: "s2khook_reuse_" + i });
    if (!n) continue;
    remember(n);
    if (n.index === identityIndex && n.id !== oldId) {
      reuseNewId = n.id;
      reuseDone = true;
      setCvar("s2_khook_accept_reuse_new", String(n.index));
      return;
    }
    n.remove();
  }
}

function applyVoice(): void {
  const humans = realClients();
  Voice.resetAll();
  voiceApplied = false;
  if (humans.length < 3) {
    clientActions.push(
      "voice_recall: need three clients (speaker, allowed-listener, denied-listener). " +
        "Speaker talks while policy allows slot1 and denies slot2; then unmute (Voice.reset) and talk again.",
    );
    return;
  }
  voiceSpeaker = humans[0].slot;
  voiceAllowed = humans[1].slot;
  voiceDenied = humans[2].slot;
  voiceApplied = Voice.setAudibleTo(voiceSpeaker, [voiceAllowed]);
  setCvar("s2_khook_accept_voice_phase", "policy");
  setCvar("s2_khook_accept_voice_speaker", String(voiceSpeaker));
  setCvar("s2_khook_accept_voice_allowed", String(voiceAllowed));
  setCvar("s2_khook_accept_voice_denied", String(voiceDenied));
  clientActions.push(
    "voice_recall actors speaker=" +
      voiceSpeaker +
      " allowed=" +
      voiceAllowed +
      " denied=" +
      voiceDenied +
      ": speaker talks now (allowed hears / denied silent). After collect of denied phase, operator run s2_khook_accept restore RUN_ID; then collect the unmuted observation.",
  );
}

function applyTransmit(): void {
  const humans = realClients();
  if (humans.length < 2) {
    clientActions.push(
      "check_transmit: need two clients in PVS of a networked visible entity (not logic_relay). " +
        "Confirm client A sees the point_worldtext, client B does not, then restore.",
    );
    return;
  }
  let origin: { x: number; y: number; z: number } | null = null;
  for (const p of Player.allConnected()) {
    if (p.steamId === "0") continue;
    const o = p.pawn ? p.pawn.origin : null;
    if (o) {
      origin = o;
      break;
    }
  }
  const e = createEntity("point_worldtext", {
    message: "S2KHOOK TRANSMIT",
    enabled: true,
    fullbright: true,
    font_size: 80,
    world_units_per_pixel: 0.25,
    color: "0 255 0",
  });
  transmitEnt = remember(e);
  if (!transmitEnt) {
    clientActions.push("check_transmit: createEntity(point_worldtext) failed");
    return;
  }
  if (origin) {
    transmitEnt.teleport([origin.x, origin.y, origin.z + 72], [0, 0, 0], null);
  }
  const slotA = humans[0].slot;
  transmitPolicy = Transmit.setVisibleTo(transmitEnt, [slotA]);
  setCvar("s2_khook_accept_tx_phase", "policy");
  SDKHook(transmitEnt, SDKHookType.SetTransmit, onSetTransmit);
  setCvar("s2_khook_accept_tx_ent", String(transmitEnt.index));
  setCvar("s2_khook_accept_tx_a", String(slotA));
  setCvar("s2_khook_accept_tx_b", String(humans[1].slot));
  clientActions.push(
    "check_transmit actors client-a=" +
      slotA +
      " client-b=" +
      humans[1].slot +
      " entity=" +
      transmitEnt.index +
      ": both stand in PVS of the green S2KHOOK TRANSMIT text. A should see it, B should not. Then run s2_khook_accept restore RUN_ID and capture both clients seeing the same text.",
  );
}

function restoreTransmitVisibility(): void {
  if (transmitEnt && transmitEnt.isValid()) {
    Transmit.reset(transmitEnt);
    SDKUnhook(transmitEnt, SDKHookType.SetTransmit, onSetTransmit);
  }
}

function applyMaskSubset(): void {
  const humans = realClients();
  if (humans.length < 2) {
    clientActions.push(
      "fire_event_handled_recipient_mask: need at least two real clients. " +
        "Observe player_changename only on the subset slot, then the all-suppressed fire.",
    );
    maskMode = "off";
    return;
  }
  maskMode = "subset";
  setCvar("s2_khook_accept_mask_a", String(humans[0].slot));
  setCvar("s2_khook_accept_mask_b", String(humans[1].slot));
  clientActions.push(
    "fire_event_handled_recipient_mask subset slot=" +
      humans[0].slot +
      " excluded slot=" +
      humans[1].slot +
      ": watch client console/listener for player_changename. Subset receives, excluded does not. Then all-suppressed fire.",
  );
}

function pushFilter(): void {
  const aExp = { count: 1 };
  const bExp = { count: 0 };
  if (!spawnA || !spawnB) {
    pending(
      "sdkhooks_one_of_two_entities",
      "js_hook_a_delivered",
      aExp,
      "require successful spawn of A and B before judging filtering",
    );
    pending(
      "sdkhooks_one_of_two_entities",
      "js_hook_b_filtered",
      bExp,
      "require successful spawn of A and B before judging filtering",
    );
    return;
  }
  if (deliveredA === 0 && deliveredB === 0) {
    if (!filterHooked) {
      push(
        "sdkhooks_one_of_two_entities",
        "js_hook_a_delivered",
        "fail",
        aExp,
        { count: 0, hooked: false },
        "registration failure: SDKHook Touch on A returned false",
      );
      push(
        "sdkhooks_one_of_two_entities",
        "js_hook_b_filtered",
        "fail",
        bExp,
        { count: 0, hooked: false },
        "registration failure: neither entity delivered",
      );
      return;
    }
    pending(
      "sdkhooks_one_of_two_entities",
      "js_hook_a_delivered",
      aExp,
      "need native probe Touch invoke on entity A (actual SDKHooks adapter; Dummy Virtuals are not a substitute)",
    );
    pending(
      "sdkhooks_one_of_two_entities",
      "js_hook_b_filtered",
      bExp,
      "need native probe Touch invoke on entity B with only A hooked",
    );
    return;
  }
  if (deliveredA === 1) {
    push("sdkhooks_one_of_two_entities", "js_hook_a_delivered", "pass", aExp, aExp, "A delivered once");
  } else {
    push(
      "sdkhooks_one_of_two_entities",
      "js_hook_a_delivered",
      "fail",
      aExp,
      { count: deliveredA },
      "A delivery count mismatch",
    );
  }
  if (deliveredB === 0) {
    push("sdkhooks_one_of_two_entities", "js_hook_b_filtered", "pass", bExp, bExp, "B filtered");
  } else {
    push(
      "sdkhooks_one_of_two_entities",
      "js_hook_b_filtered",
      "fail",
      bExp,
      { count: deliveredB },
      "filtering failure: B delivered while only A was hooked",
    );
  }
}

function pushPhase(): void {
  const checks: Array<[string, string, Record<string, number>, Record<string, number> | undefined]> = [
    ["subscribe_pre_post", "js_phase_subscribe_pre_post", { pre: 1, post: 1 }, phaseSnaps.subscribe_pre_post && { ...phaseSnaps.subscribe_pre_post }],
    ["remove_pre", "js_phase_remove_pre", { pre: 0, post: 1 }, phaseSnaps.remove_pre && { ...phaseSnaps.remove_pre }],
    ["remove_post", "js_phase_remove_post", { pre: 1, post: 0 }, phaseSnaps.remove_post && { ...phaseSnaps.remove_post }],
    ["final_unsubscribe", "js_phase_final_unsubscribe", { pre: 0, post: 0 }, phaseSnaps.final_unsubscribe && { ...phaseSnaps.final_unsubscribe }],
  ];
  const first = phaseSnaps.self_unsubscribe, second = phaseSnaps.self_unsubscribe_second;
  checks.push(["self_unsubscribe", "js_phase_self_unsubscribe",
    { first_pre: 1, first_post: 1, second_pre: 0, second_post: 1 },
    first && second ? { first_pre: first.pre, first_post: first.post, second_pre: second.pre, second_post: second.post } : undefined]);
  for (const [name, sub, expected, actual] of checks) {
    if (!actual) {
      pending("sdkhooks_phase_removal", sub, expected, "waiting for native Touch acknowledgement for " + name);
    } else {
      const ok = Object.keys(expected).every(k => actual[k] === expected[k]);
      push("sdkhooks_phase_removal", sub, ok ? "pass" : "fail", expected, actual,
        "SDKHooks callback snapshot after native Touch " + name);
    }
  }
}

function pushReuseMapReload(): void {
  const persExp = { persisted: true, index: identityIndex, id: identityId };
  if (identityPersisted && identityIndex >= 0) {
    push(
      "entity_slot_reuse_map_teardown",
      "js_identity_persisted",
      "pass",
      persExp,
      persExp,
      "index+id stored outside the plugin (cvar s2_khook_accept_old_*)",
    );
  } else {
    pending(
      "entity_slot_reuse_map_teardown",
      "js_identity_persisted",
      { persisted: true },
      "need live engine entity spawn to persist index+id",
    );
  }
  const staleExp = { stale: false, reused: true };
  if (reuseDone && reuseStale === 0) {
    push("entity_slot_reuse_map_teardown", "js_slot_reuse_no_stale", "pass", staleExp, staleExp, "new occupant did not fire old subscription");
  } else if (reuseStale > 0) {
    push(
      "entity_slot_reuse_map_teardown",
      "js_slot_reuse_no_stale",
      "fail",
      staleExp,
      { stale: true, reused: reuseDone, deliveries: reuseStale },
      "old subscription delivered for the new occupant",
    );
  } else {
    pending(
      "entity_slot_reuse_map_teardown",
      "js_slot_reuse_no_stale",
      staleExp,
      "slot reuse not achieved within bounded attempts; not a false pass (attempts=" +
        reuseAttempts +
        "/" +
        REUSE_MAX_ATTEMPTS +
        ")",
    );
  }
  const clrExp = { cleared: true };
  const postMapInvoked = Server.getCvar("s2_khook_accept_post_map_invoke") === "1";
  if (!mapEnded) {
    pending(
      "entity_slot_reuse_map_teardown",
      "js_map_teardown_clears",
      clrExp,
      "need operator changelevel while this run stays prepared; then collect again",
    );
  } else if (!postMapInvoked) {
    pending(
      "entity_slot_reuse_map_teardown",
      "js_map_teardown_clears",
      clrExp,
      "need post-map Touch invoke via live EntByIndex of a remaining trigger_push " +
        "(not whatever now occupies the saved index); if no live trigger remains, pending",
    );
  } else if (preMapCallbacks > 0 && postMapCallbacks === preMapCallbacks) {
    push(
      "entity_slot_reuse_map_teardown",
      "js_map_teardown_clears",
      "fail",
      clrExp,
      { cleared: false, pre_map_count: preMapCallbacks, post_map_count: postMapCallbacks },
      "stale post-map record: pre-map counter reused after map teardown",
    );
  } else if (postMapCallbacks === 0) {
    push("entity_slot_reuse_map_teardown", "js_map_teardown_clears", "pass", clrExp, clrExp, "post-map callbacks cleared after EntByIndex invoke");
  } else {
    push(
      "entity_slot_reuse_map_teardown",
      "js_map_teardown_clears",
      "fail",
      clrExp,
      { cleared: false, post_map_count: postMapCallbacks },
      "post-map callback still firing on old identity",
    );
  }
  pushScriptReload();
}

// Deliberately leave these two subscriptions to owner-ledger teardown. A stale
// outgoing closure appends its old generation during the native after-invoke.
function reloadTrace(phase: "pre" | "post"): void {
  if (Server.getCvar("s2_khook_accept_reload_active") !== runId) return;
  setCvar("s2_khook_accept_reload_trace", Server.getCvar("s2_khook_accept_reload_trace") + instance + ":" + phase + ",");
  if (phase === "pre") reloadPre += 1; else reloadPost += 1;
}
function onReloadPre(): void { reloadTrace("pre"); }
function onReloadPost(): void { reloadTrace("post"); }

function armScriptReload(replacement: boolean): boolean {
  if (reloadTarget) return true;
  const target = Number(Server.getCvar("s2_khook_accept_reload_target"));
  const entity = Entity.findByClass("trigger_push").find(e => e.index === target && e.isValid());
  if (!entity) return false;
  if (!SDKHook(entity, SDKHookType.Touch, onReloadPre)) return false;
  if (!SDKHook(entity, SDKHookType.TouchPost, onReloadPost)) {
    SDKUnhook(entity, SDKHookType.Touch, onReloadPre);
    return false;
  }
  if (!replacement) {
    // createEntity is game-world owned, not auto-removed by the plugin ledger.
    // This marker checks explicit OnPluginEnd cleanup; only the subscriptions
    // above deliberately rely on owner-ledger teardown.
    const marker = remember(createEntity("logic_relay", { targetname: "s2khook_reload_owned_" + instance }));
    if (!marker) {
      SDKUnhook(entity, SDKHookType.Touch, onReloadPre);
      SDKUnhook(entity, SDKHookType.TouchPost, onReloadPost);
      return false;
    }
    setCvar("s2_khook_accept_reload_marker", String(marker.index));
    setCvar("s2_khook_accept_reload_unloaded", "0");
  }
  reloadTarget = entity;
  setCvar("s2_khook_accept_reload_ready", String(instance));
  return true;
}

function pushScriptReload(): void {
  const expected = { fresh: true, pre: 1, post: 1, original: 1 };
  let ack: { run_id: string; artifact_identity: string; target_index: number; generation: number; stage: string; original: number };
  try { ack = JSON.parse(Server.getCvar("s2_khook_accept_reload_ack")); }
  catch { pending("entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload", expected, "await resident probe script reload invocation"); return; }
  if (!sawReload || !reloadTarget || ack.stage !== "after" || ack.run_id !== runId ||
      ack.artifact_identity !== artifactIdentity || ack.generation !== instance || ack.target_index !== reloadTarget.index) {
    pending("entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload", expected, "await matching replacement invocation"); return;
  }
  const actual = { fresh: instance > (handoff?.instance || instance), pre: reloadPre, post: reloadPost, original: ack.original };
  push("entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload",
    actual.fresh && reloadPre === 1 && reloadPost === 1 && ack.original === 1 ? "pass" : "fail", expected, actual,
    "generation-tagged callbacks on the same resident native target after script reload");
}

function pushVoiceTransmitMask(): void {
  const vExp = { applied: true, allowed_slot: voiceAllowed, denied_slot: voiceDenied };
  if (voiceApplied && voiceSpeaker >= 0) {
    push("voice_recall", "js_voice_policy_applied", "pass", vExp, vExp, "Voice.setAudibleTo applied; human audio is a separate subcheck");
  } else {
    pending(
      "voice_recall",
      "js_voice_policy_applied",
      { applied: true },
      "need three real clients (speaker, allowed-listener, denied-listener)",
    );
  }
  const tExp = { a: true, b: false };
  if (transmitPolicy && transmitEnt && transmitEnt.isValid()) {
    push("check_transmit", "js_visibility_policy", "pass", tExp, tExp, "Transmit.setVisibleTo A-only + SetTransmit deny B");
  } else {
    pending(
      "check_transmit",
      "js_visibility_policy",
      tExp,
      "need a networked visible entity in both clients' PVS (not logic_relay)",
    );
  }
  const mExp = { handled: true, subset: true, all_suppressed: true };
  if (handledSetRecipients && maskSubsetSeen && maskAllSeen) {
    push("fire_event_handled_recipient_mask", "js_handled_set_recipients", "pass", mExp, mExp, "Handled+setRecipients subset then all-suppressed");
  } else if (handledSetRecipients && maskSubsetSeen) {
    pending(
      "fire_event_handled_recipient_mask",
      "js_handled_set_recipients",
      mExp,
      "subset observed; need a second fire with empty recipients (all-suppressed). Sending to every human is not a mask test",
    );
  } else {
    pending(
      "fire_event_handled_recipient_mask",
      "js_handled_set_recipients",
      mExp,
      "need at least two real clients and a native-fired observable player_changename to a strict subset, then suppress for all",
    );
  }
}

function pushR6(): void {
  pushFilter();
  pushPhase();
  pushReuseMapReload();
  pushVoiceTransmitMask();
}

function pushInvalid(requested: string): void {
  const expected = { run_id: runId };
  const actual = { run_id: requested, error: "unknown_or_mismatched_run" };
  const evd = "unknown or mismatched run_id; never reuse a prior run";
  const ownedSubs: Array<[string, string]> = [
    ["frame_client_command_hooks", "js_gameframe_delivery"],
    ["frame_client_command_hooks", "js_client_connected"],
    ["frame_client_command_hooks", "js_client_identity_join"],
    ["frame_client_command_hooks", "js_command_continue_delivery"],
    ["frame_client_command_hooks", "js_command_handled_delivery"],
    ["fire_event_no_suppression", "js_no_handled_on_unsuppressed_event"],
    ["fire_event_handled_recipient_mask", "js_handled_set_recipients"],
    ["voice_recall", "js_voice_policy_applied"],
    ["sdkhooks_one_of_two_entities", "js_hook_a_delivered"],
    ["sdkhooks_one_of_two_entities", "js_hook_b_filtered"],
    ["sdkhooks_phase_removal", "js_phase_subscribe_pre_post"],
    ["sdkhooks_phase_removal", "js_phase_remove_pre"],
    ["sdkhooks_phase_removal", "js_phase_remove_post"],
    ["sdkhooks_phase_removal", "js_phase_self_unsubscribe"],
    ["sdkhooks_phase_removal", "js_phase_final_unsubscribe"],
    ["entity_slot_reuse_map_teardown", "js_identity_persisted"],
    ["entity_slot_reuse_map_teardown", "js_slot_reuse_no_stale"],
    ["entity_slot_reuse_map_teardown", "js_map_teardown_clears"],
    ["entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload"],
    ["check_transmit", "js_visibility_policy"],
  ];
  for (const [cse, sub] of ownedSubs) {
    push(cse, sub, "fail", expected, actual, evd);
  }
}

function pushOwnedPendingUncollected(): void {
  stored = [];
  pending("frame_client_command_hooks", "js_gameframe_delivery", { observed: true }, "report before collect");
  pending("frame_client_command_hooks", "js_client_connected", { observed: true }, "report before collect");
  pending(
    "frame_client_command_hooks",
    "js_client_identity_join",
    { slot: 0, steamId: "", run_id: runId },
    "report before collect",
  );
  pending("frame_client_command_hooks", "js_command_continue_delivery", { js: 1 }, "report before collect");
  pending("frame_client_command_hooks", "js_command_handled_delivery", { js: 1 }, "report before collect");
  pending("fire_event_no_suppression", "js_no_handled_on_unsuppressed_event", { handled: 0 }, "report before collect");
  pending(
    "fire_event_handled_recipient_mask",
    "js_handled_set_recipients",
    { handled: true, subset: true, all_suppressed: true },
    "report before collect",
  );
  pending("voice_recall", "js_voice_policy_applied", { applied: true }, "report before collect");
  pending("sdkhooks_one_of_two_entities", "js_hook_a_delivered", { count: 1 }, "report before collect");
  pending("sdkhooks_one_of_two_entities", "js_hook_b_filtered", { count: 0 }, "report before collect");
  pending("sdkhooks_phase_removal", "js_phase_subscribe_pre_post", { pre: 1, post: 1 }, "report before collect");
  pending("sdkhooks_phase_removal", "js_phase_remove_pre", { pre: 0, post: 1 }, "report before collect");
  pending("sdkhooks_phase_removal", "js_phase_remove_post", { pre: 1, post: 0 }, "report before collect");
  pending("sdkhooks_phase_removal", "js_phase_self_unsubscribe", { first_pre: 1, first_post: 1, second_pre: 0, second_post: 1 }, "report before collect");
  pending("sdkhooks_phase_removal", "js_phase_final_unsubscribe", { pre: 0, post: 0 }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_identity_persisted", { persisted: true }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_slot_reuse_no_stale", { stale: false }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_map_teardown_clears", { cleared: true }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload", { fresh: true, pre: 1, post: 1, original: 1 }, "report before collect");
  pending("check_transmit", "js_visibility_policy", { a: true, b: false }, "report before collect");
}

function collectJs(): void {
  if (sawReload) {
    stored = [];
    pushOwnedPendingUncollected();
    stored = stored.filter(r => r.subcheck !== "js_fresh_subscription_after_reload");
    pushScriptReload();
    collected = true;
    return;
  }

  // Clients can join after prepare. Complete the existing run's waiting setup
  // without re-preparing and erasing its earlier connection/entity evidence.
  if (runBound && Server.getCvar("s2_khook_accept_voice_phase") !== "restored" && !voiceApplied && realClients().length >= 3) applyVoice();
  if (runBound && Server.getCvar("s2_khook_accept_tx_phase") !== "restored" && !transmitEnt && realClients().length >= 2) applyTransmit();
  if (runBound && !maskSubsetSeen && maskMode === "off" && Server.getCvar("s2_khook_accept_tx_phase") !== "restored" && realClients().length >= 2) {
    applyMaskSubset(); setCvar("s2_khook_accept_mask_mode", maskMode);
  }
  stored = [];
  if (frames > 0) {
    push("frame_client_command_hooks", "js_gameframe_delivery", "pass", { observed: true }, { observed: true }, "OnGameFrame");
  } else {
    pending("frame_client_command_hooks", "js_gameframe_delivery", { observed: true }, "need live frames");
  }
  if (clientsConnected > 0) {
    push("frame_client_command_hooks", "js_client_connected", "pass", { observed: true }, { observed: true }, "OnClientConnected");
  } else {
    pending("frame_client_command_hooks", "js_client_connected", { observed: true }, "need a real client connect");
  }
  if (lastSlot >= 0) {
    const ident = { slot: lastSlot, steamId: lastSteamId, userId: lastUserId, run_id: runId };
    push("frame_client_command_hooks", "js_client_identity_join", "pass", ident, ident, "joined by slot+steamId+run_id");
  } else {
    pending(
      "frame_client_command_hooks",
      "js_client_identity_join",
      { slot: 0, steamId: "", run_id: runId },
      "need a real client identity to join with native",
    );
  }
  const contExp = { js: 1 };
  if (jsContinue > 0) {
    push("frame_client_command_hooks", "js_command_continue_delivery", jsContinue === 1 ? "pass" : "fail", contExp, { js: jsContinue }, "real ClientCommand continue token");
  } else {
    pending(
      "frame_client_command_hooks",
      "js_command_continue_delivery",
      contExp,
      "need a real client to issue s2khook_cc_entry RUN_ID s2khook-continue",
    );
  }
  const handExp = { js: 1 };
  if (jsHandled > 0) {
    push("frame_client_command_hooks", "js_command_handled_delivery", jsHandled === 1 ? "pass" : "fail", handExp, { js: jsHandled }, "real ClientCommand handled token");
  } else {
    pending(
      "frame_client_command_hooks",
      "js_command_handled_delivery",
      handExp,
      "need a real client to issue s2khook_cc_entry RUN_ID s2khook-handled",
    );
  }
  const noHandled = { handled: 0 };
  if (jsHandledUnsuppressed === 0) {
    push(
      "fire_event_no_suppression",
      "js_no_handled_on_unsuppressed_event",
      "pass",
      noHandled,
      noHandled,
      "JS does not Handled-hook player_activate",
    );
  } else {
    push(
      "fire_event_no_suppression",
      "js_no_handled_on_unsuppressed_event",
      "fail",
      noHandled,
      { handled: jsHandledUnsuppressed },
      "JS must not Handled the no-suppression event",
    );
  }
  if (maskMode === "subset" && maskSubsetSeen && !maskAllSeen) {
    maskMode = "all";
  }
  pushR6();
  collected = true;
}

function resetR6Counters(): void {
  cleanupOwned();
  spawnA = false;
  spawnB = false;
  deliveredA = 0;
  deliveredB = 0;
  phasePre = 0;
  phasePost = 0;
  phaseStage = 0;
  selfUnsubArmed = false;
  phaseSnaps = {};
  identityIndex = -1;
  identityId = -1;
  identityPersisted = false;
  reuseAttempts = 0;
  reuseNewId = -1;
  reuseStale = 0;
  reuseDone = false;
  reuseHooked = false;
  mapAtPrepare = Server.mapName || "";
  mapEnded = false;
  preMapCallbacks = 0;
  postMapCallbacks = 0;
  voiceApplied = false;
  voiceSpeaker = -1;
  voiceAllowed = -1;
  voiceDenied = -1;
  transmitPolicy = false;
  maskSubsetSeen = false;
  maskAllSeen = false;
  handledSetRecipients = false;
  maskMode = "off";
  r6Frames = 0;
  clientActions = [];
  setCvar("s2_khook_accept_voice_phase", "off");
  setCvar("s2_khook_accept_tx_phase", "off");
  setCvar("s2_khook_accept_tx_ent", "-1");
  setCvar("s2_khook_accept_tx_a", "-1");
  setCvar("s2_khook_accept_tx_b", "-1");
}

function prepareR6(): void {
  setCvar("s2_khook_accept_phase_ack", "");
  resetR6Counters();
  spawnPair();
  trySlotReuse();
  applyVoice();
  applyTransmit();
  applyMaskSubset();
  persistOutsidePlugin();
  setCvar("s2_khook_accept_unloaded", "0");
  setCvar("s2_khook_accept_post_map_invoke", "0");
  for (const line of clientActions) {
    console.log("[khook-accept] NEED_CLIENTS: " + line);
  }
}

export function OnPluginStart(): void {
  const prev = previous() as PersistState | undefined;
  handoff = prev;
  if (prev && prev.runId) {
    sawReload = true;
    instance = (prev.instance || 0) + 1;
    identityIndex = prev.oldIndex;
    identityId = prev.oldId;
    mapAtPrepare = prev.mapName;
    identityPersisted = prev.oldIndex >= 0;
    deliveredA = Number(prev.deliveredA) || 0;
    deliveredB = Number(prev.deliveredB) || 0;
  }
  console.log("[khook-accept] loaded (test fixture, not shipped) instance=" + instance + " reload=" + sawReload);
  for (const name of ["target", "marker", "ready", "unloaded"]) {
    Server.registerCvar("s2_khook_accept_reload_" + name, { type: "int", default: -1, help: "resident native script reload witness" });
  }
  for (const name of ["trace", "active", "ack"]) {
    Server.registerCvar("s2_khook_accept_reload_" + name, { type: "string", default: "", help: "script reload invocation witness" });
  }
  Server.registerCvar("s2_khook_accept_live", { type: "int", default: 0, help: "live JS generation; zero after OnPluginEnd" });
  instance = Math.max(instance, Number(Server.getCvar("s2_khook_accept_instance")) + 1);
  Server.setCvar("s2_khook_accept_live", String(instance));
  Server.registerCvar("s2_khook_accept_artifact", { type: "string", default: "", help: "frozen artifact receipt digest" });
  Server.registerCvar("s2_khook_accept_phase_ack", { type: "string", default: "", help: "native Touch invocation acknowledgement" });
  Server.registerCvar("s2_khook_accept_voice_phase", { type: "string", default: "off", help: "policy|restored|off" });
  Server.registerCvar("s2_khook_accept_tx_phase", { type: "string", default: "off", help: "policy|restored|off" });
  Server.registerCvar("s2_khook_accept_command", { type: "string", default: DEFAULT_TOKEN_CMD, help: "ClientCommand fallback target; configure before JS load" });
  tokenCommand = Server.getCvar("s2_khook_accept_command") || DEFAULT_TOKEN_CMD;
  Server.registerCvar("s2_khook_accept_run", { type: "string", default: "", help: "khook-accept bound run_id" });
  Server.registerCvar("s2_khook_accept_old_index", { type: "int", default: -1, help: "persisted entity index" });
  Server.registerCvar("s2_khook_accept_old_id", { type: "int", default: -1, help: "persisted host id" });
  Server.registerCvar("s2_khook_accept_map", { type: "string", default: "", help: "map at prepare" });
  Server.registerCvar("s2_khook_accept_instance", { type: "int", default: 0, help: "js instance generation" });
  Server.registerCvar("s2_khook_accept_unloaded", { type: "int", default: 0, help: "set on unload for reload proof" });
  Server.registerCvar("s2_khook_accept_ent_a", { type: "int", default: -1, help: "entity A index" });
  Server.registerCvar("s2_khook_accept_ent_b", { type: "int", default: -1, help: "entity B index" });
  Server.registerCvar("s2_khook_accept_phase_ent", { type: "int", default: -1, help: "phase entity index" });
  Server.registerCvar("s2_khook_accept_tx_ent", { type: "int", default: -1, help: "transmit entity index" });
  Server.registerCvar("s2_khook_accept_tx_a", { type: "int", default: -1, help: "transmit allowed client slot" });
  Server.registerCvar("s2_khook_accept_tx_b", { type: "int", default: -1, help: "transmit denied client slot" });
  Server.registerCvar("s2_khook_accept_phase_stage", { type: "int", default: -1, help: "JS phase machine stage" });
  Server.registerCvar("s2_khook_accept_post_map_invoke", { type: "int", default: 0, help: "1 after post-map EntByIndex Touch" });
  Server.registerCvar("s2_khook_accept_mask_a", { type: "int", default: -1, help: "subset recipient slot" });
  Server.registerCvar("s2_khook_accept_mask_b", { type: "int", default: -1, help: "excluded recipient slot" });
  Server.registerCvar("s2_khook_accept_mask_mode", { type: "string", default: "off", help: "subset|all|off" });
  Server.registerCvar("s2_khook_accept_map_ended", { type: "int", default: 0, help: "1 after OnMapEnd for this run" });
  Server.registerCvar("s2_khook_accept_reuse_ent", { type: "int", default: -1, help: "reuse candidate index" });
  Server.registerCvar("s2_khook_accept_reuse_new", { type: "int", default: -1, help: "reused occupant index" });
  Server.registerCvar("s2_khook_accept_voice_speaker", { type: "int", default: -1, help: "voice speaker slot" });
  Server.registerCvar("s2_khook_accept_voice_allowed", { type: "int", default: -1, help: "voice allowed listener slot" });
  Server.registerCvar("s2_khook_accept_voice_denied", { type: "int", default: -1, help: "voice denied listener slot" });

  // Reload must explicitly resume the controller's run. An old cvar or a
  // previous() blob alone cannot silently adopt a run after native restart.

  hook.onPre(MASK_EVENT, (_ev) => {
    if (!runBound || maskMode === "off") return;
    const humans = realClients();
    if (humans.length < 2) return;
    handledSetRecipients = true;
    if (maskMode === "all") {
      Events.setRecipients([]);
      maskAllSeen = true;
      return HookResult.Handled;
    }
    Events.setRecipients([humans[0].slot]);
    maskSubsetSeen = true;
    return HookResult.Handled;
  });

  command.onClientCommand(tokenCommand, (slot, argString) => {
    const args = (argString || "").trim().split(/\s+/);
    if (!runBound || args[0] !== runId || !realClients().some(c => c.slot === slot)) return HookResult.Continue;
    const tok = args[1] || "";
    if (!hookEnabled || tok === CTRL_MISSING) return HookResult.Continue;
    if (tok === CTRL_FLIP_CONTINUE) return HookResult.Handled;
    if (tok === CTRL_FLIP_HANDLED) return HookResult.Continue;
    if (tok === CONTINUE_TOKEN) {
      jsContinue += 1;
      return flipped ? HookResult.Handled : HookResult.Continue;
    }
    if (tok === HANDLED_TOKEN) {
      jsHandled += 1;
      return flipped ? HookResult.Continue : HookResult.Handled;
    }
    return HookResult.Continue;
  });

  command.server("s2_khook_accept", (cmd) => {
    const sub = (cmd.arg(0) || "").toLowerCase();
    if (sub === "runtime") {
      cmd.reply(JSON.stringify({
        schema: 1,
        kind: "khook-fixture-runtime",
        result: fixtureIdentityReady() ? "ready" : "pending",
        fixture_revision: KHOOK_FIXTURE_REVISION,
        fixture_token: KHOOK_FIXTURE_TOKEN,
        generation: Number.isInteger(instance) && instance > 0 ? instance : 1,
      }));
      return HookResult.Handled;
    }
    const id = cmd.arg(1) || "";
    const digest = cmd.arg(2) || "";
    if (sub === "bind") {
      if (!runBound || id !== runId || !bindArtifact(digest)) cmd.reply("[khook-accept] invalid binding");
      else cmd.reply("[khook-accept] bound artifact=" + artifactIdentity);
      return HookResult.Handled;
    }
    if (sub === "resume" && runBound) {
      if (id !== runId || !bindArtifact(digest)) cmd.reply("[khook-accept] resume binding mismatch");
      else if (sawReload && !armScriptReload(true)) cmd.reply("[khook-accept] pending: resident reload target unavailable");
      else cmd.reply("[khook-accept] already resumed " + runId);
      return HookResult.Handled;
    }
    if (sub === "reload-arm") {
      if (!runBound || id !== runId || !artifactIdentity) cmd.reply("[khook-accept] reload-arm requires bound run/digest");
      else if (!armScriptReload(false)) cmd.reply("[khook-accept] pending: native reload target/owned marker unavailable");
      else cmd.reply("[khook-accept] reload armed; wait for native before-ack then reload only this .s2sp");
      return HookResult.Handled;
    }
    if (sub === "resume" && !runBound) {
      if (!handoff || handoff.runId !== id || handoff.artifactIdentity !== digest || !/^[a-f0-9]{64}$/.test(digest)) {
        cmd.reply("[khook-accept] resume requires matching .s2sp state handoff and artifact binding");
        return HookResult.Handled;
      }
      runId = id; runBound = true; sawReload = true;
      bindArtifact(digest);
      for (const rec of handoff.records || []) if (rec.result !== "pending") terminal.set(rec.case + ":" + rec.subcheck, rec);
      setCvar("s2_khook_accept_run", id);
      setCvar("s2_khook_accept_instance", String(instance));
      // Do not recreate or reinvoke completed filter/phase cases after reload.
      if (!armScriptReload(true)) cmd.reply("[khook-accept] pending: resident reload target unavailable");
      else cmd.reply("[khook-accept] resumed script " + runId);
      return HookResult.Handled;
    }
    if (sub === "prepare") {
      if (!id) {
        cmd.reply("usage: s2_khook_accept prepare <run_id>");
        return HookResult.Handled;
      }
      if (digest && !/^[a-f0-9]{64}$/.test(digest)) { cmd.reply("[khook-accept] invalid artifact digest"); return HookResult.Handled; }
      runId = id;
      runBound = true;
      artifactIdentity = "";
      frozenRevision = KHOOK_FIXTURE_REVISION;
      if (digest) bindArtifact(digest);
      terminal.clear();
      collected = false;
      stored = [];
      hookEnabled = true;
      flipped = false;
      frames = 0;
      clientsConnected = 0;
      jsContinue = 0;
      jsHandled = 0;
      jsHandledUnsuppressed = 0;
      lastSlot = -1;
      lastUserId = -1;
      lastSteamId = "";
      sawReload = false;
      handoff = undefined;
      reloadTarget = null; reloadPre = reloadPost = 0;
      Server.setCvar("s2_khook_accept_run", id);
      prepareR6();
      setCvar("s2_khook_accept_mask_mode", maskMode);
      cmd.reply("[khook-accept] prepared run_id=" + id);
      for (const line of clientActions) cmd.reply("NEED_CLIENTS: " + line);
      return HookResult.Handled;
    }
    if (sub === "collect") {
      if (!id) {
        cmd.reply("usage: s2_khook_accept collect <run_id>");
        return HookResult.Handled;
      }
      if (!runBound || runId !== id) {
        const prevId = runId;
        const prevStored = stored;
        runId = id;
        stored = [];
        pushInvalid(id);
        for (const rec of stored) cmd.reply(emit(rec));
        stored = prevStored;
        runId = prevId;
        return HookResult.Handled;
      }
      if (maskMode === "subset" && maskSubsetSeen) {
        maskMode = "all";
        setCvar("s2_khook_accept_mask_mode", "all");
      }
      collectJs();
      for (const rec of stored) cmd.reply(emit(rec));
      return HookResult.Handled;
    }
    if (sub === "report") {
      if (!id) {
        cmd.reply("usage: s2_khook_accept report <run_id>");
        return HookResult.Handled;
      }
      if (!runBound || runId !== id) {
        const prevId = runId;
        const prevStored = stored;
        runId = id;
        stored = [];
        pushInvalid(id);
        for (const rec of stored) cmd.reply(emit(rec));
        stored = prevStored;
        runId = prevId;
        return HookResult.Handled;
      }
      if (!collected) {
        pushOwnedPendingUncollected();
      }
      for (const rec of stored) cmd.reply(emit(rec));
      return HookResult.Handled;
    }
    if (sub === "restore" || sub === "teardown") {
      if (!runBound || id !== runId) { cmd.reply("[khook-accept] unknown or mismatched run_id"); return HookResult.Handled; }
      // Capture the policy observations before starting the restored phase.
      collectJs();
      restoreTransmitVisibility();
      Voice.resetAll();
      maskMode = "off";
      setCvar("s2_khook_accept_voice_phase", "restored");
      setCvar("s2_khook_accept_tx_phase", "restored");
      setCvar("s2_khook_accept_mask_mode", "off");
      if (sub === "teardown") {
        cleanupOwned(); runBound = false;
        setCvar("s2_khook_accept_voice_phase", "off");
        setCvar("s2_khook_accept_tx_phase", "off");
      }
      cmd.reply("[khook-accept] " + sub);
      return HookResult.Handled;
    }
    cmd.reply("usage: s2_khook_accept runtime|prepare|bind|collect|report|restore|teardown|reload-arm|resume <run_id> [artifact_sha256]");
    return HookResult.Handled;
  });

}

export function OnGameFrame(): void {
  frames += 1;
  if (!runBound) return;
  r6Frames += 1;
  advancePhase();
}

export function OnClientConnected(c: Client): void {
  if (runBound && c && c.isValid() && !c.isBot) {
    clientsConnected += 1;
    lastSlot = c.slot;
    lastUserId = c.userId;
    lastSteamId = c.steamId;
  }
}

export function OnMapStart(map: string): void {
  if (!runBound) return;
  if (mapAtPrepare && map && map !== mapAtPrepare) {
    mapEnded = true;
  }
}

export function OnMapEnd(): void {
  if (runBound) {
    mapEnded = true;
    setCvar("s2_khook_accept_map_ended", "1");
  }
}

export function OnPluginState(): PersistState {
  return {
    runId,
    oldIndex: identityIndex,
    oldId: identityId,
    mapName: mapAtPrepare,
    unloaded: true,
    instance,
    deliveredA,
    deliveredB,
    artifactIdentity,
    sourceRevision: frozenRevision,
    records: Array.from(terminal.values()),
  };
}

export function OnPluginEnd(): void {
  if (reloadTarget) setCvar("s2_khook_accept_reload_unloaded", String(instance));
  setCvar("s2_khook_accept_live", "0");
  setCvar("s2_khook_accept_unloaded", "1");
  persistOutsidePlugin();
  cleanupOwned();
  console.log(`[khook-accept] unloading frames=${frames}`);
}

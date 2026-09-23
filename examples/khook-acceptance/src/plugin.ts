// khook-acceptance — JS fixture for KHook suites A/B/C. NOT a shipped plugin.
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
  onOutput,
} from "@s2script/sdk";
import type { Client, DamageInfo, EntityRef, HookResultValue } from "@s2script/sdk";
import { Engine } from "@s2script/sdk/unsafe";
import type { PrecacheContext } from "@s2script/sdk/sound";
import type { UserCmdView } from "@s2script/sdk/usercmd";
import { Player, items } from "@s2script/cs2";
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

type Suite = "A" | "B" | "C";

interface Rec {
  case: string;
  subcheck: string;
  producer: "js";
  result: Result;
  expected: Record<string, unknown>;
  actual: Record<string, unknown>;
  evidence: string;
  evidence_class?: "observed";
  group?: string;
  provenance?: string;
  callback_owner?: string;
  target?: string;
  observations?: Observation[];
}

interface PhaseSnap {
  pre: number;
  post: number;
}

interface PersistState {
  lifetimeRows?: Observation[];
  suite: Suite;
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

let runSuite: Suite = "A";
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
    ...rec,
    schema: 1,
    suite: runSuite,
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
  installIntegrationHooks();
  installNamedIntegrationHooks();
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
    if (!runBound || runSuite !== "A" || maskMode === "off") return;
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
    if (!runBound || runSuite !== "A" || args[0] !== runId || !realClients().some(c => c.slot === slot)) return HookResult.Continue;
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
    const suffix = [cmd.arg(2), cmd.arg(3), cmd.arg(4)].filter(value => !!value);
    const suites = suffix.filter(value => /^(A|B|C)$/.test(value));
    const digests = suffix.filter(value => /^[a-f0-9]{64}$/.test(value));
    const selected = (suites[0] || "A") as Suite;
    const digest = digests[0] || "";
    if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/.test(id) || suites.length > 1 || digests.length > 1 ||
        suites.length + digests.length !== suffix.length || !!cmd.arg(5) ||
        (sub !== "prepare" && runBound && (selected !== runSuite || id !== runId))) {
      cmd.reply(JSON.stringify({ khook_acceptance_error: "invalid command or suite/run mismatch" }));
      return HookResult.Handled;
    }
    if (selected !== "A") {
      integrationCommand(sub, id, digest, selected, line => cmd.reply(line));
      return HookResult.Handled;
    }
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
      if (!handoff || (handoff.suite || "A") !== selected || handoff.runId !== id || handoff.artifactIdentity !== digest || !/^[a-f0-9]{64}$/.test(digest)) {
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
      runSuite = selected;
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
  if (runSuite !== "A") { integrationFrame(); return; }
  r6Frames += 1;
  advancePhase();
}

export function OnClientConnected(c: Client): void {
  if (runBound && runSuite === "A" && c && c.isValid() && !c.isBot) {
    clientsConnected += 1;
    lastSlot = c.slot;
    lastUserId = c.userId;
    lastSteamId = c.steamId;
  }
}

export function OnMapStart(map: string): void {
  integrationMap += 1;
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
    suite: runSuite,
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
    lifetimeRows: [...lifetimeRows],
    records: Array.from(runSuite === "A" ? terminal.values() : integrationRecords.values()),
  };
}

export function OnPluginEnd(): void {
  if (reloadTarget) setCvar("s2_khook_accept_reload_unloaded", String(instance));
  setCvar("s2_khook_accept_live", "0");
  setCvar("s2_khook_accept_unloaded", "1");
  persistOutsidePlugin();
  cleanupOwned();
  cleanupIntegrationOwned();
  console.log(`[khook-accept] unloading frames=${frames}`);
}

// B/C contract mirrors the frozen controller registry. These declarations are
// expectations, never observations: no row passes until a real callback records it.
interface Observation {
  scenario_id: string;
  sequence: number;
  generation: number;
  invocation: string;
  peer_order: string;
  callbacks: number;
  facts: Record<string, unknown>;
  stimulus?: string;
  route?: string;
  frame_token?: number;
  map_generation?: number;
  receiver?: string;
  vtable?: string;
  manifest?: string;
}
interface IntegrationRow {
  case: string; subcheck: string; expected: Record<string, unknown>;
  group: string; provenance: string; callback_owner: string; target: string;
}
const INTEGRATION_ROWS: Record<"B" | "C", IntegrationRow[]> = {
  "B": [
    {
      "case": "declarative_this_void",
      "subcheck": "js_this_void_continue_delivery",
      "expected": {
        "pre": 1
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_this_void"
    },
    {
      "case": "declarative_this_void",
      "subcheck": "js_this_void_handled_delivery",
      "expected": {
        "pre": 1,
        "action": 2
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_this_void"
    },
    {
      "case": "declarative_mutable_narrow",
      "subcheck": "js_narrow_all_fields_mutated",
      "expected": {
        "value": 7.25,
        "a": -17,
        "b": 29,
        "c": -31
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_mutable_narrow"
    },
    {
      "case": "declarative_mutable_wide",
      "subcheck": "js_wide_mutation_delivery",
      "expected": {
        "value": 7.25,
        "integer": -17
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_mutable_wide"
    },
    {
      "case": "declarative_acquisition",
      "subcheck": "js_acquire_outbound_pre_vote",
      "expected": {
        "votes": [
          6,
          0,
          1
        ],
        "outbound_nested": true
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_acquisition"
    },
    {
      "case": "declarative_acquisition",
      "subcheck": "js_acquire_outbound_final_result",
      "expected": {
        "effective": [
          6,
          6,
          1
        ],
        "outbound_nested": true
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_acquisition"
    },
    {
      "case": "declarative_hud",
      "subcheck": "js_hud_receiver_text_continue",
      "expected": {
        "receiver_matches_controller": true,
        "text": "s2-khook-hud"
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_hud"
    },
    {
      "case": "declarative_hud",
      "subcheck": "js_hud_handled_delivery",
      "expected": {
        "pre": 1,
        "action": 2
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_hud"
    },
    {
      "case": "declarative_hud",
      "subcheck": "js_hud_direct_utlstring",
      "expected": {
        "text": "direct-hud",
        "receiver_matches_controller": true
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_hud"
    },
    {
      "case": "declarative_nesting_bypass",
      "subcheck": "js_different_id_nested_delivery",
      "expected": {
        "outer": 1,
        "inner": 1,
        "restored": true
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_nesting_bypass"
    },
    {
      "case": "declarative_nesting_bypass",
      "subcheck": "js_same_id_reentry_named_skip",
      "expected": {
        "delivered": 1,
        "nested_safe_skip": true
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_nesting_bypass"
    },
    {
      "case": "declarative_nesting_bypass",
      "subcheck": "js_bypass_absent_then_next_delivered",
      "expected": {
        "bypass": 0,
        "next": 1
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "declarative_nesting_bypass"
    },
    {
      "case": "acquisition_named_hook",
      "subcheck": "js_acquire_real_post_effective",
      "expected": {
        "real_bot": true,
        "effective_result_observed": true,
        "skipped_observed": true
      },
      "group": "live-named",
      "provenance": "live-engine",
      "callback_owner": "engine_hooks",
      "target": "acquisition_named_hook"
    },
    {
      "case": "damage_named_hook",
      "subcheck": "js_damage_pre_post_correct_victim",
      "expected": {
        "pre": 1,
        "post": 1,
        "victim_matches": true
      },
      "group": "live-named",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "damage_named_hook"
    },
    {
      "case": "chat_named_hook",
      "subcheck": "js_chat_continue_delivery",
      "expected": {
        "continue": 1
      },
      "group": "live-named",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "chat_named_hook"
    },
    {
      "case": "chat_named_hook",
      "subcheck": "js_chat_suppression_vote",
      "expected": {
        "suppressed": 1,
        "action": 2
      },
      "group": "live-named",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "chat_named_hook"
    },
    {
      "case": "output_named_hook",
      "subcheck": "js_output_delivery_and_suppression",
      "expected": {
        "actions": [
          0,
          1,
          2,
          3
        ],
        "deliveries": 4
      },
      "group": "live-named",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "output_named_hook"
    },
    {
      "case": "usercmd_named_hook",
      "subcheck": "js_usercmd_batch_delivery_neutralization",
      "expected": {
        "delivered": true,
        "neutralized": true
      },
      "group": "live-named",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "usercmd_named_hook"
    },
    {
      "case": "script_generation_lifetime",
      "subcheck": "js_old_generation_retired",
      "expected": {
        "generations_retired": 2
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "script_generation_lifetime"
    },
    {
      "case": "script_generation_lifetime",
      "subcheck": "js_new_generation_callback",
      "expected": {
        "new_generations_delivered": 2
      },
      "group": "main-runtime-bridge",
      "provenance": "main-runtime",
      "callback_owner": "engine_hooks",
      "target": "script_generation_lifetime"
    }
  ],
  "C": [
    {
      "case": "precache_map_transition",
      "subcheck": "js_precache_before_after_map_delivery",
      "expected": {
        "virtual_before": true,
        "virtual_after": true
      },
      "group": "main-runtime-bridge",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "precache_map_transition"
    },
    {
      "case": "precache_map_transition",
      "subcheck": "js_precache_resource_each_generation",
      "expected": {
        "added_before": true,
        "added_after": true
      },
      "group": "main-runtime-bridge",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "precache_map_transition"
    },
    {
      "case": "precache_map_transition",
      "subcheck": "js_precache_stale_context_rejected",
      "expected": {
        "stale_add": false
      },
      "group": "main-runtime-bridge",
      "provenance": "live-engine",
      "callback_owner": "named_hooks",
      "target": "precache_map_transition"
    }
  ]
};
const integrationRecords = new Map<string, Rec>();
let integrationMap = 1;
let integrationSequence = 0;
let integrationDriven = false;
let bridgeEntity: EntityRef | null = null;
let integrationNamedDriven = false;
let realAcquireSlot = -1;
let realOutputAction = -1;
let realOutput: EntityRef | null = null;
let damageSubject: EntityRef | null = null;
const damageStack: Observation[] = [];
const realOutputRows: Observation[] = [];
let stalePrecache: PrecacheContext | null = null;
let activeBridge: { scenario: number; sequence: number; order: number; callbacks: number; observations: Observation[] } | null = null;
let bridgeDrive: ReturnType<typeof Engine.call<"bridgeDrive">> = null;
let bridgeMark: ReturnType<typeof Engine.call<"bridgeMark">> = null;
let bridgeWindow: ReturnType<typeof Engine.call<"bridgeWindow">> = null;
let precacheBegin: ReturnType<typeof Engine.call<"precacheBegin">> = null;
let precacheFinish: ReturnType<typeof Engine.call<"precacheFinish">> = null;
let realAcquireMark: ReturnType<typeof Engine.call<"realAcquireMark">> = null;
let precacheRead: ReturnType<typeof Engine.call<"precacheRead">> = null;

function integrationRecord(name: string, actual: Record<string, unknown>, observations: Observation[], evidence: string): void {
  if (!runBound || runSuite === "A" || !artifactIdentity || !observations.length) return;
  const row = INTEGRATION_ROWS[runSuite].find(value => value.subcheck === name);
  if (!row) return;
  const result = JSON.stringify(actual) === JSON.stringify(row.expected) ? "pass" : "fail";
  const record: Rec = { ...row, producer: "js", result, actual, evidence, evidence_class: "observed", observations };
  const prior = integrationRecords.get(name);
  // Snapshot once at completion. An earlier failure cannot disappear on report.
  if (!prior || (prior.result !== "fail" && result === "fail")) integrationRecords.set(name, record);
}

function integrationCollect(): void {
  if (runSuite === "A") return;
  stored = INTEGRATION_ROWS[runSuite].map(row => integrationRecords.get(row.subcheck) || {
    ...row, producer: "js", result: "pending", actual: {}, evidence: "waiting for source-bound observed callbacks",
  });
  collected = true;
}

function integrationCommand(sub: string, id: string, digest: string, selected: "B" | "C", reply: (line: string) => void): void {
  if (sub === "prepare") {
    cleanupIntegrationOwned();
    runId = id; runSuite = selected; runBound = true; artifactIdentity = "";
    frozenRevision = KHOOK_FIXTURE_REVISION;
    if (digest) bindArtifact(digest);
    integrationRecords.clear(); precacheRows.length = 0; lifetimeRows.length = 0; integrationSequence = 0; integrationDriven = false; integrationNamedDriven = false;
    activeBridge = null; collected = false; stored = []; stalePrecache = null;
    if (bridgeEntity) { bridgeEntity.remove(); bridgeEntity = null; }
    if (selected === "B") {
      bridgeEntity = createEntity("info_target");
      if (bridgeEntity) bridgeEntity.spawn();
    }
    setCvar("s2_khook_accept_run", id);
    reply("[khook-accept] prepared " + selected + " " + id);
    return;
  }
  if (sub === "resume" && !runBound) {
    if (!handoff || handoff.suite !== selected || handoff.runId !== id || handoff.artifactIdentity !== digest ||
        handoff.sourceRevision !== KHOOK_FIXTURE_REVISION || !/^[a-f0-9]{64}$/.test(digest)) {
      reply(JSON.stringify({ khook_acceptance_error: "suite/run/artifact/source handoff mismatch" })); return;
    }
    runId = id; runSuite = selected; runBound = true; bindArtifact(digest);
    for (const record of handoff.records || []) integrationRecords.set(record.subcheck, record);
    lifetimeRows.push(...(handoff.lifetimeRows || []));
    // Native target remains resident. A callback after resume must be observed separately.
    integrationDriven = false;
    if (selected === "B") { bridgeEntity = createEntity("info_target"); bridgeEntity?.spawn(); }
    reply("[khook-accept] resumed " + selected + " " + id); return;
  }
  if (!runBound || runSuite !== selected || runId !== id) {
    reply(JSON.stringify({ khook_acceptance_error: "unknown or mismatched suite/run" })); return;
  }
  if (sub === "bind") {
    if (!bindArtifact(digest)) reply(JSON.stringify({ khook_acceptance_error: "invalid binding" }));
    return;
  }
  if (sub === "collect") integrationFrame();
  if (sub === "collect" || sub === "report") {
    integrationCollect();
    for (const record of stored) reply(emit(record));
    return;
  }
  if (sub === "teardown") {
    cleanupIntegrationOwned();
    runBound = false; reply("[khook-accept] teardown " + id); return;
  }
  if (sub === "reload-arm") {
    reply("[khook-accept] generation " + instance + " captured; reload archive, resume same suite/run/artifact, then collect; repeat twice"); return;
  }
  reply(JSON.stringify({ khook_acceptance_error: "unsupported integration command" }));
}

function bridgeObservation(facts: Record<string, unknown>): Observation | null {
  const active = activeBridge;
  if (!active || !bridgeMark || !runBound || runSuite !== "B") return null;
  const token = bridgeMark(runId, active.scenario, active.sequence, instance, 1, active.order);
  if (!token) return null;
  active.callbacks += 1;
  const observation: Observation = { scenario_id: "bridge-" + active.scenario,
    sequence: active.sequence, generation: instance,
    invocation: runId + ":" + instance + ":" + active.sequence,
    peer_order: active.order === 0 ? "peer-first" : "s2script-first", callbacks: 1, facts };
  active.observations.push(observation);
  return observation;
}

function installIntegrationHooks(): void {
  realAcquireMark = Engine.call("realAcquireMark");
  bridgeDrive = Engine.call("bridgeDrive"); bridgeMark = Engine.call("bridgeMark"); bridgeWindow = Engine.call("bridgeWindow");
  precacheBegin = Engine.call("precacheBegin"); precacheFinish = Engine.call("precacheFinish"); precacheRead = Engine.call("precacheRead");
  for (const name of ["onvoid0", "onvoid1"] as const) Engine.hook(name)?.(view => {
    if (runBound && runSuite === "B") bridgeMark?.(runId, 14, -1, instance, 2, -1);
    if (!activeBridge) return HookResult.Continue;
    const suppressed = activeBridge.scenario === 2;
    const row = bridgeObservation({ pre: 1, action: suppressed ? 2 : 0 });
    if (bridgeEntity && activeBridge.scenario === 10) {
      const same = Engine.call(activeBridge.order === 0 ? "void0" : "void1");
      same?.(bridgeEntity);
      if (row) row.facts.nested_safe_skip = activeBridge.callbacks === 1;
    }
    if (bridgeEntity && activeBridge.scenario === 11) {
      const inner = Engine.call(activeBridge.order === 0 ? "narrow0" : "narrow1");
      const before = view.receiver;
      inner?.(bridgeEntity, 1.5, 3, 4, 5);
      const after = view.receiver;
      if (row) row.facts.restored = activeBridge.callbacks === 2 && !!before && !!after && before.id === after.id && before.index === after.index;
    }
    return suppressed ? HookResult.Handled : HookResult.Continue;
  });
  for (const name of ["onnarrow0", "onnarrow1"] as const) Engine.hook(name)?.(view => {
    if (!activeBridge) return HookResult.Continue;
    view.value = 7.25; view.a = -17; view.b = 29; view.c = -31;
    bridgeObservation({ value: view.value, a: view.a, b: view.b, c: view.c });
    return HookResult.Changed;
  });
  for (const name of ["onwide0", "onwide1"] as const) Engine.hook(name)?.(view => {
    if (!activeBridge) return HookResult.Continue;
    view.value = 7.25; view.integer = -17;
    bridgeObservation({ value: view.value, integer: view.integer });
    return HookResult.Changed;
  });
  for (const name of ["onacquire0", "onacquire1"] as const) Engine.hook(name)?.(view => {
    if (!activeBridge) return HookResult.Continue;
    const implicit = activeBridge.scenario === 7;
    if (!implicit) view.result = activeBridge.scenario === 5 ? 6 : 0;
    bridgeObservation({ vote: implicit ? 1 : view.result, method: view.method, outbound_nested: true });
    return implicit ? HookResult.Handled : HookResult.Changed;
  });
  for (const name of ["onhud0", "onhud1"] as const) Engine.hook(name)?.(view => {
    if (!activeBridge) return HookResult.Continue;
    const suppressed = activeBridge.scenario === 9;
    bridgeObservation({ receiver_matches_controller: !!view.receiver && !!bridgeEntity && view.receiver.index === bridgeEntity.index && view.receiver.id === bridgeEntity.id,
      text: view.text, action: suppressed ? 2 : 0 });
    return suppressed ? HookResult.Handled : HookResult.Continue;
  });
}

function integrationFrame(): void {
  if (!runBound || !artifactIdentity) return;
  if (stalePrecache && runSuite === "C") {
    // Public API rejects the expired manifest; this cannot touch borrowed pointers.
    const added = stalePrecache.add("soundevents/game_sounds.vsndevts");
    stalePrecache = null;
    const rows = integrationRecords.get("js_precache_resource_each_generation")?.observations;
    if (rows?.length) integrationRecord("js_precache_stale_context_rejected", { stale_add: added }, [rows[rows.length - 1]], "public add called after callback return");
  }
  if (runSuite === "B") collectNamedIntegration();
  if (runSuite !== "B" || integrationDriven || !bridgeDrive || !bridgeMark || !bridgeWindow || !bridgeEntity) return;
  integrationDriven = true;
  // Native owns target calls/original counts and validates markers inside this
  // synchronous JS -> Engine.call -> main-hook -> JS window.
  const observations = new Map<number, Observation[]>();
  const results = new Map<number, Array<number | null>>();
  for (let order = 0; order < 2; ++order) for (let scenario = 1; scenario <= 11; ++scenario) {
    const sequence = ++integrationSequence;
    activeBridge = { scenario, sequence, order, callbacks: 0, observations: [] };
    const finalResult = bridgeDrive(bridgeEntity, scenario + order * 100, sequence, instance, runId);
    results.set(scenario, [...(results.get(scenario) || []), finalResult]);
    observations.set(scenario, [...(observations.get(scenario) || []), ...activeBridge.observations]);
    activeBridge = null;
  }
  for (let order = 0; order < 2; ++order) {
    const sequence = ++integrationSequence;
    activeBridge = { scenario: 12, sequence, order, callbacks: 0, observations: [] };
    if (bridgeWindow(runId, 12 + order * 100, sequence, instance, true)) {
      const bypass = Engine.call(order === 0 ? "void0Bypass" : "void1Bypass");
      const direct = Engine.call(order === 0 ? "void0" : "void1");
      bypass?.(bridgeEntity);
      const bypassCallbacks = activeBridge.callbacks;
      const checkpoint = bridgeMark(runId, 12, sequence, instance, 3, order);
      direct?.(bridgeEntity);
      for (const row of activeBridge.observations) row.facts = { bypass: bypassCallbacks, next: activeBridge.callbacks - bypassCallbacks };
      const closed = bridgeWindow(runId, 12 + order * 100, sequence, instance, false);
      if (closed && checkpoint) observations.set(12, [...(observations.get(12) || []), ...activeBridge.observations]);
    }
    activeBridge = null;
  }
  for (let order = 0; order < 2; ++order) {
    const sequence = ++integrationSequence;
    activeBridge = { scenario: 13, sequence, order, callbacks: 0, observations: [] };
    if (bridgeWindow(runId, 13 + order * 100, sequence, instance, true)) {
      const direct = Engine.call(order === 0 ? "hud0" : "hud1");
      direct?.(bridgeEntity, bridgeEntity, bridgeEntity, "direct-hud");
      const closed = bridgeWindow(runId, 13 + order * 100, sequence, instance, false);
      if (closed) observations.set(13, [...(observations.get(13) || []), ...activeBridge.observations]);
    }
    activeBridge = null;
  }
  const record = (scenario: number, name: string, select: (facts: Record<string, unknown>) => Record<string, unknown>): void => {
    const rows = observations.get(scenario) || [];
    if (rows.length !== 2) return;
    const outcomes = rows.map(row => select(row.facts));
    const equal = JSON.stringify(outcomes[0]) === JSON.stringify(outcomes[1]);
    integrationRecord(name, equal ? outcomes[0] : { per_invocation: outcomes }, rows, "measured JS callbacks inside native driver; both target sets");
  };
  record(1, "js_this_void_continue_delivery", f => ({ pre: f.pre }));
  record(2, "js_this_void_handled_delivery", f => ({ pre: f.pre, action: f.action }));
  record(3, "js_narrow_all_fields_mutated", f => f);
  record(4, "js_wide_mutation_delivery", f => f);
  record(10, "js_same_id_reentry_named_skip", f => ({ delivered: f.pre, nested_safe_skip: f.nested_safe_skip }));
  const nested = observations.get(11) || [];
  if (nested.length === 4) integrationRecord("js_different_id_nested_delivery", {
    outer: nested.filter(row => "pre" in row.facts).length / 2,
    inner: nested.filter(row => "value" in row.facts).length / 2,
    restored: nested.filter(row => "pre" in row.facts).every(row => row.facts.restored === true),
  }, nested.filter(row => "pre" in row.facts), "actual different-id Engine.call nested within the outer handler");
  record(12, "js_bypass_absent_then_next_delivered", f => f);
  const acquisition = [5, 6, 7].flatMap(scenario => observations.get(scenario) || []);
  if (acquisition.length === 6) integrationRecord("js_acquire_outbound_pre_vote", {
    votes: [5, 6, 7].map(scenario => {
      const rows = observations.get(scenario) || [];
      return rows.length === 2 && rows[0].facts.vote === rows[1].facts.vote ? rows[0].facts.vote : null;
    }), outbound_nested: true,
  }, acquisition, "outbound JS call delivered PRE handler votes; native and POST records decide propagation");
  if (acquisition.length === 6) integrationRecord("js_acquire_outbound_final_result", {
    effective: [5, 6, 7].map(scenario => {
      const values = results.get(scenario) || [];
      return values.length === 2 && values[0] === values[1] ? values[0] : null;
    }), outbound_nested: true,
  }, acquisition, "JS observes final caller result after Engine.call returns; this is NOT the main POST position");
  record(8, "js_hud_receiver_text_continue", f => ({ receiver_matches_controller: f.receiver_matches_controller, text: f.text }));
  record(9, "js_hud_handled_delivery", f => ({ pre: 1, action: f.action }));
  record(13, "js_hud_direct_utlstring", f => ({ text: f.text, receiver_matches_controller: f.receiver_matches_controller }));
  const sequence = ++integrationSequence;
  activeBridge = { scenario: 14, sequence, order: 0, callbacks: 0, observations: [] };
  const staleCallbacks = bridgeDrive(bridgeEntity, 14, sequence, instance, runId);
  for (const row of activeBridge.observations) {
    row.facts = { current_callbacks: activeBridge.callbacks, native_old_callbacks: staleCallbacks };
    lifetimeRows.push(row);
  }
  activeBridge = null;
  if (new Set(lifetimeRows.map(row => row.generation)).size >= 3) {
    const fresh = lifetimeRows.slice(1).filter(row => row.facts.current_callbacks === 1).length;
    const retired = lifetimeRows.slice(1).filter(row => row.facts.native_old_callbacks === 0).length;
    integrationRecord("js_old_generation_retired", { generations_retired: retired }, [...lifetimeRows], "native driver returned zero stale-generation markers during each replacement call");
    integrationRecord("js_new_generation_callback", { new_generations_delivered: fresh }, [...lifetimeRows], "public state handoff preserves actual new-generation callback rows");
  }
}

const lifetimeRows: Observation[] = [];
const precacheRows: Observation[] = [];
export function OnPrecache(context: PrecacheContext): void {
  if (!runBound || runSuite !== "C" || !artifactIdentity || !precacheBegin || !precacheFinish || !precacheRead) return;
  const token = precacheBegin(runId, instance, integrationMap);
  if (token === null || token <= 0) return; // A session-only callback is not a virtual callback.
  const added = context.add("soundevents/game_sounds.vsndevts");
  if (!precacheFinish(token, "soundevents/game_sounds.vsndevts", added, instance)) return;
  const nativeGeneration = precacheRead(token, 0);
  if (nativeGeneration === null || nativeGeneration <= 0) return;
  const order = precacheRead(token, 1);
  const row: Observation = { scenario_id: "precache-map", sequence: token, generation: instance,
    invocation: "precache-" + token, callbacks: 1, facts: { added }, stimulus: "engine",
    peer_order: order === 0 ? "peer-first" : order === 1 ? "s2script-first" : "none",
    route: "main-virtual-precache", frame_token: token, map_generation: nativeGeneration,
    receiver: "receiver-" + precacheRead(token, 2), vtable: "vtable-" + precacheRead(token, 3),
    manifest: "manifest-" + precacheRead(token, 4) };
  precacheRows.push(row); stalePrecache = context;
  if (new Set(precacheRows.map(value => value.map_generation)).size < 2) return;
  const rows = [...precacheRows];
  integrationRecord("js_precache_before_after_map_delivery", { virtual_before: true, virtual_after: true }, rows, "native begin/finish token authenticates this main virtual callback");
  integrationRecord("js_precache_resource_each_generation", { added_before: rows[0].facts.added, added_after: rows[rows.length - 1].facts.added }, rows,
    "public ctx.add return during actual main virtual frame; no claim about internal add route or rendering");
}

export function OnClientSayCommand(slot: number, text: string): HookResultValue {
  if (!runBound || runSuite !== "B" || !realClients().some(client => client.slot === slot)) return HookResult.Continue;
  const suppressed = text === runId + "-suppress";
  if (!suppressed && text !== runId + "-continue") return HookResult.Continue;
  const name = suppressed ? "js_chat_suppression_vote" : "js_chat_continue_delivery";
  integrationRecord(name, suppressed ? { suppressed: 1, action: 2 } : { continue: 1 }, [{
    scenario_id: name, sequence: ++integrationSequence, generation: instance, invocation: runId + ":chat:" + integrationSequence,
    callbacks: 1, peer_order: "none", facts: { slot, text }, stimulus: "real-client",
  }], "real-client SayCommand callback with run-bound token");
  return suppressed ? HookResult.Handled : HookResult.Continue;
}

export function OnPlayerRunCmd(view: UserCmdView, info: { slot: number }): HookResultValue {
  if (!runBound || runSuite !== "B" || !realClients().some(client => client.slot === info.slot) ||
      integrationRecords.has("js_usercmd_batch_delivery_neutralization")) return HookResult.Continue;
  view.forwardMove = 0; view.sideMove = 0; view.upMove = 0; view.buttons = 0n;
  integrationRecord("js_usercmd_batch_delivery_neutralization", { delivered: true, neutralized: view.forwardMove === 0 && view.sideMove === 0 && view.upMove === 0 && view.buttons === 0n }, [{
    scenario_id: "client-input", sequence: ++integrationSequence, generation: instance, invocation: runId + ":usercmd:" + integrationSequence,
    callbacks: 1, peer_order: "none", facts: { slot: info.slot, action: 2 }, stimulus: "real-client",
  }], "real client's usercmd delivered and neutralized via public borrowed view");
  return HookResult.Handled;
}


function installNamedIntegrationHooks(): void {
  items.onCanAcquirePost(view => {
    if (!runBound || runSuite !== "B" || realAcquireSlot < 0 || view.player?.slot !== realAcquireSlot) return;
    const bot = Clients.all().find(client => client.slot === realAcquireSlot && client.isBot && client.isValid());
    if (!bot) return;
    if (integrationRecords.has("js_acquire_real_post_effective")) return;
    const result = view.result, skipped = view.skipped, defIndex = view.defIndex;
    const token = realAcquireMark?.(runId, instance, bot.slot, defIndex, result, skipped);
    if (!token || token <= 0) return; // Native peer has no same-invocation real target frame.
    integrationRecord("js_acquire_real_post_effective", { real_bot: true,
      effective_result_observed: Number.isInteger(result), skipped_observed: typeof skipped === "boolean" }, [{
      scenario_id: "real-bot-acquire", sequence: token, generation: instance,
      invocation: "real-acquire-" + token, peer_order: "none", callbacks: 1,
      facts: { slot: bot.slot, defIndex, result, skipped }, stimulus: "engine",
    }], "public items.onCanAcquirePost during a real bot item action; actual POST result/skipped, not final caller result");
  });
  onOutput("logic_relay", "OnTrigger", event => {
    if (!runBound || runSuite !== "B" || realOutputAction < 0 || !realOutput || event.caller?.id !== realOutput.id) return HookResult.Continue;
    const action = realOutputAction;
    realOutputRows.push({ scenario_id: "real-output-" + action, sequence: ++integrationSequence, generation: instance,
      invocation: runId + ":output:" + integrationSequence, callbacks: 1, peer_order: "none",
      facts: { action, caller: event.caller.index, output: event.output }, stimulus: "engine" });
    return action as HookResultValue;
  });
}

function onIntegrationDamagePre(info: DamageInfo): void {
  const victim = info.victim;
  if (!victim || !runBound || runSuite !== "B" || !damageSubject || victim.id !== damageSubject.id || !Number.isFinite(info.damage)) return;
  damageStack.push({ scenario_id: "real-bot-damage", sequence: ++integrationSequence, generation: instance,
    invocation: runId + ":damage:" + integrationSequence, callbacks: 1, peer_order: "none",
    facts: { victim: victim.index, victim_id: victim.id, damage: info.damage }, stimulus: "engine" });
}
function onIntegrationDamagePost(info: DamageInfo): void {
  const victim = info.victim;
  const pre = damageStack[damageStack.length - 1];
  if (!victim || !pre || !runBound || runSuite !== "B" || victim.id !== pre.facts.victim_id) return;
  damageStack.pop();
  integrationRecord("js_damage_pre_post_correct_victim", { pre: 1, post: 1, victim_matches: true },
    [{ ...pre, facts: { ...pre.facts, post_damage: info.damage } }], "real bot victim synchronous PRE/POST; nested scopes pair by stack; synthetic dummy cannot match books-gated pawn");
}
function cleanupIntegrationOwned(): void {
  if (damageSubject) {
    SDKUnhook(damageSubject, SDKHookType.OnTakeDamage, onIntegrationDamagePre);
    SDKUnhook(damageSubject, SDKHookType.OnTakeDamagePost, onIntegrationDamagePost);
    damageSubject = null;
  }
  damageStack.length = 0; realOutputRows.length = 0;
  realAcquireSlot = -1; realOutputAction = -1;
  if (realOutput) { realOutput.remove(); realOutput = null; }
  if (bridgeEntity) { bridgeEntity.remove(); bridgeEntity = null; }
}

function collectNamedIntegration(): void {
  const client = Clients.all().find(value => value.isBot && value.isValid());
  const pawn = client ? Player.fromSlot(client.slot)?.pawn : null;
  if (pawn?.isValid && !damageSubject) {
    damageSubject = pawn.ref;
    SDKHook(damageSubject, SDKHookType.OnTakeDamage, onIntegrationDamagePre);
    SDKHook(damageSubject, SDKHookType.OnTakeDamagePost, onIntegrationDamagePost);
  }
  if (integrationNamedDriven || !pawn?.isValid || !client) return;
  integrationNamedDriven = true;
  realAcquireSlot = client.slot;
  const item = pawn.giveNamedItem("weapon_decoy");
  realAcquireSlot = -1;
  if (item) pawn.removeWeapon(item);
  realOutput = createEntity("logic_relay", { targetname: "s2khook-" + runId, spawnflags: "2" });
  if (realOutput) {
    realOutput.spawn();
    for (let action = 0; action < 4; ++action) {
      realOutputAction = action;
      realOutput.acceptInput("Trigger");
    }
    realOutputAction = -1;
    if (realOutputRows.length === 4) integrationRecord("js_output_delivery_and_suppression", {
      actions: realOutputRows.map(row => row.facts.action), deliveries: realOutputRows.length,
    }, [...realOutputRows], "owned logic_relay real engine OnTrigger callbacks; native owns original-count proof");
    realOutput.remove(); realOutput = null;
  }
}

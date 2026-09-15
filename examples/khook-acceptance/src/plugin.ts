// khook-acceptance — JS fixture for KHook suite A. NOT a shipped plugin.
//
// Uses only public APIs. Protocol: s2_khook_accept prepare|collect|report|teardown <run_id>.
// `report` is read-only. Competing command() registration for the probe token is forbidden;
// only command.onClientCommand observes it. The probe owns the engine ConCommand.
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

const TOKEN_CMD = "s2_khook_probe_token";
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
}

let runId = "";
let runBound = false;
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
let firstInvokePre = 0;
let firstInvokePost = 0;
let phaseStage4At = -1;
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
let freshDeliveries = 0;
let voiceApplied = false;
let voiceSpeaker = -1;
let voiceAllowed = -1;
let voiceDenied = -1;
let transmitPolicy = false;
let maskSubsetSeen = false;
let maskAllSeen = false;
let handledSetRecipients = false;
let clientActions: string[] = [];

function sourceRevision(): string {
  const v = Server.getCvar("s2_khook_source_revision");
  return v && v.length > 0 ? v : "unknown";
}

function tokenOf(argString: string): string {
  return (argString || "").trim().split(/\s+/)[0] || "";
}

function emit(rec: Rec): string {
  return JSON.stringify({
    schema: 1,
    suite: "A",
    run_id: runId || "",
    source_revision: sourceRevision(),
    case: rec.case,
    subcheck: rec.subcheck,
    producer: "js",
    result: rec.result,
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
  stored.push({
    case: cse,
    subcheck: sub,
    producer: "js",
    result,
    expected,
    actual,
    evidence,
  });
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
  if (sawReload) {
    if (entA && entity.index === entA.index && entity.id === entA.id) {
      freshDeliveries += 1;
    }
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
  if (!phaseEnt || !phaseEnt.isValid()) return;
  if (phaseStage === 0) {
    if (phasePre >= 1 && phasePost >= 1) {
      snapPhase("subscribe_pre_post");
      SDKUnhook(phaseEnt, SDKHookType.Touch, onPhasePre);
      preHooked = false;
      phaseStage = 1;
    }
    writePhaseStage();
    return;
  }
  if (phaseStage === 1) {
    if (phasePre === 0 && phasePost >= 1) {
      snapPhase("remove_pre");
      if (!preHooked) {
        preHooked = SDKHook(phaseEnt, SDKHookType.Touch, onPhasePre);
      }
      if (postHooked) {
        SDKUnhook(phaseEnt, SDKHookType.TouchPost, onPhasePost);
        postHooked = false;
      }
      phaseStage = 2;
    }
    writePhaseStage();
    return;
  }
  if (phaseStage === 2) {
    if (phasePre >= 1 && phasePost === 0) {
      snapPhase("remove_post");
      if (!postHooked) {
        postHooked = SDKHook(phaseEnt, SDKHookType.TouchPost, onPhasePost);
      }
      selfUnsubArmed = true;
      firstInvokePre = 0;
      firstInvokePost = 0;
      phaseStage = 3;
    }
    writePhaseStage();
    return;
  }
  if (phaseStage === 3) {
    if (firstInvokePre === 0 && phasePre >= 1) {
      firstInvokePre = phasePre;
      firstInvokePost = phasePost;
      phasePre = 0;
      phasePost = 0;
      selfUnsubArmed = false;
      writePhaseStage();
      return;
    }
    if (firstInvokePre >= 1 && phasePre === 0 && phasePost >= 1) {
      phaseSnaps.self_unsubscribe = {
        pre: firstInvokePre,
        post: firstInvokePost,
      };
      phaseSnaps.self_unsubscribe_second = { pre: phasePre, post: phasePost };
      if (preHooked) {
        SDKUnhook(phaseEnt, SDKHookType.Touch, onPhasePre);
        preHooked = false;
      }
      if (postHooked) {
        SDKUnhook(phaseEnt, SDKHookType.TouchPost, onPhasePost);
        postHooked = false;
      }
      phasePre = 0;
      phasePost = 0;
      phaseStage4At = r6Frames;
      phaseStage = 4;
    }
    writePhaseStage();
    return;
  }
  if (phaseStage === 4) {
    if (r6Frames > phaseStage4At && phasePre === 0 && phasePost === 0) {
      snapPhase("final_unsubscribe");
      phaseStage = 5;
    }
    writePhaseStage();
  }
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
      ": speaker talks now (allowed hears / denied silent). After collect of denied phase, operator Voice.reset via teardown/unmuting collect.",
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
      ": both stand in PVS of the green S2KHOOK TRANSMIT text. A should see it, B should not. Then sm / collect restore.",
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
  const subExp = { pre: 1, post: 1 };
  const rmPre = { pre: 0, post: 1 };
  const rmPost = { pre: 1, post: 0 };
  const selfExp = { first_pre: 1, second_pre: 0, second_post: 1 };
  const finExp = { pre: 0, post: 0 };
  const adapter =
    "need live engine: Touch invoke through the actual SDKHooks adapter (CTriggerPush::Touch PRE+POST); " +
    "Dummy Virtuals are supporting evidence only";
  if (phaseStage < 0) {
    pending("sdkhooks_phase_removal", "js_phase_subscribe_pre_post", subExp, "SDKHook Touch/TouchPost registration failed");
    pending("sdkhooks_phase_removal", "js_phase_remove_pre", rmPre, "SDKHook Touch/TouchPost registration failed");
    pending("sdkhooks_phase_removal", "js_phase_remove_post", rmPost, "SDKHook Touch/TouchPost registration failed");
    pending("sdkhooks_phase_removal", "js_phase_self_unsubscribe", selfExp, "SDKHook Touch/TouchPost registration failed");
    pending("sdkhooks_phase_removal", "js_phase_final_unsubscribe", finExp, "SDKHook Touch/TouchPost registration failed");
    return;
  }
  const sub = phaseSnaps.subscribe_pre_post;
  if (sub && sub.pre >= 1 && sub.post >= 1) {
    push("sdkhooks_phase_removal", "js_phase_subscribe_pre_post", "pass", subExp, subExp, "PRE+POST after subscribe");
  } else {
    pending("sdkhooks_phase_removal", "js_phase_subscribe_pre_post", subExp, adapter);
  }
  const rp = phaseSnaps.remove_pre;
  if (rp && rp.pre === 0 && rp.post >= 1) {
    push("sdkhooks_phase_removal", "js_phase_remove_pre", "pass", rmPre, rmPre, "PRE removed, POST survives");
  } else {
    pending("sdkhooks_phase_removal", "js_phase_remove_pre", rmPre, adapter);
  }
  const ro = phaseSnaps.remove_post;
  if (ro && ro.pre >= 1 && ro.post === 0) {
    push("sdkhooks_phase_removal", "js_phase_remove_post", "pass", rmPost, rmPost, "POST removed, PRE survives");
  } else {
    pending("sdkhooks_phase_removal", "js_phase_remove_post", rmPost, adapter);
  }
  const su = phaseSnaps.self_unsubscribe;
  const su2 = phaseSnaps.self_unsubscribe_second;
  if (su && su2 && su.pre >= 1 && su2.pre === 0 && su2.post >= 1) {
    push("sdkhooks_phase_removal", "js_phase_self_unsubscribe", "pass", selfExp, selfExp, "PRE self-unsub; second invoke POST only");
  } else {
    pending("sdkhooks_phase_removal", "js_phase_self_unsubscribe", selfExp, adapter);
  }
  const fin = phaseSnaps.final_unsubscribe;
  if (phaseStage >= 5 && fin && fin.pre === 0 && fin.post === 0) {
    push("sdkhooks_phase_removal", "js_phase_final_unsubscribe", "pass", finExp, finExp, "both phases removed");
  } else {
    pending("sdkhooks_phase_removal", "js_phase_final_unsubscribe", finExp, adapter);
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
      "need post-map Touch invoke via live EntByIndex after changelevel; " +
        "zero callbacks without an invoke is not a pass",
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
  const freshExp = { fresh: true, count: 1 };
  if (!sawReload) {
    pending(
      "entity_slot_reuse_map_teardown",
      "js_fresh_subscription_after_reload",
      freshExp,
      "need s2script unload/reload with the probe peer still loaded (R2 pending/retry); then collect",
    );
  } else if (filterHooked && freshDeliveries === 1) {
    push(
      "entity_slot_reuse_map_teardown",
      "js_fresh_subscription_after_reload",
      "pass",
      freshExp,
      freshExp,
      "fresh SDKHook delivered once after reload",
    );
  } else if (filterHooked && freshDeliveries === 0) {
    pending(
      "entity_slot_reuse_map_teardown",
      "js_fresh_subscription_after_reload",
      freshExp,
      "fresh SDKHook registered; need one Touch delivery on the new subscription",
    );
  } else {
    push(
      "entity_slot_reuse_map_teardown",
      "js_fresh_subscription_after_reload",
      "fail",
      freshExp,
      { fresh: filterHooked, count: freshDeliveries },
      "fresh subscription missing or did not deliver once",
    );
  }
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
  pending("sdkhooks_phase_removal", "js_phase_self_unsubscribe", { first_pre: 1, second_pre: 0, second_post: 1 }, "report before collect");
  pending("sdkhooks_phase_removal", "js_phase_final_unsubscribe", { pre: 0, post: 0 }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_identity_persisted", { persisted: true }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_slot_reuse_no_stale", { stale: false }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_map_teardown_clears", { cleared: true }, "report before collect");
  pending("entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload", { fresh: true, count: 1 }, "report before collect");
  pending("check_transmit", "js_visibility_policy", { a: true, b: false }, "report before collect");
}

function collectJs(): void {
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
  if (jsContinue >= 1) {
    push("frame_client_command_hooks", "js_command_continue_delivery", "pass", contExp, contExp, "continue token");
  } else {
    pending(
      "frame_client_command_hooks",
      "js_command_continue_delivery",
      contExp,
      "need a real client to issue s2_khook_probe_token s2khook-continue",
    );
  }
  const handExp = { js: 1 };
  if (jsHandled >= 1) {
    push("frame_client_command_hooks", "js_command_handled_delivery", "pass", handExp, handExp, "handled token");
  } else {
    pending(
      "frame_client_command_hooks",
      "js_command_handled_delivery",
      handExp,
      "need a real client to issue s2_khook_probe_token s2khook-handled",
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
  firstInvokePre = 0;
  firstInvokePost = 0;
  phaseStage4At = -1;
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
  freshDeliveries = 0;
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
}

function prepareR6(): void {
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

  if (sawReload && Server.getCvar("s2_khook_accept_unloaded") === "1") {
    const rid = Server.getCvar("s2_khook_accept_run");
    if (rid) {
      runId = rid;
      runBound = true;
      spawnPair();
      persistOutsidePlugin();
    }
  }

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

  command.onClientCommand(TOKEN_CMD, (slot, argString) => {
    const tok = tokenOf(argString);
    if (!hookEnabled || tok === CTRL_MISSING) {
      return HookResult.Continue;
    }
    if (tok === CTRL_FLIP_CONTINUE) {
      return HookResult.Handled;
    }
    if (tok === CTRL_FLIP_HANDLED) {
      return HookResult.Continue;
    }
    if (tok === CONTINUE_TOKEN) {
      jsContinue += 1;
      lastSlot = slot;
      return flipped ? HookResult.Handled : HookResult.Continue;
    }
    if (tok === HANDLED_TOKEN) {
      jsHandled += 1;
      lastSlot = slot;
      return flipped ? HookResult.Continue : HookResult.Handled;
    }
    return HookResult.Continue;
  });

  command.server("s2_khook_accept", (cmd) => {
    const sub = (cmd.arg(0) || "").toLowerCase();
    const id = cmd.arg(1) || "";
    if (sub === "prepare") {
      if (!id) {
        cmd.reply("usage: s2_khook_accept prepare <run_id>");
        return HookResult.Handled;
      }
      runId = id;
      runBound = true;
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
      freshDeliveries = 0;
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
    if (sub === "teardown") {
      restoreTransmitVisibility();
      Voice.resetAll();
      maskMode = "off";
      cleanupOwned();
      cmd.reply("[khook-accept] teardown");
      return HookResult.Handled;
    }
    cmd.reply("usage: s2_khook_accept prepare|collect|report|teardown <run_id>");
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
  clientsConnected += 1;
  if (c && c.isValid()) {
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
  };
}

export function OnPluginEnd(): void {
  setCvar("s2_khook_accept_unloaded", "1");
  persistOutsidePlugin();
  cleanupOwned();
  console.log(`[khook-accept] unloading frames=${frames}`);
}

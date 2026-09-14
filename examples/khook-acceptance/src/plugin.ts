// khook-acceptance — JS fixture for KHook suite A. NOT a shipped plugin.
//
// Uses only public APIs. Records exact engine outcomes for the live runner
// (`scripts/test-khook-live.sh A`). Command `s2_khook_accept report` is read-only.
import {
  Clients,
  Events,
  HookResult,
  SDKHook,
  SDKHookType,
  SDKUnhook,
  Transmit,
  Voice,
  command,
  createEntity,
  hook,
} from "@s2script/sdk";
import type { Client, EntityRef } from "@s2script/sdk";

interface CaseRec {
  expected: string;
  actual: string;
  result: "pass" | "fail" | "pending";
}

const cases: Record<string, CaseRec> = {};

function setCase(name: string, expected: string, actual: string, result: CaseRec["result"]): void {
  cases[name] = { expected, actual, result };
}

function emit(name: string, rec: CaseRec): string {
  const esc = (s: string) =>
    s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n");
  return (
    `{"suite":"A","case":"${name}","expected":"${esc(rec.expected)}","actual":"${esc(rec.actual)}","result":"${rec.result}"}`
  );
}

let frames = 0;
let clientsConnected = 0;
let commandSaw = 0;
let commandContinued = 0;
let commandSuppressed = 0;
let spawnA = 0;
let spawnB = 0;
let spawnedAOk = false;
let spawnedBOk = false;
let thinkPre = 0;
let thinkPost = 0;
let destroyed = 0;
let mapStarts = 0;
let mapEnds = 0;
let eventPreHandled = 0;
let lastDontBroadcastIntent = "";

let entA: EntityRef | null = null;
let entB: EntityRef | null = null;
let thinkEnt: EntityRef | null = null;

function onSpawnA(entity: EntityRef): void {
  if (!entity || !entity.isValid()) return;
  if (entA && entity.index === entA.index) spawnA += 1;
  else if (entB && entity.index === entB.index) spawnB += 1;
}
function onThinkPre(entity: EntityRef): void {
  if (entity && entity.isValid()) thinkPre += 1;
}
function onThinkPost(entity: EntityRef): void {
  if (entity && entity.isValid()) thinkPost += 1;
}

function evaluateSdkhooksOneOfTwo(): void {
  const actual =
    `hooked A only; spawnA=${spawnA} spawnB=${spawnB} spawnedA=${spawnedAOk} spawnedB=${spawnedBOk}`;
  let result: CaseRec["result"] = "pending";
  if (spawnA >= 1 && spawnB === 0) {
    result = "pass";
  } else if (spawnedAOk && spawnedBOk && spawnA === 0 && spawnB === 0) {
    result = "pending";
  } else if (spawnB > 0) {
    result = "fail";
  }
  setCase(
    "sdkhooks_one_of_two_entities",
    "only the subscribed live entity dispatches",
    actual,
    result,
  );
}

export function OnPluginStart(): void {
  console.log("[khook-accept] loaded (test fixture, not shipped)");

  // Handled+mask is a different event than the native no-suppression sample
  // (`player_activate`). Do not return Handled on that name.
  hook.onPre("player_changename", () => {
    const humans = Clients.all().filter((c) => c.isValid() && !c.isBot);
    if (humans.length > 0) {
      Events.setRecipients(humans.map((c) => c.slot));
      lastDontBroadcastIntent = "handled+mask slots=" + humans.map((c) => c.slot).join(",");
    } else {
      lastDontBroadcastIntent = "handled+empty-mask (no human clients)";
    }
    eventPreHandled += 1;
    return HookResult.Handled;
  });

  command("khook_probe_ping", () => HookResult.Continue);
  command.onClientCommand("khook_probe_ping", () => {
    commandSaw += 1;
    if (commandContinued === 0) {
      commandContinued += 1;
      return HookResult.Continue;
    }
    commandSuppressed += 1;
    return HookResult.Handled;
  });

  command.server("s2_khook_accept", (cmd) => {
    const sub = (cmd.arg(0) || "").toLowerCase();
    if (sub === "prepare") {
      prepare();
      cmd.reply("[khook-accept] prepared");
      return HookResult.Handled;
    }
    if (sub === "report") {
      refreshJsCases();
      for (const name of SUITE_A) {
        const rec = cases[name];
        if (rec) cmd.reply(emit(name, rec));
      }
      return HookResult.Handled;
    }
    if (sub === "teardown") {
      teardown();
      cmd.reply("[khook-accept] teardown");
      return HookResult.Handled;
    }
    cmd.reply("usage: s2_khook_accept prepare|report|teardown");
    return HookResult.Handled;
  });

  refreshJsCases();
}

function prepare(): void {
  const a = createEntity("logic_relay");
  const b = createEntity("logic_relay");
  entA = a;
  entB = b;
  if (a) SDKHook(a, SDKHookType.Spawn, onSpawnA);
  spawnedAOk = a ? a.spawn() : false;
  spawnedBOk = b ? b.spawn() : false;
  evaluateSdkhooksOneOfTwo();

  thinkEnt = a && a.isValid() ? a : null;
  if (thinkEnt) {
    SDKHook(thinkEnt, SDKHookType.Think, onThinkPre);
    SDKHook(thinkEnt, SDKHookType.ThinkPost, onThinkPost);
  }

  setCase(
    "fire_event_no_suppression",
    "original FireEvent exactly once, normal broadcast",
    "JS does not fire or Handled-hook this case; native probe owns original vs PRE on player_activate",
    "pending",
  );

  const humans = Clients.all().filter((c) => c.isValid() && !c.isBot);
  if (humans.length >= 2) {
    const ok = Voice.setAudibleTo(humans[0].slot, [humans[1].slot]);
    const stats = Voice.stats();
    setCase(
      "voice_recall",
      "denied listen bit reaches original as false; original once; unmuted unchanged",
      `setAudibleTo=${ok} stats.rewrites=${stats ? stats.rewrites : "n/a"} — needs a real voice packet, not first-fire`,
      "pending",
    );
  } else {
    setCase(
      "voice_recall",
      "denied listen bit reaches original as false; original once; unmuted unchanged",
      `human clients=${humans.length}; need two real clients speaking`,
      "pending",
    );
  }

  if (a && a.isValid()) {
    const vis = Transmit.setVisibleTo(a, humans.length > 0 ? [humans[0].slot] : []);
    const ts = Transmit.stats();
    setCase(
      "check_transmit",
      "first-fire layout validation and intended recipient filtering verified",
      `setVisibleTo=${vis} snapshots=${ts.snapshots} bitsCleared=${ts.bitsCleared} — recipient filtering needs a human in PVS`,
      "pending",
    );
  }

  Events.fire("player_changename", { userid: 0, oldname: "khook-c", newname: "khook-d" }, false);
  setCase(
    "fire_event_handled_recipient_mask",
    "original once with expected broadcast flag; intended recipients only",
    `onPre Handled count=${eventPreHandled} intent=${lastDontBroadcastIntent || "none"}; JS-fired events may skip JS onPre`,
    "pending",
  );
}

function teardown(): void {
  if (thinkEnt && thinkEnt.isValid()) {
    SDKUnhook(thinkEnt, SDKHookType.Think, onThinkPre);
    SDKUnhook(thinkEnt, SDKHookType.ThinkPost, onThinkPost);
  }
  if (entA && entA.isValid()) {
    entA.remove();
  }
}

function refreshJsCases(): void {
  setCase(
    "frame_client_command_hooks",
    "GameFrame counters increment; client lifecycle delivery; command suppression/continuation",
    `frames=${frames} clientsConnected=${clientsConnected} commandSaw=${commandSaw} continued=${commandContinued} suppressed=${commandSuppressed} (JS named publics are not KHook-peer proof; native owns pass)`,
    "pending",
  );

  evaluateSdkhooksOneOfTwo();

  if (thinkEnt && thinkEnt.isValid() && thinkPre + thinkPost > 0) {
    setCase(
      "sdkhooks_phase_removal",
      "PRE then POST removal, reverse, and in-callback removal: remaining phase survives; no deadlock",
      `Think pre=${thinkPre} post=${thinkPost}; report is read-only — use s2_khook_accept teardown to unhook`,
      "pending",
    );
  } else if (!cases["sdkhooks_phase_removal"]) {
    setCase(
      "sdkhooks_phase_removal",
      "PRE then POST removal, reverse, and in-callback removal: remaining phase survives; no deadlock",
      `thinkPre=${thinkPre} thinkPost=${thinkPost}; no live Think yet`,
      "pending",
    );
  }

  if (entA && entA.isValid()) {
    setCase(
      "entity_slot_reuse_map_teardown",
      "no stale entity delivery; filters and retained bindings retire on delete/reuse/map/teardown",
      `A idx=${entA.index} stillValid=${entA.isValid()} destroyedPublics=${destroyed} mapStarts=${mapStarts} mapEnds=${mapEnds}; report is read-only`,
      "pending",
    );
  } else if (!cases["entity_slot_reuse_map_teardown"]) {
    setCase(
      "entity_slot_reuse_map_teardown",
      "no stale entity delivery; filters and retained bindings retire on delete/reuse/map/teardown",
      `destroyed=${destroyed} mapStarts=${mapStarts} mapEnds=${mapEnds}; map change / unload not yet observed`,
      "pending",
    );
  }
}

const SUITE_A = [
  "new_capsule_registration",
  "shared_capsule_registration",
  "peer_actions_both_orders",
  "one_normal_invocation",
  "frame_client_command_hooks",
  "fire_event_no_suppression",
  "fire_event_handled_recipient_mask",
  "voice_recall",
  "sdkhooks_one_of_two_entities",
  "sdkhooks_phase_removal",
  "entity_slot_reuse_map_teardown",
  "check_transmit",
];

export function OnGameFrame(): void {
  frames += 1;
}

export function OnClientConnected(_c: Client): void {
  clientsConnected += 1;
}

export function OnEntityDestroyed(_entity: EntityRef | null, _className: string): void {
  destroyed += 1;
}

export function OnMapStart(_map: string): void {
  mapStarts += 1;
}

export function OnMapEnd(): void {
  mapEnds += 1;
}

export function OnPluginEnd(): void {
  console.log(`[khook-accept] unloading frames=${frames} destroyed=${destroyed}`);
}

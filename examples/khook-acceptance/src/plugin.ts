// khook-acceptance — JS fixture for KHook suite A. NOT a shipped plugin.
//
// Uses only public APIs. Records exact engine outcomes for the live runner
// (`scripts/test-khook-live.sh A`). Command `s2_khook_accept report` prints one
// JSON object per suite A case the JS side can observe; cases that need real
// clients stay pending (a first-fire log is not mute/recipient proof).
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
let commandSuppressed = 0;
let spawnA = 0;
let spawnB = 0;
let thinkPre = 0;
let thinkPost = 0;
let destroyed = 0;
let mapStarts = 0;
let mapEnds = 0;
let eventPost = 0;
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

export function OnPluginStart(): void {
  console.log("[khook-accept] loaded (test fixture, not shipped)");

  hook.on("player_changename", () => {
    eventPost += 1;
  });
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

  // Register so a client (or fakeCommand) can invoke it; the listener is the
  // suppression/continuation observation the suite asserts.
  command("khook_probe_ping", () => HookResult.Continue);
  command.onClientCommand("khook_probe_ping", () => {
    commandSaw += 1;
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
    cmd.reply("usage: s2_khook_accept prepare|report");
    return HookResult.Handled;
  });

  refreshJsCases();
}

function prepare(): void {
  const a = createEntity("logic_relay");
  const b = createEntity("logic_relay");
  entA = a;
  entB = b;
  // Two live entities, only A subscribed — B is spawned as the negative control.
  if (a) SDKHook(a, SDKHookType.Spawn, onSpawnA);
  const spawnedA = a ? a.spawn() : false;
  const spawnedB = b ? b.spawn() : false;

  const onlyA = spawnA >= 1 && spawnB === 0;
  setCase(
    "sdkhooks_one_of_two_entities",
    "only the subscribed live entity dispatches",
    `hooked A only; spawnA=${spawnA} spawnB=${spawnB} spawnedA=${spawnedA} spawnedB=${spawnedB}`,
    a && b && spawnedA && spawnedB ? (onlyA ? "pass" : "fail") : "pending",
  );

  thinkEnt = a && a.isValid() ? a : null;
  if (thinkEnt) {
    SDKHook(thinkEnt, SDKHookType.Think, onThinkPre);
    SDKHook(thinkEnt, SDKHookType.ThinkPost, onThinkPost);
  }

  const fired = Events.fire("player_changename", { userid: 0, oldname: "khook-a", newname: "khook-b" }, false);
  setCase(
    "fire_event_no_suppression",
    "original FireEvent exactly once, normal broadcast",
    `Events.fire returned ${fired}; JS post-dispatch count=${eventPost} (JS-fired events may not re-enter JS on/onPre; native probe must count the original)`,
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

function refreshJsCases(): void {
  const frameOk = frames > 0;
  const clientOk = clientsConnected > 0;
  const cmdOk = commandSaw > 0 && commandSuppressed > 0;
  let fc: CaseRec["result"] = "pending";
  if (frameOk && clientOk && cmdOk) fc = "pass";
  setCase(
    "frame_client_command_hooks",
    "GameFrame counters increment; client lifecycle delivery; command suppression/continuation",
    `frames=${frames} clientsConnected=${clientsConnected} commandSaw=${commandSaw} suppressed=${commandSuppressed} (arm suppression with client command khook_probe_ping)`,
    fc,
  );

  if (thinkEnt && thinkEnt.isValid() && thinkPre + thinkPost > 0) {
    SDKUnhook(thinkEnt, SDKHookType.Think, onThinkPre);
    const preAfterUnhook = thinkPre;
    setCase(
      "sdkhooks_phase_removal",
      "PRE then POST removal, reverse, and in-callback removal: remaining phase survives; no deadlock",
      `Think pre=${thinkPre} post=${thinkPost} after PRE unhook pre still ${preAfterUnhook}; in-callback/reverse orders need a live pawn Think`,
      thinkPost > 0 ? "pending" : "pending",
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
    const idx = entA.index;
    const gone = entA.remove();
    setCase(
      "entity_slot_reuse_map_teardown",
      "no stale entity delivery; filters and retained bindings retire on delete/reuse/map/teardown",
      `removed A idx=${idx} ok=${gone} stillValid=${entA.isValid()} destroyedPublics=${destroyed} mapStarts=${mapStarts} mapEnds=${mapEnds}`,
      gone && !entA.isValid() ? "pending" : "pending",
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

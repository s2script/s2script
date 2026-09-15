// khook-acceptance — JS fixture for KHook suite A. NOT a shipped plugin.
//
// Uses only public APIs. Protocol: s2_khook_accept prepare|collect|report|teardown <run_id>.
// `report` is read-only. Competing command() registration for the probe token is forbidden;
// only command.onClientCommand observes it. The probe owns the engine ConCommand.
import { HookResult, Server, command } from "@s2script/sdk";
import type { Client } from "@s2script/sdk";

const TOKEN_CMD = "s2_khook_probe_token";
const CONTINUE_TOKEN = "s2khook-continue";
const HANDLED_TOKEN = "s2khook-handled";
const CTRL_MISSING = "s2khook-ctrl-missing";
const CTRL_FLIP_CONTINUE = "s2khook-ctrl-flip-continue";
const CTRL_FLIP_HANDLED = "s2khook-ctrl-flip-handled";

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

function pushR6Pending(): void {
  pending(
    "fire_event_handled_recipient_mask",
    "js_handled_set_recipients",
    { handled: true, subset: true },
    "R6: recipient-mask client exercise not implemented",
  );
  pending("voice_recall", "js_voice_policy_applied", { applied: true }, "R6: voice policy not implemented");
  pending(
    "sdkhooks_one_of_two_entities",
    "js_hook_a_delivered",
    { spawnA: 1 },
    "R6: SDKHooks entity filter not implemented",
  );
  pending(
    "sdkhooks_one_of_two_entities",
    "js_hook_b_filtered",
    { spawnB: 0 },
    "R6: SDKHooks entity filter not implemented",
  );
  pending(
    "sdkhooks_phase_removal",
    "js_phase_subscribe_pre_post",
    { pre: 1, post: 1 },
    "R6: SDKHooks phase removal not implemented",
  );
  pending("sdkhooks_phase_removal", "js_phase_remove_pre", { pre: 0, post: 1 }, "R6: SDKHooks phase removal not implemented");
  pending("sdkhooks_phase_removal", "js_phase_remove_post", { pre: 1, post: 0 }, "R6: SDKHooks phase removal not implemented");
  pending(
    "sdkhooks_phase_removal",
    "js_phase_self_unsubscribe",
    { second_pre: 0 },
    "R6: SDKHooks phase removal not implemented",
  );
  pending(
    "sdkhooks_phase_removal",
    "js_phase_final_unsubscribe",
    { pre: 0, post: 0 },
    "R6: SDKHooks phase removal not implemented",
  );
  pending(
    "entity_slot_reuse_map_teardown",
    "js_identity_persisted",
    { persisted: true },
    "R6: slot reuse/map/teardown not implemented",
  );
  pending(
    "entity_slot_reuse_map_teardown",
    "js_slot_reuse_no_stale",
    { stale: false },
    "R6: slot reuse/map/teardown not implemented",
  );
  pending(
    "entity_slot_reuse_map_teardown",
    "js_map_teardown_clears",
    { cleared: true },
    "R6: slot reuse/map/teardown not implemented",
  );
  pending(
    "entity_slot_reuse_map_teardown",
    "js_fresh_subscription_after_reload",
    { fresh: true },
    "R6: unload/reload not implemented",
  );
  pending("check_transmit", "js_visibility_policy", { a: true, b: false }, "R6: check_transmit not implemented");
}

function pushInvalid(requested: string): void {
  const expected = { run_id: runId };
  const actual = { run_id: requested, error: "unknown_or_mismatched_run" };
  const evd = "unknown or mismatched run_id; never reuse a prior run";
  const owned: Array<[string, string]> = [
    ["frame_client_command_hooks", "js_gameframe_delivery"],
    ["frame_client_command_hooks", "js_client_connected"],
    ["frame_client_command_hooks", "js_client_identity_join"],
    ["frame_client_command_hooks", "js_command_continue_delivery"],
    ["frame_client_command_hooks", "js_command_handled_delivery"],
    ["fire_event_no_suppression", "js_no_handled_on_unsuppressed_event"],
  ];
  for (const [cse, sub] of owned) {
    push(cse, sub, "fail", expected, actual, evd);
  }
  pushR6Pending();
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
  pushR6Pending();
  collected = true;
}

export function OnPluginStart(): void {
  console.log("[khook-accept] loaded (test fixture, not shipped)");
  Server.registerCvar("s2_khook_accept_run", { type: "string", default: "", help: "khook-accept bound run_id" });

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
      Server.setCvar("s2_khook_accept_run", id);
      cmd.reply("[khook-accept] prepared run_id=" + id);
      return HookResult.Handled;
    }
    if (sub === "collect") {
      if (!id) {
        cmd.reply("usage: s2_khook_accept collect <run_id>");
        return HookResult.Handled;
      }
      if (!runBound || runId !== id) {
        const prev = runId;
        const prevStored = stored;
        runId = id;
        stored = [];
        pushInvalid(id);
        for (const rec of stored) cmd.reply(emit(rec));
        stored = prevStored;
        runId = prev;
        return HookResult.Handled;
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
        const prev = runId;
        const prevStored = stored;
        runId = id;
        stored = [];
        pushInvalid(id);
        for (const rec of stored) cmd.reply(emit(rec));
        stored = prevStored;
        runId = prev;
        return HookResult.Handled;
      }
      if (!collected) {
        collectJs();
      }
      for (const rec of stored) cmd.reply(emit(rec));
      return HookResult.Handled;
    }
    if (sub === "teardown") {
      cmd.reply("[khook-accept] teardown");
      return HookResult.Handled;
    }
    cmd.reply("usage: s2_khook_accept prepare|collect|report|teardown <run_id>");
    return HookResult.Handled;
  });
}

export function OnGameFrame(): void {
  frames += 1;
}

export function OnClientConnected(c: Client): void {
  clientsConnected += 1;
  if (c && c.isValid()) {
    lastSlot = c.slot;
    lastUserId = c.userId;
    lastSteamId = c.steamId;
  }
}

export function OnPluginEnd(): void {
  console.log(`[khook-accept] unloading frames=${frames}`);
}

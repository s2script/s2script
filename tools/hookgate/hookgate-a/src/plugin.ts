// hookgate-a — live-gate fixture for declarative inbound hooks. NOT a shipped plugin.
//
// Drives the six checks in docs/superpowers/specs/2026-08-02-declarative-inbound-hooks-design.md §9.
// The one that matters most is the BYPASS: `ctx.gameRules.onTerminateRound` must NOT fire when a
// plugin calls `GameRules.terminateRound()` — only when the ENGINE ends a round. That is
// SourceMod's exact semantic (g_pIgnoreTerminateDetour), and it is also what keeps a hook from
// firing while core holds the V8 isolate borrow, where a pre-hook could not be delivered at all.
//
// Prefix [HOOKGATE] so `docker logs | grep HOOKGATE` reads as a transcript.
import { plugin } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
import { GameRules, RoundEndReason } from "@s2script/cs2";

export default plugin((ctx) => {
  const L = (m: string) => console.log(`[HOOKGATE] ${m}`);
  L("loaded");

  let frame = 0;
  ctx.server.onGameFrame(() => { frame += 1; });

  // What the next onTerminateRound should do. Set by the commands below so one boot can exercise
  // observe / suppress / mutate without a reload.
  let mode: "observe" | "suppress" | "mutate" = "observe";
  let engineFired = 0;
  let afterOwnCall = 0;   // fires attributable to OUR OWN terminateRound() — must stay 0

  // ------------------------------------------------------------------ checks 1, 3, 4
  ctx.gameRules.onTerminateRound((ev) => {
    engineFired += 1;
    L(`onTerminateRound #${engineFired}: reason=${ev.reason} delay=${ev.delay} frame=${frame} mode=${mode}`);

    if (mode === "suppress") {
      mode = "observe";
      L("  -> returning Handled: the round must NOT end");
      return HookResult.Handled;
    }
    if (mode === "mutate") {
      mode = "observe";
      // A reason the engine will report back in round_end, so the substitution is observable.
      ev.reason = RoundEndReason.TerroristsWin;
      ev.delay = 1.0;
      L(`  -> rewrote reason=${ev.reason} delay=${ev.delay}, returning Changed`);
      return HookResult.Changed;
    }
    return HookResult.Continue;
  });

  // ------------------------------------------------------------------ check 6
  // The engine respawns players at round start; that must fire. A JS player.respawn() must not.
  let respawns = 0;
  ctx.players.onRespawn((ev) => {
    respawns += 1;
    // ev.player must be a real EntityRef — a bare array here was a real defect this slice fixed.
    const ok = ev.player && typeof ev.player.isValid === "function";
    L(`onRespawn #${respawns}: player=${ok ? `EntityRef(idx=${ev.player!.index} valid=${ev.player!.isValid()})` : "NOT AN EntityRef (FAIL)"} frame=${frame}`);
  });

  // ------------------------------------------------------------------ check 2: the bypass
  // Our own call must NOT reach our own handler. If the counter moves across this command, the
  // latch is not arming and the SourceMod-parity semantic is broken.
  ctx.commands.registerServer("hook_own_call", () => {
    const before = engineFired;
    L(`hook_own_call: firing our own terminateRound at frame=${frame} (handler count=${before})`);
    const r = GameRules.terminateRound(RoundEndReason.CTsWin, 5);
    L(`hook_own_call: terminateRound returned ${r}`);
    // Checked on the next command, since the engine call is queued to next frame.
    afterOwnCall = before;
  });

  ctx.commands.registerServer("hook_report", () => {
    const bypassHeld = engineFired === afterOwnCall;
    L(`REPORT engineFired=${engineFired} respawns=${respawns} frame=${frame}`);
    L(`REPORT bypass ${bypassHeld ? "HELD (our own call did not fire our hook)" : "BROKEN (our own call fired our hook)"}`);
  });

  ctx.commands.registerServer("hook_suppress", () => { mode = "suppress"; L("armed: next onTerminateRound returns Handled"); });
  ctx.commands.registerServer("hook_mutate",   () => { mode = "mutate";   L("armed: next onTerminateRound rewrites reason+delay"); });

  return { onUnload() { L(`unloading (engineFired=${engineFired} respawns=${respawns})`); } };
});

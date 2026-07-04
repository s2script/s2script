import { Events, HookResult } from "@s2script/cs2";

// Slice 5D.3 live gate — event actionability (block + modify + fire).
//  BLOCK + MODIFY: a pre-hook on round_start (deterministic via mp_restartgame — the same FireEvent
//   mechanism as any game event, without needing combat) sets timelimit to a sentinel (MODIFY) then
//   returns Handled (BLOCK = suppress client broadcast). The POST handler still fires server-side and
//   reads the modified value — proving the hook fires on real engine events, the modify took effect,
//   and a Handled pre-hook suppresses broadcast without stopping server-side delivery (SM parity).
//  FIRE: from onLoad we synthesize a player_hurt (a benign notification); ok=true proves the fire
//   plumbing (CreateEvent + FireEvent). Note: a JS-triggered fire cannot re-dispatch to JS subscribers
//   (the isolate is already borrowed while any JS runs) — the engine-side fire still happens; a nested
//   re-dispatch is skipped by design (no panic — the try_borrow guard). See CLAUDE/README.
export function onLoad(): void {
  console.log("[demo] onLoad (5D.3 event actionability)");

  Events.onPre("round_start", (ev) => {
    const tl = ev.getInt("timelimit");
    ev.setInt("timelimit", 4242);                  // MODIFY
    console.log("[demo] PRE round_start timelimit " + tl + "->" + ev.getInt("timelimit") + " (Handled)");
    return HookResult.Handled;                      // BLOCK the client broadcast
  });

  Events.on("round_start", (ev) => {
    // Expect timelimit=4242 — proves the pre-hook's modify reached the server-side POST, and that a
    // Handled pre-hook did NOT stop server-side delivery (broadcast-suppress, not full block).
    console.log("[demo] POST round_start timelimit=" + ev.getInt("timelimit"));
  });

  const ok = Events.fire("player_hurt",
    { userid: 0, attacker: 0, dmg_health: 100, weapon: "s2script_fired" }, true);
  console.log("[demo] fired player_hurt (from onLoad) ok=" + ok);
}

export function onUnload(): void { console.log("[demo] onUnload"); }

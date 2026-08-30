// ddqgate-a — live-gate fixture A for the deferred-dispatch queue. NOT a shipped plugin.
//
// Drives the three checks in docs/superpowers/specs/2026-08-01-deferred-dispatch-queue-design.md §5.
// Pair with ddqgate-b, which subscribes to the INNER event from a SECOND plugin context — that
// cross-plugin observation is the whole point. Before this slice the inner dispatch was silently
// dropped, so b never ran at all.
//
// Every line is prefixed [DDQ-A] so `docker logs | grep DDQ` reads as a transcript.
import { Events } from "@s2script/sdk/events";
import { Player } from "@s2script/cs2";
import { hook } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";

// The deferred-dispatch selftest native. DELIBERATELY not in @s2script/sdk's .d.ts and never will
// be: core installs it only in a process started with S2_DEFER_SELFTEST set, so in any normal
// server it does not exist on the global at all. Declared here as possibly-undefined precisely so
// the code below has to prove it is there before calling it — which is also the gate's first check.
declare const __s2_defer_selftest: (() => number) | undefined;

export function OnPluginStart(): void {
  const L = (m: string) => console.log(`[DDQ-A] ${m}`);
  L("loaded");

  // A frame counter, so the log proves WHEN the deferred delivery lands relative to the defer.
  // The spec says one frame later (§4) — not the same frame, and not never.
  let frame = 0;
  hook.gameFrame(() => { frame += 1; });

  // ------------------------------------------------------------------ check 1 + 2
  // The outer handler fires an inner event from INSIDE a dispatch. The engine dispatches that
  // synchronously back into core while core still holds HOST.borrow_mut(), so the inner dispatch
  // re-enters, fails try_borrow_mut, and is deferred.
  //
  // The fields are the point. A queued dispatch that lost its IGameEvent would replay against a
  // dead s_currentEvent and every getInt/getString would return its DEFAULT — b would log 0 and "".
  // Non-default values in b's line are the proof that DuplicateEvent preserved the payload.
  let fired = 0;
  const fireInner = (why: string) => {
    fired += 1;
    L(`firing ddq_probe #${fired} from ${why} at frame=${frame} (expect b at frame=${frame + 1})`);
    // A REAL catalog event. A forced custom name (CreateEvent bForce) can be FIRED, but
    // AddListener never registers a listener for a name the game does not define, so the second
    // plugin would never be dispatched to and there would be nothing to defer. Learned the hard way.
    Events.fire("player_changename", {
      userid: 0,
      oldname: `probe-${fired}`,
      newname: `frame-${frame}`,
    });
  };

  let armedSlay = false;
  // The slay happens INSIDE this handler, i.e. inside dispatch_game_event's borrow.
  hook.event("player_changename", () => {
    if (!armedSlay) return;
    armedSlay = false;
    const victim = Player.all().find((p) => (p.pawn?.health ?? 0) > 0);
    if (!victim || !victim.pawn) { L("slay: no live pawn"); return; }
    L(`slay: killing a live pawn from INSIDE a dispatch at frame=${frame}`);
    victim.pawn.slay();
    L("slay: returned — player_death should have deferred");
  });

  hook.event("round_start", () => {
    L(`round_start at frame=${frame}`);
    fireInner("round_start");
  });

  // On-demand trigger, so the gate does not depend on waiting for a round.
  command.server("ddq_fire", () => {
    // Fired from a command handler — also inside a dispatch, so also a re-entrancy.
    fireInner("ddq_fire command");
  });

  // ------------------------------------------------------------------ check 3
  // A deferred handler that THROWS must still free the duplicated event. The throw is isolated by
  // fan_out's per-handler TryCatch, so the only observable failure is a leak — which shows up as
  // the server's RSS climbing across repeated ddq_throw, not as an error. Fire a burst so a leak
  // would be measurable.
  command.server("ddq_throw", () => {
    L(`throw-path burst of 20 at frame=${frame}`);
    for (let i = 0; i < 20; i++) {
      Events.fire("player_changename", { userid: 0, oldname: `throw-${i}`, newname: "throw" });
    }
  });

  // ------------------------------------------------------------- the REAL re-entrancy
  // Events.fire() turned out NOT to re-enter: the engine does not dispatch a JS-fired event back
  // to our listener inside the borrow. The case the queue actually exists for is an ENGINE CALL
  // that fires an event synchronously — which is exactly Respawn/TerminateRound in A5b. slay() is
  // CommitSuicide, and it fires player_death inline, so calling it from INSIDE an event handler
  // reproduces that shape today, without waiting for A5b.
  command.server("ddq_slay", () => {
    const victims = Player.all().filter((p) => (p.pawn?.health ?? 0) > 0);
    L(`ddq_slay: ${victims.length} live pawn(s) at frame=${frame}`);
    if (victims.length === 0) return;
    // Slay from inside a round_start-style dispatch by going through an event handler instead of
    // doing it here: arm it, then let the next fired event do the killing.
    armedSlay = true;
    L("armed — slaying from inside the next player_changename handler");
    Events.fire("player_changename", { userid: 0, oldname: "arm", newname: "arm" });
  });

  // ------------------------------------------------- the PROVABLE re-entrancy (ddq_selftest)
  // Both triggers above were tried on a live server and neither re-enters:
  //   * Events.fire() from a handler is delivered SYNCHRONOUSLY — CS2 does not route a JS-fired
  //     event back through our listener inside the borrow.
  //   * ddq_slay's player_death DOES land a frame later, but from the engine's OWN next-frame
  //     delivery, not our drain. The tell: the drain runs at the top of Hook_GameFramePre, BEFORE
  //     the frame dispatch that increments the counters above, so a drained delivery reads the OLD
  //     value of b's counter. It read the new one.
  // The genuine trigger is an engine call that fires an event synchronously inside the borrow —
  // i.e. A5b's Respawn/TerminateRound, which do not exist yet. So the shim ships a synthetic one,
  // gated on S2_DEFER_SELFTEST, in the same spirit as S2_DAMAGE_SELFTEST. This command is its only
  // caller: being inside a JS native is what puts core in the borrow the selftest needs.
  let selftests = 0;
  command.server("ddq_selftest", (c) => {
    if (typeof __s2_defer_selftest !== "function") {
      const m = "ddq_selftest: native ABSENT — the server was not started with S2_DEFER_SELFTEST set";
      L(m);
      c.reply(m);
      return;
    }
    selftests += 1;
    L(`ddq_selftest #${selftests} at frame=${frame} — calling the gated native`);
    const r = __s2_defer_selftest();
    // 1 = the dispatch reported DEFERRED and a duplicate was queued (the path under test ran)
    // 0 = refused/degraded (shim gate off, no event manager, CreateEvent or duplication failed)
    // -1 = NOT deferred — the isolate was free, or nothing subscribes to player_changename
    const verdict = r === 1 ? "DEFERRED+QUEUED (expect DDQ-B one frame later)"
                  : r === 0 ? "REFUSED/DEGRADED — read the shim's [s2script] defer self-test lines"
                            : "NOT DEFERRED — this run proves nothing";
    L(`ddq_selftest #${selftests}: native returned ${r} — ${verdict}`);
    c.reply(`ddq_selftest #${selftests}: ${r} — ${verdict}`);
  });

  command.server("ddq_report", () => {
    L(`REPORT fired=${fired} selftests=${selftests} frame=${frame}`);
  });
}

export function OnPluginEnd(): void {
  console.log("[DDQ-A] unloading");
}

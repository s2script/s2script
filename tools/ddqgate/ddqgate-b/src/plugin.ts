// ddqgate-b — live-gate fixture B for the deferred-dispatch queue. NOT a shipped plugin.
//
// The SECOND plugin context. It subscribes to the event ddqgate-a fires from inside a dispatch.
// Before this slice that inner dispatch was dropped by the try_borrow_mut guard and this plugin
// never ran — the failure presented only as "my plugin stopped getting events".
//
// What this plugin logs IS the gate:
//   - it running at all      -> the deferred dispatch was delivered, not dropped
//   - seq/frame/tag non-default -> DuplicateEvent preserved the payload (a lost event would replay
//                                 against a dead s_currentEvent and read every field as its default)
//   - its frame vs a's frame -> the one-frame delivery the spec promises (§4)
import { hook } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  const L = (m: string) => console.log(`[DDQ-B] ${m}`);
  L("loaded");

  let frame = 0;
  hook.server.onGameFrame(() => { frame += 1; });

  let seen = 0, thrown = 0, selftests = 0;
  hook.event("player_changename", (e) => {
    const oldname = e.getString("oldname");
    const newname = e.getString("newname");

    // Check 3: the throw burst. A deferred handler that throws must still free its duplicated
    // event. fan_out isolates the throw per-handler, so the only observable failure is a leak.
    if (oldname.startsWith("throw-")) {
      thrown += 1;
      throw new Error(`ddqgate-b deliberate throw #${thrown}`);
    }

    // The shim's gated selftest (`ddq_selftest`). This is the ONE arrival that is unambiguously a
    // DRAIN delivery: nothing else in the game fires player_changename with these fields, and the
    // shim printed the frame it deferred on. `newname` carries "selftest-<seq>@frame-<N>" so this
    // line can be read against the shim's own "DRAIN replaying ... at frame N+1" line without
    // trusting either counter alone.
    //
    // The JS-side frame reading is the OPPOSITE of intuition and is the whole reason this branch
    // logs it: the drain runs at the TOP of Hook_GameFramePre, BEFORE the frame dispatch that
    // increments `frame` below — so a DRAINED delivery reads the SAME `frame` value that was
    // current when the defer happened (delta 0), while an engine-delivered next-frame event reads
    // the incremented one (delta 1). Delta 0 here is the drain's signature, not a bug.
    if (oldname === "ddq-selftest") {
      selftests += 1;
      const at = Number(newname.replace(/^selftest-\d+@frame-/, ""));
      // The shim stamps BOTH an int field (userid = the sequence number, deliberately non-zero
      // because 0 is also the int default) and two string fields. All three surviving the
      // duplication is the payload check; any of them reading back as its default is the failure.
      const seq = e.getInt("userid");
      const verdict = newname === "" || seq === 0 ? "ZEROED-PAYLOAD (FAIL)" : "payload OK";
      L(`selftest #${selftests}: userid=${seq} oldname="${oldname}" newname="${newname}" ` +
        `deferredAtShimFrame=${Number.isNaN(at) ? "?" : at} nowJsFrame=${frame} — ${verdict}`);
      return;
    }

    seen += 1;
    // A dispatch that lost its IGameEvent replays against a dead s_currentEvent, so every field
    // reads as its DEFAULT — oldname and newname would both be "". That is the failure this
    // fixture exists to catch, and it is why the payload is carried in the strings.
    const at = Number(newname.replace("frame-", ""));
    const verdict = oldname === "" && newname === "" ? "ZEROED-PAYLOAD (FAIL)" : "payload OK";
    L(`probe #${seen}: oldname="${oldname}" newname="${newname}" nowFrame=${frame} delta=${frame - at} — ${verdict}`);
  });

  // The deferred delivery that matters: player_death fired by an engine call made from inside a
  // dispatch. A lost payload reads userid=0 here.
  let deaths = 0;
  hook.event("player_death", (e) => {
    deaths += 1;
    L(`player_death #${deaths}: userid=${e.getInt("userid")} attacker=${e.getInt("attacker")} weapon="${e.getString("weapon")}" nowFrame=${frame}`);
  });

  command.server("ddq_report_b", () => {
    L(`REPORT seen=${seen} selftests=${selftests} thrown=${thrown} deaths=${deaths} frame=${frame}`);
  });
}

export function OnPluginEnd(): void {
  console.log("[DDQ-B] unloading");
}

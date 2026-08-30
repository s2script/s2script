// detourgate-a — live-gate fixture for the two-tier detour slice. NOT a shipped plugin.
//
// The slice changes the PATCH WIDTH of every detour in the framework (14-byte absolute jump -> a
// 5-byte E9 into a near-allocated island). Offline tests prove the relocation arithmetic; only a
// real server proves the engine still runs through the patched prologues.
//
// So this subscribes to all four PRE-EXISTING detours and counts them. A count that stays 0 for a
// detour whose traffic definitely occurred is the regression this fixture exists to catch.
//
//   ProcessUsercmds     -> hook.runcmd            (bots generate usercmds every tick)
//   FireOutputInternal  -> hook.output            (map logic fires outputs each round)
//   DispatchTraceAttack -> hook.damage            (bots shoot each other)
//   HostSay             -> chat trigger           (needs a human; see the report note)
//
// Prefix [DETOURGATE] so `docker logs | grep DETOURGATE` reads as a transcript.
import { hook } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  const L = (m: string) => console.log(`[DETOURGATE] ${m}`);
  L("loaded — subscribing to all four pre-existing detours");

  let frames = 0;
  let usercmds = 0;
  let outputs = 0;
  let damages = 0;

  hook.gameFrame(() => { frames += 1; });

  // ProcessUsercmds. Lazy-installed on FIRST subscribe, so this subscription is what triggers the
  // install — the boot log should show it taking the near (E9) tier.
  hook.runcmd(() => { usercmds += 1; });

  // FireOutputInternal. A wildcard-ish subscription: round logic fires outputs on these every round.
  hook.output("*", "*", () => { outputs += 1; });

  // DispatchTraceAttack.
  hook.damage(() => { damages += 1; });

  command.server("detour_report", () => {
    L(`REPORT frames=${frames} usercmds=${usercmds} outputs=${outputs} damages=${damages}`);
    L(`  ProcessUsercmds    : ${usercmds > 0 ? "FIRING" : "NOT FIRING (bots should produce these every tick)"}`);
    L(`  FireOutputInternal : ${outputs > 0 ? "FIRING" : "no outputs seen yet"}`);
    L(`  DispatchTraceAttack: ${damages > 0 ? "FIRING" : "no damage seen yet"}`);
    L(`  HostSay            : not provable without a human client (see docs/superpowers deferred-live-tests)`);
  });
}

export function OnPluginEnd(): void {
  console.log("[DETOURGATE] unloading");
}

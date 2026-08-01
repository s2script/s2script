// a4gate-b — live-gate fixture B for the A4 dispatch chain (#55-#60). NOT a shipped plugin.
//
// The SECOND context on every channel a4gate-a subscribes. Two jobs:
//   - check 2: observe the damage chain from a different plugin, so "Handled did not truncate" is
//     proven ACROSS contexts and not just across two handlers in one closure.
//   - check 3: be the reload subject. It holds a live subscription on a per-tick channel
//     (onGameFrame), so touching its .s2sp exercises the generation token A4d folded into the handler
//     type while dispatch is actively running.
import { plugin } from "@s2script/sdk/plugin";

interface State { load: number; }

export default plugin((ctx) => {
  const load = ((ctx.previous as State | undefined)?.load ?? 0) + 1;
  const L = (m: string) => console.log(`[A4GATE-B#${load}] ${m}`);
  L("loaded");

  let dmg = 0, pre = 0, created = 0, runcmd = 0, frames = 0;

  ctx.entities.onDamage((info) => {
    dmg += 1;
    L(`dmg#${dmg} B RAN saw=${info.damage}`);
  });

  ctx.events.onPre("player_spawn", () => { pre += 1; if (pre <= 3) L(`onPre player_spawn #${pre}`); });
  ctx.entities.onCreate("*", () => { created += 1; });
  ctx.clients.onRunCmd(() => { runcmd += 1; if (runcmd === 1) L("onRunCmd FIRED (first)"); });

  // Per-tick heartbeat: the live subscription the reload has to survive. Logged sparsely so the
  // transcript stays readable — a gap in the sequence after a reload is the failure signal.
  ctx.server.onGameFrame(() => {
    frames += 1;
    if (frames % 512 === 0) L(`heartbeat frames=${frames} dmg=${dmg} pre=${pre} created=${created}`);
  });

  ctx.commands.registerServer("a4_report_b", () => {
    L(`REPORT dmg=${dmg} pre=${pre} created=${created} runcmd=${runcmd} frames=${frames}`);
  });

  return {
    onUnload() { L(`unloading (frames=${frames} dmg=${dmg})`); },
    state(): State { return { load }; },
  };
});

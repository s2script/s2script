// a4gate-b — live-gate fixture B for the A4 dispatch chain (#55-#60). NOT a shipped plugin.
//
// The SECOND context on every channel a4gate-a subscribes. Two jobs:
//   - check 2: observe the damage chain from a different plugin, so "Handled did not truncate" is
//     proven ACROSS contexts and not just across two handlers in one closure.
//   - check 3: be the reload subject. It holds a live subscription on a per-tick channel
//     (onGameFrame), so touching its .s2sp exercises the generation token A4d folded into the handler
//     type while dispatch is actively running.
import { hook, previous } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";

interface State { load: number; }

let load = 0;
const L = (m: string) => console.log(`[A4GATE-B#${load}] ${m}`);
let dmg = 0, pre = 0, created = 0, runcmd = 0, frames = 0;

export function OnPluginStart(): void {
  load = ((previous() as State | undefined)?.load ?? 0) + 1;
  L("loaded");

  dmg = 0; pre = 0; created = 0; runcmd = 0; frames = 0;

  hook.entity.onDamage((info) => {
    dmg += 1;
    L(`dmg#${dmg} B RAN saw=${info.damage}`);
  });

  hook.event("player_spawn", () => { pre += 1; if (pre <= 3) L(`onPre player_spawn #${pre}`); }, "pre");
  hook.entity.onCreate("*", () => { created += 1; });
  hook.client.onRunCmd(() => { runcmd += 1; if (runcmd === 1) L("onRunCmd FIRED (first)"); });

  hook.server.onGameFrame(() => {
    frames += 1;
    if (frames % 512 === 0) L(`heartbeat frames=${frames} dmg=${dmg} pre=${pre} created=${created}`);
  });

  command.server("a4_report_b", () => {
    L(`REPORT dmg=${dmg} pre=${pre} created=${created} runcmd=${runcmd} frames=${frames}`);
  });
}

export function OnPluginEnd(): void {
  L(`unloading (frames=${frames} dmg=${dmg})`);
}

export function OnPluginState(): State {
  return { load };
}

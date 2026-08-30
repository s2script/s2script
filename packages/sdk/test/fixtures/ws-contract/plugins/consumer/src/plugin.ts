// No `.s2script/types/` copy anywhere: this resolves through npm's workspace symlink to the
// sibling's own api.d.ts (design spec 2026-07-27 §3.2).
import type { Greeter } from "@fixture/ws-greeter";
import { command } from "@s2script/sdk/commands";
import { use } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const g = use<Greeter>("@fixture/ws-greeter");
  command("ws_greet", (cmd) => {
    cmd.reply(g.greet("world"));
  });
}

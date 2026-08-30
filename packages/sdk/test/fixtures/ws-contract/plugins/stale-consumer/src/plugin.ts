import type { Greeter } from "@fixture/ws-greeter";
import { command } from "@s2script/sdk/commands";
import { use } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const g = use<Greeter>("@fixture/ws-greeter");
  command("ws_stale_greet", (cmd) => {
    // A string argument: valid against the SIBLING's contract, TS2345 against the stale copy.
    cmd.reply(g.greet("world"));
  });
}

import type { Greeter } from "@demo/greeter";
import { command } from "@s2script/sdk/commands";
import { use } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const g = use<Greeter>("@demo/greeter");
  command("greet_me", (cmd) => {
    cmd.reply(g.greet("world"));
  });
}

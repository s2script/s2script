import { greet } from "@fixture/greetlib";
import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  command("ws_library_test", (cmd) => {
    cmd.reply(greet("world"));
  });
}

import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  command("ws_alpha", (cmd) => {
    cmd.reply("alpha");
  });
}

import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  command("ws_filter_keep", (cmd) => {
    cmd.reply("keep");
  });
}

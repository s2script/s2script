import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  command("ws_zulu", (cmd) => {
    cmd.reply("zulu");
  });
}

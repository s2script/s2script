import { label } from "@fixture/util";
import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  command("fixture_ws", (cmd) => { cmd.reply(label()); });
}

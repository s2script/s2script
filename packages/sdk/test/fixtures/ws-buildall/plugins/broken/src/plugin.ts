import { command } from "@s2script/sdk/commands";
// Deliberately broken: package.json declares `config.rounds` as an int with a string default, so
// buildPlugin's cheap fail-fast rejects it. The SOURCE is fine — the point of the fixture is that
// this plugin's failure must not stop its siblings from building (collect-all, spec §5.1 step 4).

export function OnPluginStart(): void {
  command("ws_broken", (cmd) => {
    cmd.reply("broken");
  });
}

// @s2script/adminhelp — SourceMod sm_help: list every registered command with the admin access it
// requires, paginated. Reads Commands.list() (the core command registry + flag mask) and maps each
// flag mask to human names via ADMFLAG. No engine work beyond the core __s2_commands_list native.

import { plugin } from "@s2script/sdk/plugin";
import { Commands } from "@s2script/sdk/commands";
import { ADMFLAG } from "@s2script/sdk/admin";

const PER_PAGE = 10;

// flags: 0 = anyone, < 0 = server-console-only, otherwise the ADMFLAG bit mask required.
function flagsToLabel(flags: number): string {
  if (flags === 0) return "anyone";
  if (flags < 0) return "server console";
  if ((flags & ADMFLAG.ROOT) === ADMFLAG.ROOT) return "root";
  const names: string[] = [];
  for (const [name, bit] of Object.entries(ADMFLAG)) {
    if (bit !== 0 && (flags & bit) === bit) names.push(name.toLowerCase());
  }
  return names.length ? names.join("|") : "admin";
}

export default plugin((ctx) => {
  ctx.commands.registerAdmin("sm_help", ADMFLAG.GENERIC, (cmd) => {
    const cmds = Commands.list().slice().sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
    const pages = Math.max(1, Math.ceil(cmds.length / PER_PAGE));
    let page = cmd.argInt(0, 1);
    if (page < 1) page = 1;
    if (page > pages) page = pages;

    cmd.reply("[SM] Commands (page " + page + "/" + pages + ", " + cmds.length + " total):");
    const start = (page - 1) * PER_PAGE;
    for (const c of cmds.slice(start, start + PER_PAGE)) {
      cmd.reply("  " + c.name + " - " + flagsToLabel(c.flags));
    }
    if (page < pages) cmd.reply("[SM] Type sm_help " + (page + 1) + " for the next page.");
  });

  console.log("[adminhelp] onLoad - sm_help registered");
});

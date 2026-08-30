// @s2script/adminhelp — SourceMod sm_help: list every registered command with the admin access it
// requires, paginated. Reads Commands.list() (the core command registry + flag mask) and maps each
// flag mask to human names via ADMFLAG. No engine work beyond the core __s2_commands_list native.

import { command, translations, Commands, ADMFLAG, Translations, HookResult } from "@s2script/sdk";

const PER_PAGE = 10;

// flags: 0 = anyone, < 0 = server-console-only, otherwise the ADMFLAG bit mask required.
// `slot` is the CALLER viewing sm_help's own output — translate for them, not any listed command's
// owner (there isn't one).
function flagsToLabel(flags: number, slot: number): string {
  if (flags === 0) return Translations.translate(slot, "Flags Anyone");
  if (flags < 0) return Translations.translate(slot, "Flags Server Console");
  if ((flags & ADMFLAG.ROOT) === ADMFLAG.ROOT) return Translations.translate(slot, "Flags Root");
  // The individual flag letters/names (e.g. "kick", "ban") are ADMFLAG identifiers, not prose —
  // left as-is, same treatment as basecommands' flagString() letter ladder.
  const names: string[] = [];
  for (const [name, bit] of Object.entries(ADMFLAG)) {
    if (bit !== 0 && (flags & bit) === bit) names.push(name.toLowerCase());
  }
  return names.length ? names.join("|") : Translations.translate(slot, "Flags Admin");
}

export function OnPluginStart(): void {
  translations.load("adminhelp", "common");

  command.admin("sm_help", ADMFLAG.GENERIC, (cmd) => {
    const cmds = Commands.list().slice().sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
    const pages = Math.max(1, Math.ceil(cmds.length / PER_PAGE));
    let page = cmd.argInt(0, 1);
    if (page < 1) page = 1;
    if (page > pages) page = pages;

    cmd.replyT("Commands Header", page, pages, cmds.length);
    const start = (page - 1) * PER_PAGE;
    for (const c of cmds.slice(start, start + PER_PAGE)) {
      cmd.replyT("Command Row", c.name, flagsToLabel(c.flags, cmd.callerSlot));
    }
    if (page < pages) cmd.replyT("Next Page", page + 1);
    return HookResult.Handled;
  });
}

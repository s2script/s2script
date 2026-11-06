import { StringTables, command, HookResult } from "@s2script/sdk";

/**
 * Network string tables — list them, look a string up, read an entry's user data.
 *
 *   sm_st_list                  every table with its string count
 *   sm_st_find <table> <value>  the index of <value> in <table> (-1 when absent)
 *   sm_st_show <table> <index>  the string at <index> and its user-data length
 *
 * Handles are plain data and are looked up again on each call, so these commands keep working
 * across map changes without caching anything.
 */
export const name = "stringtables";
export const describe = "network string tables (sm_st_list / _find / _show)";

export function OnPluginStart(): void {
  command("sm_st_list", (cmd) => {
    const tables = StringTables.all();
    if (tables.length === 0) cmd.reply("[cookbook] stringtables: none (no map loaded, or unavailable)");
    for (const t of tables) cmd.reply(`[cookbook] ${t.id}: ${t.name} (${t.count()} strings)`);
    return HookResult.Handled;
  });

  command("sm_st_find", (cmd) => {
    const t = StringTables.find(cmd.arg(0));
    if (!t) {
      cmd.reply("[cookbook] usage: sm_st_find <table> <value> — no such table");
      return HookResult.Handled;
    }
    cmd.reply(`[cookbook] ${t.name}: "${cmd.arg(1)}" is at ${t.findString(cmd.arg(1))}`);
    return HookResult.Handled;
  });

  command("sm_st_show", (cmd) => {
    const t = StringTables.find(cmd.arg(0));
    const index = cmd.argInt(1, -1);
    const value = t?.getString(index) ?? null;
    if (!t || value === null) {
      cmd.reply("[cookbook] usage: sm_st_show <table> <index> — no such table or index");
      return HookResult.Handled;
    }
    const data = t.getData(index);
    cmd.reply(`[cookbook] ${t.name}[${index}] = "${value}" (${data?.length ?? 0} bytes of user data)`);
    return HookResult.Handled;
  });
}

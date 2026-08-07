/** basecommands — English seed. Generated into translations/basecommands.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  "Usage Kick": "Usage: sm_kick <target> [reason]",
  "Kick Ambiguous Target": "{green}[SM]{default} Multiple players match '{1}' — be more specific (or use @all).",
  // No colour tag: this is the engine kick reason (disconnect UI), which never runs through the
  // colour-expanding chat/console funnels — a {tag} here would show up as literal text.
  "Kick Reason Default": "Kicked by admin",
  "Kicked Player": "{green}[SM]{default} Kicked {1} player.",
  "Kicked Players": "{green}[SM]{default} Kicked {1} players.",

  "Usage Map": "Usage: sm_map <mapname>",
  "Invalid Map Name": "{green}[SM]{default} Invalid map name.",
  "Map Not Valid": "{green}[SM]{default} '{1}' is not a valid map.",
  "Changing Map": "{green}[SM]{default} Changing map to {1}…",

  "Who Header Name": "Name",
  "Who Header Groups": "Groups",
  "Who Header Access": "Admin access",
  "See Console For Output": "{green}[SM]{default} See console for output.",
  "Not An Admin": '{green}[SM]{default} "{1}" is not an admin.',
  "Who Access With Groups": '{green}[SM]{default} "{1}" is logged in as "{2}" with access: {3}',
  "Who Access": '{green}[SM]{default} "{1}" has access: {2}',
  "Flag None": "none",
  "Flag Root": "root",

  "Admin Cache Reloaded": "{green}[SM]{default} Admin cache reloaded.",

  "Usage Rcon": "Usage: sm_rcon <command>",
  "Rcon Command Sent": "{green}[SM]{default} Command sent.",

  "Usage Exec": "Usage: sm_exec <cfgfile>",
  "Invalid Config Name": "{green}[SM]{default} Invalid config name.",
  "Executing Config": "{green}[SM]{default} Executing {1}.",

  "Usage Cvar": "Usage: sm_cvar <name> [value]",
  "Cvar Value": "{green}[SM]{default} {1} = {2}",
  "Invalid Cvar Value": "{green}[SM]{default} Invalid cvar value (no ; or quotes).",
  "Cvar Set": "{green}[SM]{default} {1} set to {2}",

  "Sm Version": "{green}[SM]{default} s2script 0.1.0 — a TypeScript plugin framework for Source 2 / CS2, by Gabriel Hirakawa.",
  "Sm Repo": "{green}[SM]{default} github.com/s2script/s2script",
  "Plugins Header": "{green}[SM]{default} Plugins ({1}):",
  "Plugin List Row Running": '  {1} "{2}" (running)',
  "Plugin List Row Unloaded": '  {1} "{2}" (unloaded)',
  "Usage Sm Plugins": "Usage: sm plugins <list|load|unload|reload> [id]",
  "Unloading Plugin": "{green}[SM]{default} Unloading '{1}'…",
  "Plugin Not Loaded": "{green}[SM]{default} Not a loaded plugin: {1}",
  "Reloading Plugin": "{green}[SM]{default} Reloading '{1}'…",
  "Plugin Not Found": "{green}[SM]{default} No such plugin: {1}",
  "Loading Plugin": "{green}[SM]{default} Loading '{1}'…",
  "Plugin Not Unloaded": "{green}[SM]{default} Plugin is not unloaded: {1}",
  "Sm Unknown Subcommand": "{green}[SM]{default} Unknown sub-command '{1}'. Try: sm plugins list",

  // adminmenu topmenu item — MenuStyle.Center (both the item's own display name and the sub-menu
  // title), which never expands colour tags, and neither carried one originally. Map names
  // themselves (MAP_CHOICES) are proper nouns, not prose — left untranslated.
  "Change Map Item": "Change Map",
  "Change Map Title": "Change Map",
};

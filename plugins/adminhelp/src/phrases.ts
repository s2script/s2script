/** adminhelp — English seed. Generated into translations/adminhelp.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  // "[SM]"-prefixed lines (cmd.reply, which expands tags) get the established {green}[SM]{default}
  // convention. The per-command listing lines below have no "[SM]" prefix in the original, so they
  // stay colour-free (also: they're indented list rows, not something to decorate).
  "Commands Header": "{green}[SM]{default} Commands (page {1}/{2}, {3} total):",
  // The leading "  " is significant — it's the indent that sets this row off as a list item under
  // the header above. A translation that trims leading whitespace flattens that indent.
  "Command Row": "  {1} - {2}",
  "Next Page": "{green}[SM]{default} Type sm_help {1} for the next page.",
  // flagsToLabel's ladder — SM's PerformWho-adjacent access-label words, shown per listed command.
  "Flags Anyone": "anyone",
  "Flags Server Console": "server console",
  "Flags Root": "root",
  "Flags Admin": "admin",
};

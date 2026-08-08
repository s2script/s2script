/** adminmenu — English seed. Generated into translations/adminmenu.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  // None of these carried a colour or an "[SM] " prefix originally, so none gain a tag now — and
  // none may gain one later: every Menu in this plugin is MenuStyle.Center (set in both
  // showCategory() and the sm_admin handler in plugin.ts), whose HTML renderer never expands {tag}
  // syntax (it HTML-escapes title/item text and paints its own fixed <font color=...> styling — see
  // basebans' ban-duration menu, Task 7). A {tag} added to any key below would render as literal,
  // escaped text in the menu, not a colour.
  "Must Be In Game": "Run sm_admin in-game.",
  "No Admin Actions Available": "No admin actions available.",
  "Admin Menu Title": "Admin Menu",
  // Display-only labels for the three category names this plugin registers via addCategory. The
  // CATEGORY STRING ITSELF ("Player Commands" etc.) is a cross-plugin matching key — basebans,
  // playercommands, basecomm and basevotes all pass that exact literal to ctx.topmenu.addItem — so
  // it must never change. These keys translate the label shown in the menu without touching that
  // identifier: see categoryLabel() in plugin.ts.
  "Category Player Commands": "Player Commands",
  "Category Server Commands": "Server Commands",
  "Category Voting Commands": "Voting Commands",
};

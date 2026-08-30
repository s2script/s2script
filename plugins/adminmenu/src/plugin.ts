import {
  command, hook, translations, TopMenu, Menu, MenuStyle, Admin, ADMFLAG, Translations,
} from "@s2script/sdk";
import type { PhraseKey } from "@s2script/sdk";

// Pure helpers (no side effects) — module-level.
function itemsFor(category: string, flags: number) {
  return TopMenu.snapshot().items.filter(i => i.category === category && ((flags & ADMFLAG.ROOT) !== 0 || (flags & i.flags) === i.flags));
}

// category names this plugin owns (its own addCategory calls) -> the phrase key for their DISPLAY
// label. `category` itself stays the untranslated matching key everywhere else (itemsFor, m.info,
// TopMenu.select) — only the text shown in the menu is looked up here. A category this plugin does
// not know about (a third-party plugin's own) falls back to showing its raw name, unchanged from
// today's behavior.
const CATEGORY_LABEL_KEY: Record<string, PhraseKey> = {
  "Player Commands": "Category Player Commands",
  "Server Commands": "Category Server Commands",
  "Voting Commands": "Category Voting Commands",
};
function categoryLabel(slot: number, category: string): string {
  const key = CATEGORY_LABEL_KEY[category];
  return key ? Translations.translate(slot, key) : category;
}

function showCategory(slot: number, category: string, flags: number): void {
  const items = itemsFor(category, flags);
  const m = new Menu(categoryLabel(slot, category));
  m.style = MenuStyle.Center;
  m.freezePlayer = true;   // WASD nav — freeze movement while the menu is open
  // it.id/it.name are OTHER plugins' own TopMenuItem registrations (e.g. basebans' "Kick") — out of
  // scope for this conversion, left exactly as rendered today.
  for (const it of items) m.addItem(it.id, it.name);
  m.onSelect(e => { TopMenu.select(e.info, slot); });
  m.display(slot, 30);
}

export function OnPluginStart(): void {
  translations.load("adminmenu", "common");

  // Fix the standard category order (items land in these; a plugin may add more). These strings are
  // a cross-plugin matching key (basebans/playercommands/basecomm/basevotes addItem against the same
  // literals) and must stay English/untranslated here — see categoryLabel above for the display side.
  hook.topmenu.addCategory("Player Commands");
  hook.topmenu.addCategory("Server Commands");
  hook.topmenu.addCategory("Voting Commands");

  command("sm_admin", (cmd) => {
    const slot = cmd.callerSlot;
    if (slot < 0) { cmd.replyT("Must Be In Game"); return; }
    const admin = Admin.forSlot(slot);
    if (!admin) { cmd.replyT("No access"); return; }
    const snap = TopMenu.snapshot();
    // Only categories with >=1 visible item.
    const cats = snap.categories.filter(c => itemsFor(c, admin.flags).length > 0);
    if (cats.length === 0) { cmd.replyT("No Admin Actions Available"); return; }
    const m = new Menu(Translations.translate(slot, "Admin Menu Title"));
    m.style = MenuStyle.Center;
    m.freezePlayer = true;   // WASD nav — freeze movement while the menu is open
    for (const c of cats) m.addItem(c, categoryLabel(slot, c));
    m.onSelect(e => { showCategory(slot, e.info, admin.flags); });
    m.display(slot, 30);
  });

  console.log("[adminmenu] onLoad — sm_admin registered");
}

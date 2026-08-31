import {
  command, topmenu, translations, TopMenu, Menu, MenuStyle, Admin, ADMFLAG, Translations,
  HookResult,
} from "@s2script/sdk";
import type { PhraseKey, TopMenuSheet } from "@s2script/sdk";

function itemOnSheet(item: { sheets?: TopMenuSheet[] }, sheet: TopMenuSheet): boolean {
  const sheets = item.sheets && item.sheets.length > 0 ? item.sheets : ["admin"];
  return sheets.includes(sheet);
}

function itemsFor(category: string, flags: number, sheet: TopMenuSheet) {
  return TopMenu.snapshot().items.filter(i =>
    i.category === category &&
    itemOnSheet(i, sheet) &&
    ((flags & ADMFLAG.ROOT) !== 0 || (flags & i.flags) === i.flags)
  );
}

const CATEGORY_LABEL_KEY: Record<string, PhraseKey> = {
  "Player Commands": "Category Player Commands",
  "Server Commands": "Category Server Commands",
  "Voting Commands": "Category Voting Commands",
};
function categoryLabel(slot: number, category: string): string {
  const key = CATEGORY_LABEL_KEY[category];
  return key ? Translations.translate(slot, key) : category;
}

function showCategory(slot: number, category: string, flags: number, sheet: TopMenuSheet): void {
  const items = itemsFor(category, flags, sheet);
  const m = new Menu(categoryLabel(slot, category));
  m.style = MenuStyle.Center;
  m.freezePlayer = true;   // user-requested HUD sheet — freeze while they pick
  for (const it of items) m.addItem(it.id, it.name);
  m.onSelect(e => { TopMenu.select(e.info, slot); });
  m.display(slot, 30);
}

function showSheet(slot: number, sheet: TopMenuSheet): boolean {
  const flags = Admin.forSlot(slot)?.flags ?? 0;
  const cats = TopMenu.snapshot().categories.filter(c => itemsFor(c, flags, sheet).length > 0);
  if (cats.length === 0) return false;
  const titleKey: PhraseKey = sheet === "admin" ? "Admin Menu Title" : "Menu Title";
  const m = new Menu(Translations.translate(slot, titleKey));
  m.style = MenuStyle.Center;
  m.freezePlayer = true;   // user-requested HUD sheet — freeze while they pick
  for (const c of cats) m.addItem(c, categoryLabel(slot, c));
  m.onSelect(e => { showCategory(slot, e.info, flags, sheet); });
  m.display(slot, 30);
  return true;
}

export function OnPluginStart(): void {
  translations.load("adminmenu", "common");

  // Fix the standard category order (items land in these; a plugin may add more). These strings are
  // a cross-plugin matching key (basebans/playercommands/basecomm/basevotes addItem against the same
  // literals) and must stay English/untranslated here — see categoryLabel above for the display side.
  topmenu.addCategory("Player Commands");
  topmenu.addCategory("Server Commands");
  topmenu.addCategory("Voting Commands");

  command("sm_admin", (cmd) => {
    const slot = cmd.callerSlot;
    if (slot < 0) { cmd.replyT("Must Be In Game"); return HookResult.Handled; }
    const admin = Admin.forSlot(slot);
    if (!admin) { cmd.replyT("No access"); return HookResult.Handled; }
    if (!showSheet(slot, "admin")) cmd.replyT("No Admin Actions Available");
    return HookResult.Handled;
  });

  command("sm_menu", (cmd) => {
    const slot = cmd.callerSlot;
    if (slot < 0) { cmd.replyT("Must Be In Game Menu"); return HookResult.Handled; }
    if (!showSheet(slot, "menu")) cmd.replyT("No Menu Actions Available");
    return HookResult.Handled;
  });
}

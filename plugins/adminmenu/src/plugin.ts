import {
  command, topmenu, translations, TopMenu, Admin, ADMFLAG, Translations,
  HookResult,
} from "@s2script/sdk";
import type { Client, PhraseKey, TopMenuSheet } from "@s2script/sdk";
import { hudkit, Player } from "@s2script/cs2";
import type { Dashboard, DashRow, DashTab } from "@s2script/cs2";

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
function tabTitle(slot: number, tab: { id: string; title: string }): string {
  const key = CATEGORY_LABEL_KEY[tab.id];
  return key ? Translations.translate(slot, key) : (tab.title || tab.id);
}

function visibleTabs(slot: number, flags: number, sheet: TopMenuSheet): DashTab[] {
  const snap = TopMenu.snapshot();
  const titles = new Map((snap.tabs ?? []).map(t => [t.id, t.title]));
  return snap.categories
    .filter(id => itemsFor(id, flags, sheet).length > 0)
    .map(id => ({ id, title: tabTitle(slot, { id, title: titles.get(id) ?? id }) }));
}

const sheetOf = new Map<number, TopMenuSheet>();
const frozenMoveType = new Map<number, number>();
const MOVETYPE_NONE = 0;

function freeze(slot: number): void {
  if (frozenMoveType.has(slot)) return;
  const pawn = Player.fromSlot(slot)?.pawn;
  if (!pawn || pawn.moveType === null || pawn.moveType === MOVETYPE_NONE) return;
  frozenMoveType.set(slot, pawn.moveType);
  pawn.moveType = MOVETYPE_NONE;
}

function unfreeze(slot: number): void {
  const prev = frozenMoveType.get(slot);
  frozenMoveType.delete(slot);
  if (prev === undefined) return;
  const pawn = Player.fromSlot(slot)?.pawn;
  if (pawn) pawn.moveType = prev;
}

function flagsOf(slot: number): number {
  return Admin.forSlot(slot)?.flags ?? 0;
}

let hub: Dashboard | null = null;

function showSheet(slot: number, sheet: TopMenuSheet): boolean {
  if (visibleTabs(slot, flagsOf(slot), sheet).length === 0) return false;
  if (!hub) return false;
  sheetOf.set(slot, sheet);
  freeze(slot);
  hub.open(slot);
  return true;
}

export function OnPluginStart(): void {
  translations.load("adminmenu", "common");

  // Standard shared buckets stay registered so a third-party plugin can still addItem against
  // the SourceMod category names. Empty tabs are hidden at paint time.
  topmenu.addCategory("Player Commands");
  topmenu.addCategory("Server Commands");
  topmenu.addCategory("Voting Commands");

  hub = hudkit.dashboard({
    title: (slot) => Translations.translate(slot, sheetOf.get(slot) === "menu" ? "Menu Title" : "Admin Menu Title"),
    closeText: "Close",
    tabs: (slot) => visibleTabs(slot, flagsOf(slot), sheetOf.get(slot) ?? "admin"),
    rows: (slot, tabId): DashRow[] => {
      const sheet = sheetOf.get(slot) ?? "admin";
      return itemsFor(tabId, flagsOf(slot), sheet).map(it => ({ id: it.id, a: it.name }));
    },
    onPick: (slot, _tabId, row, view) => {
      view.close();
      unfreeze(slot);
      sheetOf.delete(slot);
      TopMenu.select(row.id, slot);
    },
    onClose: (slot) => {
      unfreeze(slot);
      sheetOf.delete(slot);
    },
  });

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

export function OnClientDisconnect(client: Client): void {
  frozenMoveType.delete(client.slot);
  sheetOf.delete(client.slot);
  hub?.close(client.slot);
}

export function OnClientActive(client: Client): void {
  frozenMoveType.delete(client.slot);
  sheetOf.delete(client.slot);
  hub?.close(client.slot);
}

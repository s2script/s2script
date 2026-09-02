/**
 * The SourceMod-parity menu half: a feature-exercising demo menu and a categorised weapon buy menu.
 *
 * On CS2 both Chat and Center paint the same hudkit sheet (click to pick). They are here because
 * the custom-HUD API replaced the old WASD HTML / chat-number backends: run `sm_hud_menu chat` or
 * `sm_hud_menu center` and you get the same sheet, with freeze only on Center.
 */
import { Menu, MenuStyle, MenuCancelReason } from "@s2script/sdk";
import { Pawn, CsItem } from "@s2script/cs2";
import { moveTypeName } from "./movement";

/** Resolve a user-typed style token; anything unrecognised falls back to Chat, as the renderer does. */
export function parseStyle(token: string): MenuStyle {
  return token.toLowerCase() === "center" ? MenuStyle.Center : MenuStyle.Chat;
}

function cancelReasonName(reason: MenuCancelReason): string {
  switch (reason) {
    case MenuCancelReason.Exit: return "Exit";
    case MenuCancelReason.Timeout: return "Timeout";
    case MenuCancelReason.Disconnect: return "Disconnect";
    case MenuCancelReason.NewMenu: return "NewMenu";
    default: return `unknown (${reason})`;
  }
}

/**
 * A menu that exercises every feature of the Menu surface at once — pagination (more than the
 * 7-per-page chat limit), a disabled item, the auto Exit control, `freezePlayer`, a display
 * timeout, and both cancel paths.
 *
 * The items DO something rather than just logging, because a menu that only prints proves the menu
 * fired but not that the selection carried the right `info` through to a real effect.
 */
export function showDemo(slot: number, style: MenuStyle, log: (line: string) => void): void {
  const m = new Menu("hud-lab — menu feature test");
  m.style = style;
  m.exitButton = true;
  // CS2 paints both styles as a HUD sheet; freeze is still Center-only so Chat-style demos stay mobile.
  m.freezePlayer = style === MenuStyle.Center;

  m.addItem("heal", "Heal to 100");
  m.addItem("noclip", "Toggle noclip");
  m.addItem("movetype", "Report my movetype");
  m.addItem("strip", "Strip my weapons");
  m.addItem("disabled", "Disabled item (should not select)", { disabled: true });
  // Past 7 items the chat backend must paginate — this is the pagination test.
  for (let i = 1; i <= 6; i++) m.addItem(`filler${i}`, `Filler item ${i} (pagination test)`);

  m.onSelect((e) => {
    const pawn = Pawn.forSlot(e.slot);
    if (!pawn?.isValid) { log(`[menu] slot ${e.slot} picked ${e.info} but has no live pawn`); return; }
    switch (e.info) {
      case "heal":
        pawn.health = 100;
        log(`[menu] slot ${e.slot} healed to ${pawn.health}`);
        break;
      case "noclip": {
        const now = pawn.moveType;
        pawn.moveType = now === 7 ? 2 : 7; // NOCLIP <-> WALK
        log(`[menu] slot ${e.slot} movetype ${moveTypeName(now)} -> ${moveTypeName(pawn.moveType)}`);
        break;
      }
      case "movetype":
        log(`[menu] slot ${e.slot} movetype is ${moveTypeName(pawn.moveType)}`);
        break;
      case "strip":
        log(`[menu] slot ${e.slot} strip -> ${pawn.stripWeapons()}`);
        break;
      case "disabled":
        // Reaching this means the renderer let a disabled item through — a framework bug, not a
        // user action, so it is worth shouting about rather than silently ignoring.
        log(`[menu] BUG: slot ${e.slot} selected a DISABLED item`);
        break;
      default:
        log(`[menu] slot ${e.slot} picked filler ${e.info} (item index ${e.item})`);
    }
  });

  m.onCancel((e) => log(`[menu] slot ${e.slot} closed: ${cancelReasonName(e.reason)}`));
  m.display(slot, 30);
}

/** One buy-menu category: a heading and the items under it. */
interface BuyCategory {
  id: string;
  label: string;
  items: ReadonlyArray<{ id: string; label: string }>;
}

/**
 * The SourceMod-style categorised buy menu.
 *
 * Two levels (category -> item), because that is the shape the old SM buy menus had and the shape a
 * custom-HUD replacement would need to reproduce. Every item is a real `giveNamedItem`, so a
 * selection that silently fails is visible immediately.
 */
const CATEGORIES: ReadonlyArray<BuyCategory> = [
  {
    id: "rifles", label: "Rifles",
    items: [
      { id: CsItem.AK47, label: "AK-47" },
      { id: CsItem.M4A1, label: "M4A4" },
      { id: CsItem.M4A1S, label: "M4A1-S" },
      { id: CsItem.AUG, label: "AUG" },
      { id: CsItem.SG556, label: "SG 553" },
      { id: CsItem.GalilAR, label: "Galil AR" },
      { id: CsItem.Famas, label: "FAMAS" },
    ],
  },
  {
    id: "snipers", label: "Snipers",
    items: [
      { id: CsItem.AWP, label: "AWP" },
      { id: CsItem.SSG08, label: "SSG 08" },
      { id: CsItem.SCAR20, label: "SCAR-20" },
      { id: CsItem.G3SG1, label: "G3SG1" },
    ],
  },
  {
    id: "smgs", label: "SMGs",
    items: [
      { id: CsItem.MP9, label: "MP9" },
      { id: CsItem.MP7, label: "MP7" },
      { id: CsItem.MP5SD, label: "MP5-SD" },
      { id: CsItem.UMP45, label: "UMP-45" },
      { id: CsItem.P90, label: "P90" },
      { id: CsItem.Bizon, label: "PP-Bizon" },
      { id: CsItem.Mac10, label: "MAC-10" },
    ],
  },
  {
    id: "pistols", label: "Pistols",
    items: [
      { id: CsItem.Deagle, label: "Desert Eagle" },
      { id: CsItem.Revolver, label: "R8 Revolver" },
      { id: CsItem.Glock, label: "Glock-18" },
      { id: CsItem.USPS, label: "USP-S" },
      { id: CsItem.P250, label: "P250" },
      { id: CsItem.FiveSeven, label: "Five-SeveN" },
      { id: CsItem.Tec9, label: "Tec-9" },
      { id: CsItem.CZ, label: "CZ75-Auto" },
      { id: CsItem.Elite, label: "Dual Berettas" },
      { id: CsItem.HKP2000, label: "P2000" },
    ],
  },
  {
    id: "heavy", label: "Heavy",
    items: [
      { id: CsItem.Nova, label: "Nova" },
      { id: CsItem.XM1014, label: "XM1014" },
      { id: CsItem.MAG7, label: "MAG-7" },
      { id: CsItem.SawedOff, label: "Sawed-Off" },
      { id: CsItem.M249, label: "M249" },
      { id: CsItem.Negev, label: "Negev" },
    ],
  },
  {
    id: "gear", label: "Grenades & Gear",
    items: [
      { id: CsItem.HighExplosive, label: "HE Grenade" },
      { id: CsItem.Flashbang, label: "Flashbang" },
      { id: CsItem.Smoke, label: "Smoke Grenade" },
      { id: CsItem.Molotov, label: "Molotov" },
      { id: CsItem.Incendiary, label: "Incendiary" },
      { id: CsItem.Decoy, label: "Decoy" },
      { id: CsItem.Healthshot, label: "Healthshot" },
      { id: CsItem.AssaultSuit, label: "Kevlar + Helmet" },
      // No CsItem member for the defuse kit — `giveNamedItem` also accepts a raw class string,
      // and this is the one the bomb tests actually need.
      { id: "item_defuser", label: "Defuse Kit" },
      { id: CsItem.Taser, label: "Zeus x27" },
    ],
  },
];

/** Open the top level of the buy menu for `slot`. */
export function showBuyMenu(slot: number, style: MenuStyle, log: (line: string) => void): void {
  const m = new Menu("Buy Menu");
  m.style = style;
  m.freezePlayer = style === MenuStyle.Center;
  for (const c of CATEGORIES) m.addItem(c.id, `${c.label} (${c.items.length})`);
  m.onSelect((e) => {
    const category = CATEGORIES.find((c) => c.id === e.info);
    if (category) showBuyCategory(e.slot, style, category, log);
  });
  m.onCancel((e) => log(`[buy] slot ${e.slot} closed top level: ${cancelReasonName(e.reason)}`));
  m.display(slot, 30);
}

function showBuyCategory(slot: number, style: MenuStyle, category: BuyCategory, log: (line: string) => void): void {
  const m = new Menu(`Buy — ${category.label}`);
  m.style = style;
  m.freezePlayer = style === MenuStyle.Center;
  for (const it of category.items) m.addItem(it.id, it.label);
  m.onSelect((e) => {
    const pawn = Pawn.forSlot(e.slot);
    if (!pawn?.isValid) { log(`[buy] slot ${e.slot} has no live pawn`); return; }
    // `giveNamedItem` returns the created Weapon or null — reporting which is the difference
    // between "the menu worked" and "the give worked".
    const weapon = pawn.giveNamedItem(e.info);
    log(`[buy] slot ${e.slot} give ${e.info} -> ${weapon ? `entity ${weapon.ref.index}` : "FAILED (null)"}`);
    // Re-open the category so buying several things does not mean re-navigating each time — the
    // behaviour the old SM buy menus had.
    showBuyCategory(e.slot, style, category, log);
  });
  m.onCancel((e) => {
    // Backing out of a category returns to the top level rather than closing outright.
    if (e.reason === MenuCancelReason.Exit) showBuyMenu(e.slot, style, log);
  });
  m.display(slot, 30);
}

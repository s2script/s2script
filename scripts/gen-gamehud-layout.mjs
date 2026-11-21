#!/usr/bin/env node
// Generates the game-kit workshop layouts and the descriptor the server runtime drives them with:
//
//   s2script_gamehud.xml     in-round HUD: top rail, kill feed, status card, weapon card, target
//                            label, toast
//   s2script_gamepanels.xml  panels: pop-up vote, drawer list, prompt, sheet, intro/spectate hub
//   s2script_gamekit.css     one stylesheet for both (hand-written base + generated families)
//   s2script_gamekit.manifest.json  ids, dialog variables, buttons, meters and pools per layout
//   games/cs2/js/gamekit.generated.js  the same descriptor, as the runtime loads it
//
// WHY GENERATED. A server can only toggle classes and set dialog strings on panels the client
// already has, so every row of every pool is a fixed id the runtime must agree with exactly. One
// script owns both sides, and check-gamehud-layout-generated.sh fails CI when they drift.
//
// WHY TWO LAYOUTS. Each custom_hud_layout entity has its own 1024-entry intern budget per vector,
// and the panels (a sheet alone is ~250 ids) would crowd the in-round HUD's. Both are spawned
// observable=false: a spectator never sees the watched player's private panels or hidden feed rows.
//
//   node scripts/gen-gamehud-layout.mjs          write the files
//   node scripts/gen-gamehud-layout.mjs --check  exit 1 if any committed file differs
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const WORKSHOP = join(ROOT, "examples/hud-lab/workshop");
const LAYOUT_DIR = join(WORKSHOP, "panorama/layout/custom_game");
const OUT_CSS = join(WORKSHOP, "panorama/styles/custom_game/s2script_gamekit.css");
const OUT_MANIFEST = join(WORKSHOP, "s2script_gamekit.manifest.json");
const OUT_JS = join(ROOT, "games/cs2/js/gamekit.generated.js");
const BASE_CSS = join(ROOT, "scripts/gamehud/base.css");

// ---- the numbers the runtime and the markup must agree on -------------------------------------
export const SPEC = {
  addon: "3790153369",
  hideClass: "s2-hide",
  // Accent per theme. Themes are named by colour, never by gamemode: any gamemode picks one.
  themes: {
    amber: "#e8b04a", red: "#e5484d", orange: "#f28a2e", teal: "#3fd4b8",
    blue: "#5d9cf5", violet: "#a98cff", green: "#46c07a",
  },
  // Tones are the chat colour tags (pawn.js ChatColors), lowercased, so a phrase colours the same
  // in chat and on the HUD. Meanings (docs/translations/STYLE.md): grey general text, white
  // emphasis, yellow numbers, green string values, blue commands/hints, orange actions, red
  // warnings, lightred the target, lightblue a failed/invalid target, darkred the admin/executor. TTT roles: innocent green, traitor red,
  // detective darkblue. A Label carries one tone class, so a mixed-colour phrase takes the colour
  // of its leading tag on the HUD. `accent` is not a tag: it follows the theme.
  tones: {
    default: "#eef1f3", white: "#ffffff", darkred: "#d9363e", lightpurple: "#ba81f0",
    green: "#5fd068", olive: "#bfe28a", lime: "#a2ff47", red: "#ff5a5f", grey: "#aeb5bc",
    yellow: "#ede47a", silver: "#c6d1dc", blue: "#5e98d9", darkblue: "#4b69ff",
    bluegrey: "#9fb6cc", lightblue: "#8fc4ff", purple: "#d32ce6", lightred: "#eb6b6b", orange: "#e4ae39",
    accent: null,
  },
  rail: { pips: 10 },
  feed: { rows: 6 },
  status: { bars: 2 },
  vote: { options: 8 },
  drawer: { tabs: 5, rows: 8 },
  prompt: { answers: 4 },
  sheet: { side: 16, list: 8, paragraphs: 4, toggles: 8, grid: 9, tableCols: 6, tableRows: 8, rowActions: 2, dots: 10 },
  hub: { items: 8, card: 6, picker: 12 },
  summary: { stats: 4, cols: 4, rows: 5 },
  // Weapon card rows, in CS2's slot-key order: primary, secondary, knife, grenades, other (C4 and the rest).
  // Each row holds up to `items` items (knife and Zeus share a row, grenades share a row) and a "+N"
  // label. An item is an icon, a label, or both.
  weapon: { slots: ["primary", "secondary", "knife", "grenades", "other"], items: 4, iconHeight: 16, headIconHeight: 22 },
};
const S = SPEC;

// ---- tiny markup builder ----------------------------------------------------------------------
function layoutBuilder(name) {
  const st = { name, ids: [], vars: [], buttons: [], meters: {}, pools: {}, lines: [] };
  const T = (n) => "\t".repeat(n);
  const reg = (id) => { if (st.ids.includes(id)) throw new Error(`duplicate id ${id}`); st.ids.push(id); return id; };
  const b = {
    st,
    raw: (d, s) => st.lines.push(T(d) + s),
    open: (d, tag, id, cls) => st.lines.push(`${T(d)}<${tag}${id ? ` id="${reg(id)}"` : ""} class="${cls}">`),
    close: (d, tag) => st.lines.push(`${T(d)}</${tag}>`),
    panel: (d, id, cls) => st.lines.push(`${T(d)}<Panel${id ? ` id="${reg(id)}"` : ""} class="${cls}" />`),
    // A Label with fixed text and no id: nothing for the server to drive.
    text: (d, cls, value) => st.lines.push(`${T(d)}<Label class="${cls}" text="${value}" />`),
    label: (d, id, cls) => { reg(id); st.vars.push(id); st.lines.push(`${T(d)}<Label id="${id}" class="${cls}" text="{s:${id}}" />`); return id; },
    image: (d, id, cls) => { reg(id); st.vars.push(id); st.lines.push(`${T(d)}<Image id="${id}" class="${cls}" src="{s:${id}}" scaling="stretch-to-fit-preserve-aspect" />`); return id; },
    // An icon sized by its texture, not by CSS: a fixed height and a width that follows the image's
    // own aspect (texturewidth -1). Both attributes are on CustomHud's Image whitelist.
    icon: (d, id, cls, height) => { reg(id); st.vars.push(id); st.lines.push(`${T(d)}<Image id="${id}" class="${cls}" src="{s:${id}}" textureheight="${height}" texturewidth="-1" />`); return id; },
    button: (d, id, cls, kids) => { b.open(d, "Button", id, cls); st.buttons.push(id); kids(d + 1); b.close(d, "Button"); },
    meter: (d, meterName, fillId, trackCls, fillCls) => {
      b.open(d, "Panel", null, trackCls);
      b.panel(d + 1, fillId, `${fillCls} s2-w0`);
      b.close(d, "Panel");
      st.meters[meterName] = fillId;
    },
    tick: (d) => b.panel(d, null, "gk-tick gk-acc-bg"),
    pool: (name, entry) => { (st.pools[name] ||= []).push(entry); },
  };
  return b;
}

function wrap(b, header, bodyFn) {
  b.raw(0, header);
  b.raw(0, `<root>`);
  b.raw(1, `<styles>`);
  b.raw(2, `<include src="file://{resources}/styles/custom_game/s2script_gamekit.css" />`);
  b.raw(1, `</styles>`);
  b.raw(1, `<Panel class="gk-root">`);
  b.open(2, "Panel", "gk_theme", "gk-theme gk-theme-amber");
  bodyFn(3);
  b.close(2, "Panel");
  b.raw(1, `</Panel>`);
  b.raw(0, `</root>`);
  return b.st.lines.join("\n") + "\n";
}

const HEADER = (what) => `<!--
  s2script game kit: ${what}. Workshop addon ${S.addon} (with s2script_lib.xml).

  GENERATED by scripts/gen-gamehud-layout.mjs. Do not edit by hand: change the generator and run it.

  Driven by the server with two levers only: classes (s2-hide, s2-w0..s2-w10 meter steps,
  gk-theme-*, gk-t-*/gk-f-*/gk-e-* tones, gk-sel/gk-on/s2-btn-disabled state) and {s:} dialog
  variables whose names equal their panel ids. No <scripts> and no event attributes: Valve's
  custom_hud_layout contract supports neither. The root Panel may not carry an id, so the theme
  class lives on gk_theme, its first child. The full id list is in s2script_gamekit.manifest.json.
-->`;

// ---- layout 1: in-round HUD -------------------------------------------------------------------
const hud = layoutBuilder("hud");
const hudXml = wrap(hud, HEADER("in-round HUD (top rail, kill feed, status card, weapon card, target label, toast)"), (d) => {
  const b = hud;
  b.raw(d, `<!-- Top rail: two score blocks around a clock, and a round line. The side tone (gk-e-*) is a class on gk_rail_l / gk_rail_r. -->`);
  b.open(d, "Panel", "gk_rail", "gk-rail s2-hide");
  b.open(d + 1, "Panel", null, "gk-rail-row");
  for (const side of ["l", "r"]) {
    if (side === "r") b.label(d + 2, "gk_rail_clock", "gk-clock");
    b.open(d + 2, "Panel", `gk_rail_${side}`, `gk-side gk-side-${side} gk-e-blue`);
    const score = () => b.label(d + 3, `gk_rail_${side}score`, "gk-score");
    const lab = () => b.label(d + 3, `gk_rail_${side}lab`, "gk-side-lab");
    const pips = () => {
      b.open(d + 3, "Panel", null, "gk-pips");
      for (let i = 0; i < S.rail.pips; i++) b.panel(d + 4, `gk_rail_${side}p${i}`, "gk-pip s2-hide");
      b.close(d + 3, "Panel");
    };
    if (side === "l") { score(); lab(); pips(); } else { pips(); lab(); score(); }
    b.close(d + 2, "Panel");
  }
  b.close(d + 1, "Panel");
  b.label(d + 1, "gk_rail_sub", "gk-rail-sub");
  b.close(d, "Panel");

  b.raw(d, `<!-- Kill feed: rows only as wide as their content, right-aligned. The runtime rotates which row is newest. -->`);
  b.open(d, "Panel", "gk_feed", "gk-feed");
  for (let i = 0; i < S.feed.rows; i++) {
    const r = `gk_kf${i}`;
    b.open(d + 1, "Panel", r, "gk-kill gk-e-grey s2-hide");
    b.label(d + 2, `${r}_tag`, "gk-kill-tag");
    b.label(d + 2, `${r}_a`, "gk-kill-name");
    b.image(d + 2, `${r}_wi`, "gk-kill-icon");
    b.label(d + 2, `${r}_w`, "gk-kill-wtext");
    b.panel(d + 2, `${r}_hs`, "gk-kill-hs s2-hide");
    b.label(d + 2, `${r}_v`, "gk-kill-name");
    b.close(d + 1, "Panel");
    b.pool("feed", { id: r, vars: [`${r}_tag`, `${r}_a`, `${r}_wi`, `${r}_w`, `${r}_v`] });
  }
  b.close(d, "Panel");

  b.raw(d, `<!-- Status card: role, hint and two bars. Bars move in 10% steps (s2-w0..s2-w10). -->`);
  b.open(d, "Panel", "gk_status", "gk-panel gk-status s2-hide");
  b.open(d + 1, "Panel", null, "gk-role-row");
  b.label(d + 2, "gk_status_role", "gk-role gk-t-accent");
  b.label(d + 2, "gk_status_hint", "gk-role-hint");
  b.close(d + 1, "Panel");
  for (let i = 0; i < S.status.bars; i++) {
    b.open(d + 1, "Panel", `gk_bar${i}`, "gk-bar-row");
    b.label(d + 2, `gk_bar${i}_lab`, "gk-bar-lab");
    b.meter(d + 2, `bar${i}`, `gk_bar${i}_fill`, "gk-bar", `gk-bar-fill gk-f-${i ? "blue" : "green"}`);
    b.label(d + 2, `gk_bar${i}_val`, "gk-bar-val");
    b.close(d + 1, "Panel");
  }
  b.close(d, "Panel");

  b.raw(d, `<!-- Weapon card: replaces the stock weapons HUD (m_iHideHUD bit 6). Bottom right, clear of chat (left) and the kill feed (top right).
       A compact slot list (the item in hand takes gk-on, its row gk-sel), then the active weapon's name (or grenade count) and clip / reserve. -->`);
  b.open(d, "Panel", "gk_wpn", "gk-panel gk-wpn s2-hide");
  b.open(d + 1, "Panel", "gk_wpn_slots", "gk-wpn-slots s2-hide");
  S.weapon.slots.forEach((slotName, i) => {
    const r = `gk_wpn_s${i}`;
    b.open(d + 2, "Panel", r, "gk-wslot s2-hide");
    b.text(d + 3, "gk-wslot-k", String(i + 1));
    b.open(d + 3, "Panel", null, "gk-wslot-items");
    const vars = [];
    for (let j = 0; j < S.weapon.items; j++) {
      const c = `${r}_i${j}`;
      b.open(d + 4, "Panel", c, "gk-wslot-i s2-hide");
      vars.push(b.icon(d + 5, `${c}_ic`, "gk-wslot-ic s2-hide", S.weapon.iconHeight));
      vars.push(b.label(d + 5, `${c}_t`, "gk-wslot-t s2-hide"));
      b.close(d + 4, "Panel");
    }
    vars.push(b.label(d + 4, `${r}_more`, "gk-wslot-t gk-wslot-more s2-hide"));
    b.close(d + 3, "Panel");
    b.close(d + 2, "Panel");
    b.pool("weaponSlots", { id: r, slot: slotName, vars });
  });
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", null, "gk-wpn-hd");
  b.icon(d + 2, "gk_wpn_icon", "gk-wpn-icon s2-hide", S.weapon.headIconHeight);
  b.label(d + 2, "gk_wpn_name", "gk-wpn-name");
  b.label(d + 2, "gk_wpn_count", "gk-wpn-count s2-hide");
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", "gk_wpn_ammo", "gk-wpn-ammo s2-hide");
  b.label(d + 2, "gk_wpn_clip", "gk-wpn-clip");
  b.label(d + 2, "gk_wpn_res", "gk-wpn-res");
  b.close(d + 1, "Panel");
  b.close(d, "Panel");

  b.raw(d, `<!-- Target label: who the player is looking at, centred just under the crosshair. A name and an optional sub line.
       Starts faded and collapsed; the runtime fades it (gk-fade, 0.1 s) before it collapses, so a sweeping aim does not blink. -->`);
  b.open(d, "Panel", "gk_target", "gk-panel gk-target gk-fade s2-hide");
  b.label(d + 1, "gk_target_n", "gk-target-n gk-t-default");
  b.label(d + 1, "gk_target_s", "gk-target-s s2-hide");
  b.close(d, "Panel");

  b.raw(d, `<!-- Toast: one line and a small subline, top centre. -->`);
  b.open(d, "Panel", "gk_toast", "gk-toast s2-hide");
  b.label(d + 1, "gk_toast_t", "gk-toast-t");
  b.label(d + 1, "gk_toast_s", "gk-toast-s");
  b.close(d, "Panel");

  b.raw(d, `<!-- Round-end summary: who won, why, a few headline stats and a short table. Passive: no cursor. -->`);
  b.open(d, "Panel", "gk_sum", "gk-panel gk-sum gk-e-accent s2-hide");
  b.label(d + 1, "gk_sum_eye", "gk-eyebrow gk-t-accent");
  b.label(d + 1, "gk_sum_t", "gk-sum-t gk-t-accent");
  b.label(d + 1, "gk_sum_s", "gk-sum-s");
  b.open(d + 1, "Panel", null, "gk-kv gk-sum-kv");
  for (let i = 0; i < S.summary.stats; i++) {
    const r = `gk_sum_kv${i}`;
    b.open(d + 2, "Panel", r, "gk-kv-cell s2-hide");
    b.label(d + 3, `${r}_k`, "gk-kv-k");
    b.label(d + 3, `${r}_v`, "gk-kv-v");
    b.close(d + 2, "Panel");
    b.pool("summaryStats", { id: r, vars: [`${r}_k`, `${r}_v`] });
  }
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", "gk_sum_table", "gk-sum-table s2-hide");
  b.open(d + 2, "Panel", null, "gk-tr gk-th");
  for (let c = 0; c < S.summary.cols; c++) b.label(d + 3, `gk_sum_th${c}`, `gk-td gk-col${c} s2-hide`);
  b.close(d + 2, "Panel");
  for (let i = 0; i < S.summary.rows; i++) {
    const r = `gk_sum_tr${i}`;
    b.open(d + 2, "Panel", r, "gk-tr s2-hide");
    const vars = [];
    for (let c = 0; c < S.summary.cols; c++) vars.push(b.label(d + 3, `${r}_c${c}`, `gk-td gk-col${c} s2-hide`));
    b.close(d + 2, "Panel");
    b.pool("summaryRows", { id: r, vars });
  }
  b.close(d + 1, "Panel");
  b.close(d, "Panel");
});

// ---- layout 2: panels -------------------------------------------------------------------------
const pn = layoutBuilder("panels");
// A surface the player opens and holds the cursor for gets a close button: a captured cursor with
// no way out is a trap.
const head = (b, d, prefix, closeable) => {
  b.open(d, "Panel", null, "gk-hd");
  b.tick(d + 1);
  b.label(d + 1, `${prefix}_title`, "gk-hd-title");
  b.label(d + 1, `${prefix}_right`, "gk-hd-right");
  if (closeable) b.button(d + 1, `${prefix}_close`, "gk-close", (dd) => b.label(dd, `${prefix}_close_t`, "gk-close-t"));
  b.close(d, "Panel");
};
const panelsXml = wrap(pn, HEADER("panels (pop-up vote, drawer list, prompt, sheet, intro and spectate hub)"), (d) => {
  const b = pn;

  // The hub is the FIRST child on purpose. It covers the whole screen, and Panorama draws later
  // siblings on top and hit-tests them first, so the vote, drawer, prompt and sheet after it always
  // draw over the intro or spectate hub and take the click, whichever opened first. A z-index in
  // the stylesheet says the same thing. Keep every new overlay after this block.
  b.raw(d, `<!-- Hub: the intro on first join and the spectate hub. Brand, item list, profile card, player picker, shot label. -->`);
  b.raw(d, `<!-- First child on purpose: every panel after it draws on top and takes the click. -->`);
  b.open(d, "Panel", "gk_hub", "gk-hub s2-hide");
  b.panel(d + 1, null, "gk-hub-vig");
  b.open(d + 1, "Panel", null, "gk-brand");
  b.label(d + 2, "gk_hub_eye", "gk-eyebrow gk-t-accent");
  b.label(d + 2, "gk_hub_title", "gk-brand-title");
  b.label(d + 2, "gk_hub_sub", "gk-brand-sub");
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", "gk_hub_list", "gk-panel gk-hub-list");
  head(b, d + 2, "gk_hub_list", true);
  for (let i = 0; i < S.hub.items; i++) {
    const r = `gk_hub_i${i}`;
    b.button(d + 2, r, "gk-row gk-hub-row s2-hide", (dd) => { b.label(dd, `${r}_t`, "gk-hub-t"); b.label(dd, `${r}_m`, "gk-hub-m"); });
    b.pool("hubItems", { id: r, vars: [`${r}_t`, `${r}_m`] });
  }
  b.label(d + 2, "gk_hub_ft", "gk-ft");
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", "gk_hub_card", "gk-panel gk-card-pf s2-hide");
  b.open(d + 2, "Panel", null, "gk-hd");
  b.tick(d + 3);
  b.label(d + 3, "gk_hub_card_h", "gk-hd-title");
  b.close(d + 2, "Panel");
  b.open(d + 2, "Panel", null, "gk-pf-who");
  b.panel(d + 3, null, "gk-pf-av gk-acc-bg");
  b.open(d + 3, "Panel", null, "gk-pf-names");
  b.label(d + 4, "gk_hub_card_n", "gk-pf-n");
  b.label(d + 4, "gk_hub_card_s", "gk-pf-s");
  b.close(d + 3, "Panel");
  b.close(d + 2, "Panel");
  b.open(d + 2, "Panel", null, "gk-kv");
  for (let i = 0; i < S.hub.card; i++) {
    const r = `gk_hub_kv${i}`;
    b.open(d + 3, "Panel", r, "gk-kv-cell s2-hide");
    b.label(d + 4, `${r}_k`, "gk-kv-k");
    b.label(d + 4, `${r}_v`, "gk-kv-v");
    b.close(d + 3, "Panel");
    b.pool("hubCard", { id: r, vars: [`${r}_k`, `${r}_v`] });
  }
  b.close(d + 2, "Panel");
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", "gk_hub_pick", "gk-picker s2-hide");
  for (let i = 0; i < S.hub.picker; i++) {
    const r = `gk_hub_p${i}`;
    b.button(d + 2, r, "gk-pcard gk-e-grey s2-hide", (dd) => { b.label(dd, `${r}_t`, "gk-pcard-t"); b.label(dd, `${r}_s`, "gk-pcard-s"); });
    b.pool("hubPicker", { id: r, vars: [`${r}_t`, `${r}_s`] });
  }
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", "gk_hub_shot", "gk-shot s2-hide");
  b.panel(d + 2, null, "gk-shot-rec gk-acc-bg");
  b.label(d + 2, "gk_hub_shot_t", "gk-shot-t");
  b.label(d + 2, "gk_hub_shot_w", "gk-shot-w");
  b.close(d + 1, "Panel");
  b.close(d, "Panel");

  b.raw(d, `<!-- Pop-up vote: clickable rows that are also numbered, so players can type the number in chat. -->`);
  b.open(d, "Panel", "gk_vote", "gk-panel gk-vote s2-hide");
  head(b, d + 1, "gk_vote");
  b.meter(d + 1, "voteTimer", "gk_vote_timer", "gk-timer", "gk-timer-fill gk-acc-bg");
  b.label(d + 1, "gk_vote_q", "gk-vote-q");
  for (let i = 0; i < S.vote.options; i++) {
    const r = `gk_vo${i}`;
    b.button(d + 1, r, "gk-row gk-vote-row s2-hide", (dd) => {
      b.meter(dd, `vo${i}`, `${r}_fill`, "gk-vote-track", "gk-vote-fill");
      b.open(dd, "Panel", null, "gk-row-in");
      b.label(dd + 1, `${r}_n`, "gk-num gk-t-accent");
      b.label(dd + 1, `${r}_t`, "gk-row-t");
      b.label(dd + 1, `${r}_c`, "gk-row-c");
      b.close(dd, "Panel");
    });
    b.pool("voteOptions", { id: r, vars: [`${r}_n`, `${r}_t`, `${r}_c`] });
  }
  b.label(d + 1, "gk_vote_ft", "gk-ft");
  b.close(d, "Panel");

  b.raw(d, `<!-- Drawer list: tabs and rows with a small tile, opened by the player, click-only. -->`);
  b.open(d, "Panel", "gk_drawer", "gk-panel gk-drawer s2-hide");
  head(b, d + 1, "gk_dr", true);
  b.open(d + 1, "Panel", null, "gk-tabs");
  for (let i = 0; i < S.drawer.tabs; i++) {
    b.button(d + 2, `gk_dr_tab${i}`, "gk-tab s2-hide", (dd) => b.label(dd, `gk_dr_tab${i}_t`, "gk-tab-t"));
    b.pool("drawerTabs", { id: `gk_dr_tab${i}`, vars: [`gk_dr_tab${i}_t`] });
  }
  b.close(d + 1, "Panel");
  for (let i = 0; i < S.drawer.rows; i++) {
    const r = `gk_dr_r${i}`;
    b.button(d + 1, r, "gk-row gk-dr-row s2-hide", (dd) => {
      b.image(dd, `${r}_img`, "gk-thumb");
      b.open(dd, "Panel", null, "gk-dr-text");
      b.label(dd + 1, `${r}_t`, "gk-row-t");
      b.label(dd + 1, `${r}_s`, "gk-row-sub");
      b.close(dd, "Panel");
      b.label(dd, `${r}_m`, "gk-row-meta");
    });
    b.pool("drawerRows", { id: r, vars: [`${r}_img`, `${r}_t`, `${r}_s`, `${r}_m`] });
  }
  b.label(d + 1, "gk_dr_ft", "gk-ft");
  b.close(d, "Panel");

  b.raw(d, `<!-- Prompt: one question at a predictable moment, clickable answers. -->`);
  b.open(d, "Panel", "gk_prompt", "gk-panel gk-prompt s2-hide");
  head(b, d + 1, "gk_pr", true);
  b.label(d + 1, "gk_pr_q", "gk-prompt-q");
  b.label(d + 1, "gk_pr_why", "gk-prompt-why");
  for (let i = 0; i < S.prompt.answers; i++) {
    const r = `gk_pr_a${i}`;
    b.button(d + 1, r, "gk-row gk-pr-row s2-hide", (dd) => b.label(dd, `${r}_t`, "gk-row-t"));
    b.pool("promptAnswers", { id: r, vars: [`${r}_t`] });
  }
  b.label(d + 1, "gk_pr_ft", "gk-ft");
  b.close(d, "Panel");

  b.raw(d, `<!-- Sheet: grouped side tabs and one of five body kinds (list, paragraphs, toggles, grid, table). -->`);
  b.open(d, "Panel", "gk_sheet", "gk-panel gk-sheet s2-hide");
  b.open(d + 1, "Panel", null, "gk-hd gk-sheet-hd");
  b.tick(d + 2);
  b.label(d + 2, "gk_sh_title", "gk-hd-title");
  b.label(d + 2, "gk_sh_right", "gk-hd-right");
  b.button(d + 2, "gk_sh_close", "gk-close", (dd) => b.label(dd, "gk_sh_close_t", "gk-close-t"));
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", null, "gk-sheet-mid");
  b.open(d + 2, "Panel", "gk_sh_side", "gk-side-tabs");
  for (let i = 0; i < S.sheet.side; i++) {
    const r = `gk_sh_s${i}`;
    b.button(d + 3, r, "gk-stab s2-hide", (dd) => { b.label(dd, `${r}_t`, "gk-stab-t"); b.label(dd, `${r}_m`, "gk-stab-m"); });
    b.pool("sheetTabs", { id: r, vars: [`${r}_t`, `${r}_m`] });
  }
  b.label(d + 3, "gk_sh_more", "gk-stab-more s2-hide");
  b.close(d + 2, "Panel");
  b.open(d + 2, "Panel", "gk_sh_body", "gk-sheet-body");
  b.label(d + 3, "gk_sh_h", "gk-body-h");
  b.label(d + 3, "gk_sh_k", "gk-body-k");
  // list
  b.open(d + 3, "Panel", "gk_sh_list", "gk-body-list s2-hide");
  for (let i = 0; i < S.sheet.list; i++) {
    const r = `gk_sh_li${i}`;
    b.open(d + 4, "Panel", r, "gk-li s2-hide");
    b.label(d + 5, `${r}_n`, "gk-li-n gk-t-accent");
    b.label(d + 5, `${r}_t`, "gk-li-t");
    b.close(d + 4, "Panel");
    b.pool("sheetList", { id: r, vars: [`${r}_n`, `${r}_t`] });
  }
  b.close(d + 3, "Panel");
  // paragraphs
  b.open(d + 3, "Panel", "gk_sh_para", "gk-body-para s2-hide");
  for (let i = 0; i < S.sheet.paragraphs; i++) {
    b.label(d + 4, `gk_sh_p${i}`, "gk-para s2-hide");
    b.pool("sheetParagraphs", { id: `gk_sh_p${i}`, vars: [`gk_sh_p${i}`] });
  }
  b.close(d + 3, "Panel");
  // toggles
  b.open(d + 3, "Panel", "gk_sh_tog", "gk-body-tog s2-hide");
  for (let i = 0; i < S.sheet.toggles; i++) {
    const r = `gk_sh_tg${i}`;
    b.button(d + 4, r, "gk-tog s2-hide", (dd) => {
      b.open(dd, "Panel", null, "gk-tog-box");
      b.panel(dd + 1, null, "gk-tog-mark");
      b.close(dd, "Panel");
      b.label(dd, `${r}_t`, "gk-tog-t");
      b.label(dd, `${r}_v`, "gk-tog-v");
    });
    b.pool("sheetToggles", { id: r, vars: [`${r}_t`, `${r}_v`] });
  }
  b.close(d + 3, "Panel");
  // grid
  b.open(d + 3, "Panel", "gk_sh_grid", "gk-body-grid s2-hide");
  for (let i = 0; i < S.sheet.grid; i++) {
    const r = `gk_sh_g${i}`;
    b.button(d + 4, r, "gk-card s2-hide", (dd) => {
      b.open(dd, "Panel", null, "gk-card-top");
      b.label(dd + 1, `${r}_t`, "gk-card-t");
      b.label(dd + 1, `${r}_p`, "gk-card-p gk-t-accent");
      b.close(dd, "Panel");
      b.label(dd, `${r}_d`, "gk-card-d");
    });
    b.pool("sheetGrid", { id: r, vars: [`${r}_t`, `${r}_p`, `${r}_d`] });
  }
  b.close(d + 3, "Panel");
  // table
  b.open(d + 3, "Panel", "gk_sh_table", "gk-body-table s2-hide");
  b.open(d + 4, "Panel", null, "gk-tr gk-th");
  for (let c = 0; c < S.sheet.tableCols; c++) b.label(d + 5, `gk_sh_th${c}`, `gk-td gk-col${c} s2-hide`);
  b.close(d + 4, "Panel");
  for (let i = 0; i < S.sheet.tableRows; i++) {
    const r = `gk_sh_tr${i}`;
    b.open(d + 4, "Panel", r, "gk-tr s2-hide");
    const vars = [];
    for (let c = 0; c < S.sheet.tableCols; c++) vars.push(b.label(d + 5, `${r}_c${c}`, `gk-td gk-col${c} s2-hide`));
    for (let a = 0; a < S.sheet.rowActions; a++) {
      b.button(d + 5, `${r}_x${a}`, "gk-act s2-hide", (dd) => b.label(dd, `${r}_x${a}_t`, "gk-act-t"));
    }
    b.close(d + 4, "Panel");
    b.pool("sheetTable", { id: r, vars });
  }
  b.close(d + 3, "Panel");
  b.close(d + 2, "Panel");
  b.close(d + 1, "Panel");
  b.open(d + 1, "Panel", null, "gk-sheet-ft");
  b.label(d + 2, "gk_sh_ft", "gk-ft-t");
  b.open(d + 2, "Panel", "gk_sh_pager", "gk-pager s2-hide");
  b.button(d + 3, "gk_sh_prev", "gk-arrow", (dd) => b.label(dd, "gk_sh_prev_t", "gk-arrow-t"));
  for (let i = 0; i < S.sheet.dots; i++) b.panel(d + 3, `gk_sh_d${i}`, "gk-dot s2-hide");
  b.button(d + 3, "gk_sh_next", "gk-arrow", (dd) => b.label(dd, "gk_sh_next_t", "gk-arrow-t"));
  b.close(d + 2, "Panel");
  b.close(d + 1, "Panel");
  b.close(d, "Panel");

});

// ---- stylesheet: hand-written base + generated families ---------------------------------------
const css = [];
css.push(`/* ---- GENERATED below this line by scripts/gen-gamehud-layout.mjs ---- */`);
css.push(``);
css.push(`/* Meter steps, shared with hudkit's s2script_hud.css: 0%..100% in 10% steps. */`);
for (let i = 0; i <= 10; i++) css.push(`.s2-w${i} { width: ${i * 10}%; }`);
css.push(``);
css.push(`/* Themes: one accent per theme, reached through gk-acc-* and the accent tone. */`);
for (const [theme, hex] of Object.entries(S.themes)) {
  css.push(`.gk-theme-${theme} .gk-acc-bg { background-color: ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-acc-text, .gk-theme-${theme} .gk-t-accent { color: ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-f-accent { background-color: ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-e-accent { border-left: 3px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-sel { border-left: 3px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-tab.gk-sel { border-left: 0px solid ${hex}; border-bottom: 2px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-dot.gk-on { background-color: ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-tog.gk-on .gk-tog-box { background-color: ${hex}; border: 2px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-vote-fill { background-color: ${hex}33; }`);
  css.push(`.gk-theme-${theme} .gk-card:hover { border: 1px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-pcard.gk-sel { background-color: ${hex}24; border-bottom: 2px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-toast { border-bottom: 3px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-li.gk-hl { background-color: ${hex}24; border-left: 3px solid ${hex}; }`);
}
css.push(``);
css.push(`/* Tones: gk-t-* colours a Label, gk-f-* fills a shape, gk-e-* paints a left edge. */`);
for (const [tone, hex] of Object.entries(S.tones)) {
  if (!hex) continue;
  css.push(`.gk-t-${tone} { color: ${hex}; }`);
  css.push(`.gk-f-${tone} { background-color: ${hex}; }`);
  css.push(`.gk-e-${tone} { border-left: 3px solid ${hex}; }`);
}
css.push(``);
css.push(`/* Score-block edges sit on the bottom, not the left. */`);
for (const [tone, hex] of Object.entries(S.tones)) {
  if (!hex) continue;
  css.push(`.gk-side.gk-e-${tone} { border-left: 0px solid ${hex}; border-bottom: 3px solid ${hex}; }`);
  css.push(`.gk-side.gk-e-${tone} .gk-score { color: ${hex}; }`);
  css.push(`.gk-side.gk-e-${tone} .gk-pip { background-color: ${hex}; }`);
  css.push(`.gk-sum.gk-e-${tone} { border-left: 0px solid ${hex}; border-top: 3px solid ${hex}; }`);
  css.push(`.gk-pcard.gk-e-${tone} { border-left: 0px solid ${hex}; border-top: 3px solid ${hex}; }`);
  css.push(`.gk-toast.gk-e-${tone} { border-left: 0px solid ${hex}; border-bottom: 3px solid ${hex}; }`);
}
for (const [theme, hex] of Object.entries(S.themes)) {
  css.push(`.gk-theme-${theme} .gk-side.gk-e-accent { border-left: 0px solid ${hex}; border-bottom: 3px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-side.gk-e-accent .gk-score { color: ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-side.gk-e-accent .gk-pip { background-color: ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-toast.gk-e-accent { border-left: 0px solid ${hex}; border-bottom: 3px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-pcard.gk-e-accent { border-left: 0px solid ${hex}; border-top: 3px solid ${hex}; }`);
  css.push(`.gk-theme-${theme} .gk-sum.gk-e-accent { border-left: 0px solid ${hex}; border-top: 3px solid ${hex}; }`);
}
const cssOut = readFileSync(BASE_CSS, "utf8").trimEnd() + "\n\n" + css.join("\n") + "\n";

// ---- descriptor (manifest JSON + runtime JS) ---------------------------------------------------
const describe = (b, file) => ({
  resource: `panorama/layout/custom_game/${file}`,
  addons: [S.addon],
  observable: false,
  hideClass: S.hideClass,
  buttons: b.st.buttons,
  meters: b.st.meters,
  slots: b.st.pools,
});
const descriptor = {
  spec: SPEC,
  classes: {
    theme: Object.keys(S.themes).map((t) => `gk-theme-${t}`),
    text: Object.keys(S.tones).map((t) => `gk-t-${t}`),
    fill: Object.keys(S.tones).map((t) => `gk-f-${t}`),
    edge: Object.keys(S.tones).map((t) => `gk-e-${t}`),
    state: ["s2-hide", "gk-sel", "gk-on", "gk-hl", "gk-grp", "gk-fade", "gk-new", "gk-mine", "gk-off", "s2-btn-disabled"],
    width: "s2-w0..s2-w10",
  },
  hud: describe(hud, "s2script_gamehud.xml"),
  panels: describe(pn, "s2script_gamepanels.xml"),
};
const manifest = {
  generatedBy: "scripts/gen-gamehud-layout.mjs",
  ...descriptor,
  panelIds: { hud: hud.st.ids, panels: pn.st.ids },
  dialogVariables: { hud: hud.st.vars, panels: pn.st.vars },
};
const manifestOut = JSON.stringify(manifest, null, 2) + "\n";
const jsOut = `// @s2script/cs2 — game-kit layout descriptors.
// GENERATED by scripts/gen-gamehud-layout.mjs from the same numbers as the workshop markup.
// Do not edit by hand.
(function () {
  globalThis.__s2pkg_gamekit_layout = Object.freeze(${JSON.stringify(descriptor, null, 2).replace(/\n/g, "\n  ")});
})();
`;

// ---- write / check ----------------------------------------------------------------------------
const outputs = [
  [join(LAYOUT_DIR, "s2script_gamehud.xml"), hudXml],
  [join(LAYOUT_DIR, "s2script_gamepanels.xml"), panelsXml],
  [OUT_CSS, cssOut],
  [OUT_MANIFEST, manifestOut],
  [OUT_JS, jsOut],
];
const summary = `hud ${hud.st.ids.length} ids/${hud.st.vars.length} vars, panels ${pn.st.ids.length} ids/${pn.st.vars.length} vars`;
if (process.argv.includes("--check")) {
  let stale = 0;
  for (const [path, body] of outputs) {
    let cur = null;
    try { cur = readFileSync(path, "utf8"); } catch { /* missing */ }
    if (cur !== body) { console.error(`stale: ${path.slice(ROOT.length + 1)}`); stale++; }
  }
  if (stale) { console.error("run: node scripts/gen-gamehud-layout.mjs"); process.exit(1); }
  console.log(`game-kit layouts up to date (${summary})`);
} else {
  for (const [path, body] of outputs) { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, body); }
  console.log(`wrote game-kit layouts (${summary})`);
}

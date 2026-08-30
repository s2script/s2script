/**
 * A HUD that actually works today.
 *
 * The update's `custom_hud_layout` is not usable from a plugin: its layout asset does not load on a
 * stock map (panelIds/classNames read 0), and the two setters that would drive classes and text take
 * `CUtlString` arguments s2script's engine-call ABI cannot express — see the long note in
 * gamedata/hud-lab.gamedata.jsonc, and the server crash that proved it.
 *
 * So this renders on the surface CS2 already gives a server: the centre-screen HTML panel, driven by
 * the `show_survival_respawn_status` game event. That is the same mechanism `MenuStyle.Center`
 * already uses (games/cs2/js/pawn.js), so it is proven on this build — it is what SourceMod's
 * `PrintToCenterHtml` does, and it supports real markup: `<font>` with `color` and the CS2 size
 * classes (`fontSize-l/m/sm/s`), `<br>`, and HTML entities.
 *
 * TWO THINGS THAT LOOK LIKE BUGS AND ARE NOT:
 *
 *  1. CS2 paints `loc_token` for ONE FRAME. The panel must be re-sent every tick or it vanishes.
 *     That is why this owns a post-phase `onGameFrame` subscription rather than sending once.
 *  2. The client filters the event on `userid`. Sending without the target's real userid silently
 *     shows nothing, so every send carries `Player.userId` — not the field's zero default.
 *
 * The panel is one flowing text block with no reserved regions, so layout is line-budgeted: a fixed
 * number of `<br>`-separated rows. Exceed the budget and rows push off the bottom of the screen.
 */
import { Events, createScope } from "@s2script/sdk";
import { Player, GameRules } from "@s2script/cs2";
import type { Pawn as PawnType } from "@s2script/cs2";

/** The event CS2 renders as centre-screen HTML. */
const HUD_EVENT = "show_survival_respawn_status";

/**
 * Lifetime in seconds stamped on each frame. Integer (the field is an int) and > one frame interval
 * so there is no flicker between re-sends, but small enough that the panel self-clears within ~1s of
 * us stopping — the same reasoning as the centre menu renderer's MENU_TTL.
 */
const HUD_TTL = 1;

/** Escape text that goes inside the panel — a player name is attacker-controlled markup otherwise. */
function esc(s: string): string {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** CS2 centre-panel font tiers, largest to smallest. */
const FONT = { title: "fontSize-m", body: "fontSize-sm", foot: "fontSize-s" } as const;

function row(text: string, color: string, size: string = FONT.body): string {
  return `<br><font class='${size}' color='${color}'>${text}</font>`;
}

/** Colour a value by how healthy it is — the whole point of a HUD is reading it at a glance. */
function healthColor(hp: number): string {
  if (hp > 66) return "#4ade80";
  if (hp > 33) return "#facc15";
  return "#f87171";
}

/** mm:ss for the round clock; clamps negatives (the engine can report past-zero briefly). */
function fmtClock(seconds: number): string {
  const t = Math.max(0, Math.floor(seconds));
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, "0")}`;
}

function bar(value: number, max: number, width = 10): string {
  const filled = Math.max(0, Math.min(width, Math.round((value / max) * width)));
  // Block glyphs rather than a nested table: the panel is a text block, so this is the only
  // meter that survives it.
  return "&#9608;".repeat(filled) + "&#9617;".repeat(width - filled);
}

/** One line of the HUD's data model, so the renderer stays dumb. */
interface HudModel {
  name: string;
  health: number;
  armor: number;
  /** Seconds left on the round clock (null pre-round / no gamerules proxy). */
  timeLeft: number | null;
  team: string;
  weapon: string;
  moveType: number | null;
  buyMenuOpen: boolean | null;
  round: number | null;
  bombPlanted: boolean;
}

function readModel(slot: number): HudModel | null {
  const player = Player.fromSlot(slot);
  const pawn: PawnType | null = player?.pawn ?? null;
  if (!player || !pawn?.isValid) return null;
  const team = pawn.teamNum;
  // One read of the gamerules proxy per frame, not three.
  const rules = GameRules.get();
  return {
    name: player.playerName ?? `slot ${slot}`,
    health: pawn.health ?? 0,
    armor: pawn.armorValue ?? 0,
    team: team === 3 ? "CT" : team === 2 ? "T" : "SPEC",
    weapon: (pawn.activeWeapon?.ref.name ?? "").replace(/^weapon_/, "") || "none",
    moveType: pawn.moveType,
    buyMenuOpen: pawn.isBuyMenuOpen,
    timeLeft: rules?.timeRemaining ?? null,
    round: rules?.totalRoundsPlayed ?? null,
    bombPlanted: rules?.bombPlanted ?? false,
  };
}

/**
 * Render the model to CS2 centre-panel HTML.
 *
 * Kept separate from the send so the layout can be reasoned about (and changed) without touching the
 * per-tick machinery. Line budget: title + 5 rows + footer = 7.
 */
function render(m: HudModel): string {
  const teamColor = m.team === "CT" ? "#6cb4ee" : m.team === "T" ? "#eab308" : "#9ca3af";
  let html = `<font class='${FONT.title}' color='#ffd700'>s2script HUD</font>`;
  html += row(
    `<font color='${teamColor}'>[${m.team}]</font> ${esc(m.name)}`,
    "#e5e7eb",
  );
  html += row(
    `HP ${bar(m.health, 100)} <font color='${healthColor(m.health)}'>${m.health}</font>` +
      ` &nbsp; AP <font color='#93c5fd'>${m.armor}</font>`,
    "#9ca3af",
  );
  html += row(`WEP <font color='#e5e7eb'>${esc(m.weapon)}</font>`, "#9ca3af");
  const flags: string[] = [];
  if (m.buyMenuOpen) flags.push("BUY");
  if (m.bombPlanted) flags.push("<font color='#f87171'>BOMB</font>");
  if (m.moveType === 7) flags.push("NOCLIP");
  if (m.moveType === 0) flags.push("FROZEN");
  const clock = m.timeLeft === null ? "--:--" : fmtClock(m.timeLeft);
  html += row(
    `ROUND <font color='#e5e7eb'>${m.round ?? "?"}</font> &nbsp; <font color='#e5e7eb'>${clock}</font>` +
      (flags.length ? ` &nbsp; ${flags.join(" ")}` : ""),
    "#9ca3af",
  );
  html += row("sm_hud_demo off &nbsp; to hide", "#6b7280", FONT.foot);
  return html;
}

/**
 * The live HUD: a set of viewer slots, one shared per-frame repaint.
 *
 * SourceMod has only `OnGameFrame` (before simulation). Post-simulation paint is the Metamod
 * post hook, subscribed here via `createScope().server.onGameFrame` — not a fake SM public. The tick returns
 * immediately while the viewer set is empty. That check is a `Set.size` test per frame, which is
 * nothing next to the work it guards (a schema read and an event send per viewer).
 */
export class DemoHud {
  private readonly viewers = new Set<number>();

  constructor() {
    // "post" phase: the model reads fields the engine re-derives during simulation (health after
    // damage, movetype after a move). Reading in "pre" would paint last tick's values.
    // "low" priority: a HUD must never delay gameplay work in the same frame.
    createScope().server.onGameFrame(() => this.tick(), { phase: "post", priority: "low" });
  }

  /** Whether `slot` currently has the HUD up. */
  has(slot: number): boolean {
    return this.viewers.has(slot);
  }

  /** Number of active viewers — what the status board reports. */
  get count(): number {
    return this.viewers.size;
  }

  show(slot: number): void {
    this.viewers.add(slot);
  }

  /** Stop painting. No explicit clear message exists; the panel expires on its own once we stop
   *  re-sending, which is what HUD_TTL is sized for. */
  hide(slot: number): void {
    this.viewers.delete(slot);
  }

  hideAll(): void {
    this.viewers.clear();
  }

  private tick(): void {
    if (this.viewers.size === 0) return;   // the common case — keep it first and cheap
    for (const slot of [...this.viewers]) {
      const player = Player.fromSlot(slot);
      if (!player) { this.viewers.delete(slot); continue; }   // disconnected — stop paying for them
      const model = readModel(slot);
      if (!model) continue;                                    // connected but not spawned: skip this frame
      Events.fireToClient(slot, HUD_EVENT, {
        loc_token: render(model),
        duration: HUD_TTL,
        userid: player.userId,   // REQUIRED: the client filters the event on this
      });
    }
  }
}

/** A one-shot centre-panel message — the `PrintToCenterHtml` shape, for `sm_hud_say`. */
export function sendOnce(slot: number, html: string): boolean {
  const player = Player.fromSlot(slot);
  if (!player) return false;
  return Events.fireToClient(slot, HUD_EVENT, {
    loc_token: html,
    duration: 5,
    userid: player.userId,
  });
}

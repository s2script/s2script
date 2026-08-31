/**
 * A HUD driven by real game state — the demo that shows what `ui` is actually for.
 *
 * Round clock, live scoreboard, your real K/D/A, and a kill feed fed by `player_death`.
 *
 * THE THING THAT MATTERS HERE IS THE UPDATE DISCIPLINE. Every field is a networked engine call, so
 * a naive "push everything each frame" HUD would issue thousands of calls a second across a full
 * server. Two rules keep it cheap, and both are the framework's job rather than the caller's:
 *
 *   1. DIFF BEFORE SEND. Every value is compared against what that player was last sent, and
 *      unchanged values are skipped. A round clock changes ~once a second; the scoreboard changes a
 *      few times a round. Steady-state traffic is near zero.
 *   2. TICK, DON'T FRAME. The refresh runs on a coarse timer, not OnGameFrame. Nothing here is
 *      sub-second, so a 64Hz repaint would be pure waste.
 */
import { delay } from "@s2script/sdk";
import { Player, Teams, GameRules } from "@s2script/cs2";
import type { Hud } from "@s2script/cs2";
import { LIVE_PANELS } from "./livehud";

/** How often the HUD re-reads game state. Fast enough that a 1s clock never visibly stutters. */
const TICK_SECONDS = 0.25;

/** Kill-feed rows the layout declares. A hard cap — panels cannot be created at runtime. */
const FEED_ROWS = LIVE_PANELS.feed.length;

/**
 * mm:ss, clamped — the engine can report past-zero briefly at round end.
 *
 * CEIL, not floor. A countdown that reads "0:00" for a whole second before the round actually ends
 * is wrong in the direction people notice; every game clock rounds up so the last visible second is
 * a real one. `GameRulesView.timeRemaining` is documented to match the in-game HUD clock, so the
 * value needs no correction — only the display convention has to agree.
 */
function clock(seconds: number | null): string {
  if (seconds === null) return "--:--";
  const t = Math.max(0, Math.ceil(seconds));
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, "0")}`;
}

/** One kill, newest first. */
interface FeedEntry { attacker: string; weapon: string; victim: string; headshot: boolean }

export class LiveDemo {
  private readonly viewers = new Set<number>();
  /** Last value sent per (slot, field) — the diff cache that keeps this cheap. */
  private readonly sent = new Map<string, string>();
  private readonly feed: FeedEntry[] = [];
  private running = false;

  constructor(private readonly hud: Hud, private readonly log: (s: string) => void) {}

  get count(): number { return this.viewers.size; }
  has(slot: number): boolean { return this.viewers.has(slot); }

  /** Send only if the value actually changed for this player. */
  private put(slot: number, id: string, value: string | number): void {
    const v = String(value);
    const key = `${slot}:${id}`;
    if (this.sent.get(key) === v) return;
    const err = this.hud.set(slot, id, v);
    if (err) { this.log(`set ${id} refused: ${err}`); return; }
    this.sent.set(key, v);
  }

  /** Same diffing for classes, which are equally networked. */
  private putClass(slot: number, panelId: string, className: string, on: boolean): void {
    const key = `${slot}:${panelId}.${className}`;
    const v = on ? "1" : "0";
    if (this.sent.get(key) === v) return;
    if (this.hud.setClass(slot, panelId, className, on)) return;
    this.sent.set(key, v);
  }

  /** Every panel this demo owns. Order matters only for readability. */
  private static readonly CHROME = ["timer", "team_ct", "team_t", "pcard"] as const;

  /**
   * Collapse everything for a player.
   *
   * REQUIRED ON CONNECT, not just on stop. The layout's panels carry no hide class in the markup,
   * so they default visible — and a panel renders as soon as the player has ANY per-player state on
   * the layout. Without this, the chrome appears (empty) the instant anything touches the layout for
   * them, before they have asked for a HUD at all.
   */
  hideAll(slot: number): void {
    for (const id of LiveDemo.CHROME) this.hud.hide(slot, id);
    for (const row of LIVE_PANELS.feed) this.hud.hide(slot, row);
  }

  start(slot: number): void {
    this.viewers.add(slot);
    // FILL FIRST, REVEAL SECOND. Showing before painting makes the HUD appear empty and then
    // populate — a visible flicker on every open. Panels are still hidden while these writes land,
    // so the player sees nothing until the whole thing is ready.
    this.paint(slot);
    this.paintFeed(slot);
    for (const id of LiveDemo.CHROME) this.hud.show(slot, id);
    this.arm();
  }

  stop(slot: number): void {
    this.viewers.delete(slot);
    this.hideAll(slot);
    this.forget(slot);
  }

  /** Drop a disconnected player's diff cache so it cannot grow without bound. */
  forget(slot: number): void {
    for (const k of [...this.sent.keys()]) if (k.startsWith(`${slot}:`)) this.sent.delete(k);
  }

  /** Record a kill and repaint the feed for everyone watching. */
  pushKill(e: FeedEntry): void {
    this.feed.unshift(e);
    // The row count is fixed in the markup; older entries simply fall off.
    if (this.feed.length > FEED_ROWS) this.feed.length = FEED_ROWS;
    for (const slot of this.viewers) this.paintFeed(slot);
  }

  private paintFeed(slot: number): void {
    // A row is only revealed for a player who actually has the HUD open; otherwise filling the feed
    // would pop rows onto the screen of someone who never asked for one.
    const open = this.viewers.has(slot);
    for (let i = 0; i < FEED_ROWS; i++) {
      const row = LIVE_PANELS.feed[i];
      const e = this.feed[i];
      if (!e || !open) { this.putClass(slot, row, "s2-hide", true); continue; }
      this.putClass(slot, row, "s2-hide", false);
      this.put(slot, `feed_${i}_a`, e.attacker);
      this.put(slot, `feed_${i}_w`, e.weapon);
      this.put(slot, `feed_${i}_v`, e.victim);
      this.put(slot, `feed_${i}_t`, e.headshot ? "HS" : "");
      this.putClass(slot, `feed_${i}_t`, "s2-feed-hs", e.headshot);
    }
  }

  private paint(slot: number): void {
    const p = Player.fromSlot(slot);
    // A player who has LEFT is dropped. A player who is merely DEAD is not: the controller
    // survives death, only the pawn goes away, and dropping the viewer here would silently stop
    // their HUD the moment they died.
    if (!p) { this.viewers.delete(slot); this.forget(slot); return; }
    const rules = GameRules.get();

    // Round clock + the meter, which is a step class family rather than a number.
    const left = rules?.timeRemaining ?? null;
    const total = rules?.roundTime ?? null;
    this.put(slot, "timer_label", rules?.warmupPeriod ? "WARMUP" : "ROUND");
    this.put(slot, "timer_value", clock(left));
    if (left !== null && total) this.hud.setMeter(slot, "timer", (left / total) * 100);
    // Colour states: amber under 30s, red once the bomb is down.
    this.putClass(slot, "timer", "s2-timer-low", left !== null && left <= 30);
    this.putClass(slot, "timer", "s2-timer-bomb", rules?.bombPlanted === true);

    // Live scoreboard. Team ids: 2 = T, 3 = CT.
    this.put(slot, "team_ct_name", "COUNTER-TERRORISTS");
    this.put(slot, "team_ct_score", Teams.getScore(3) ?? 0);
    this.put(slot, "team_t_name", "TERRORISTS");
    this.put(slot, "team_t_score", Teams.getScore(2) ?? 0);

    // The viewer's own card, from real match stats.
    const st = p.matchStats;
    const k = st?.kills ?? 0, d = st?.deaths ?? 0, a = st?.assists ?? 0;
    this.put(slot, "pcard_name", p.playerName ?? `slot ${slot}`);
    // `pawn` is null while dead — say so rather than rendering a misleading "0 HP · 0 AP", which
    // reads as a live player at zero health.
    const pawn = p.pawn;
    this.put(slot, "pcard_meta", pawn?.isValid
      ? `${pawn.health ?? 0} HP · ${pawn.armorValue ?? 0} AP`
      : "DEAD");
    this.put(slot, "pcard_k", k);
    this.put(slot, "pcard_d", d);
    this.put(slot, "pcard_a", a);
    // K/D rather than HS% — headshots are not exposed on MatchStats, and inventing a number that
    // looks authoritative is worse than showing one that is.
    this.put(slot, "pcard_hs", d > 0 ? (k / d).toFixed(2) : String(k));
    this.put(slot, "pcard_form_label", "K / D");
    this.put(slot, "pcard_badge_t", rules?.bombPlanted ? "BOMB" : "LIVE");
    this.putClass(slot, "pcard_badge", "s2-badge-bad", rules?.bombPlanted === true);
    this.putClass(slot, "pcard_badge", "s2-badge-good", rules?.bombPlanted !== true);
  }

  /** One shared loop for every viewer; it exits when the last one leaves. */
  private arm(): void {
    if (this.running) return;
    this.running = true;
    void (async () => {
      // EVERY paint is guarded. An unhandled throw in here rejects the promise and kills the loop
      // for EVERY viewer, permanently and silently — the HUD just freezes at its last values and
      // looks like a desync rather than a crash. One bad player must not stop the world, so a
      // failing paint is logged once and the loop carries on.
      const complained = new Set<number>();
      while (this.viewers.size > 0) {
        for (const slot of [...this.viewers]) {
          try {
            this.paint(slot);
            complained.delete(slot);
          } catch (err) {
            if (!complained.has(slot)) {
              complained.add(slot);
              this.log(`paint threw for slot ${slot} (HUD continues): ${String(err)}`);
            }
          }
        }
        try {
          await delay(TICK_SECONDS);
        } catch {
          // Even the timer must not be able to end the loop; drop out cleanly instead of rejecting.
          break;
        }
      }
      this.running = false;
    })();
  }
}

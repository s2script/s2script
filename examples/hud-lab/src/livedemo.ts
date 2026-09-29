/**
 * A HUD driven by real game state, built from the shared `hudkit` components — no layout of its own.
 *
 * Round clock (top-left badge), live scoreboard (top-right badge), your K/D/A (bottom-left badge),
 * and a kill feed as toasts fed by `player_death`. Everything renders from `s2script_lib`, which
 * every client already has, so nothing here needs a workshop republish.
 *
 * UPDATE DISCIPLINE. Every field is a networked engine call. The refresh runs on a coarse timer,
 * not OnGameFrame, and hudkit diffs each value against what that player was last sent, so a round
 * clock costs about one call a second and the scoreboard a few a round.
 */
import { delay } from "@s2script/sdk";
import { Player, Teams, GameRules, hudkit } from "@s2script/cs2";
import type { Badge } from "@s2script/cs2";

/** How often the HUD re-reads game state. Fast enough that a 1s clock never visibly stutters. */
const TICK_SECONDS = 0.25;

/**
 * mm:ss, clamped — the engine can report past-zero briefly at round end.
 *
 * CEIL, not floor: a countdown that reads "0:00" for a whole second before the round ends is wrong
 * in the direction people notice. `GameRulesView.timeRemaining` matches the in-game clock.
 */
function clock(seconds: number | null): string {
  if (seconds === null) return "--:--";
  const t = Math.max(0, Math.ceil(seconds));
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, "0")}`;
}

/** One kill. */
export interface FeedEntry { attacker: string; weapon: string; victim: string; headshot: boolean }

export class LiveDemo {
  private readonly viewers = new Set<number>();
  private running = false;
  private badges: { round: Badge; score: Badge; card: Badge } | null = null;

  constructor(private readonly log: (s: string) => void) {}

  get count(): number { return this.viewers.size; }
  has(slot: number): boolean { return this.viewers.has(slot); }

  /**
   * Claim the three corner badges from the shared pool on first use. Returns an error when the
   * pool is exhausted (other plugins hold them), so the caller can say so instead of failing late.
   */
  private claim(): string | null {
    if (this.badges) return null;
    const round = hudkit.badge({ corner: "tl", title: "ROUND" });
    const score = hudkit.badge({ corner: "tr", title: "SCORE" });
    const card = hudkit.badge({ corner: "bl", title: "YOU" });
    if (!round || !score || !card) {
      for (const b of [round, score, card]) b?.release();
      return "hudkit badge pool exhausted (another plugin holds the corners)";
    }
    this.badges = { round, score, card };
    return null;
  }

  /** Start painting for a player. Returns an error string when the badges could not be claimed. */
  start(slot: number): string | null {
    const err = this.claim();
    if (err) return err;
    this.viewers.add(slot);
    this.paint(slot);
    this.arm();
    return null;
  }

  stop(slot: number): void {
    this.viewers.delete(slot);
    if (this.badges) for (const b of Object.values(this.badges)) b.hide(slot);
    if (this.viewers.size === 0) this.release();
  }

  /** Return the badges to the pool once nobody is watching. */
  private release(): void {
    if (!this.badges) return;
    for (const b of Object.values(this.badges)) b.release();
    this.badges = null;
  }

  /** Show the kill to everyone watching, as a short toast. */
  pushKill(e: FeedEntry): void {
    const message = `${e.attacker}  [${e.weapon}]  ${e.victim}${e.headshot ? "  HS" : ""}`;
    for (const slot of this.viewers) {
      const err = hudkit.toast(slot, { title: "KILL", message, variant: e.headshot ? "warn" : "ghost", holdSeconds: 4 });
      if (err) this.log(`kill toast refused for slot ${slot}: ${err}`);
    }
  }

  private paint(slot: number): void {
    const p = Player.fromSlot(slot);
    // A player who has LEFT is dropped. A player who is merely DEAD is not: the controller
    // survives death, only the pawn goes away.
    if (!p || !this.badges) { this.stop(slot); return; }
    const rules = GameRules.get();

    const left = rules?.timeRemaining ?? null;
    const phase = rules?.warmupPeriod ? "WARMUP" : rules?.bombPlanted ? "BOMB PLANTED" : "ROUND";
    this.badges.round.show(slot, { title: phase, text: clock(left) });

    // Team ids: 2 = T, 3 = CT.
    this.badges.score.show(slot, { title: "SCORE", text: `CT ${Teams.getScore(3) ?? 0}  :  ${Teams.getScore(2) ?? 0} T` });

    const st = p.matchStats;
    const k = st?.kills ?? 0, d = st?.deaths ?? 0, a = st?.assists ?? 0;
    // `pawn` is null while dead — say so rather than rendering a misleading "0 HP".
    const pawn = p.pawn;
    const life = pawn?.isValid ? `${pawn.health ?? 0} HP · ${pawn.armorValue ?? 0} AP` : "DEAD";
    const kd = d > 0 ? (k / d).toFixed(2) : String(k);
    this.badges.card.show(slot, { title: p.playerName ?? `slot ${slot}`, text: `${k} / ${d} / ${a}  ·  K/D ${kd}  ·  ${life}` });
  }

  /** One shared loop for every viewer; it exits when the last one leaves. */
  private arm(): void {
    if (this.running) return;
    this.running = true;
    void (async () => {
      // Every paint is guarded: one throw would otherwise end the loop for every viewer and freeze
      // their HUD at its last values.
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
          break;
        }
      }
      this.running = false;
    })();
  }
}

import type { Recipe } from "../recipe.ts";
import { after, every, delay, type Timer } from "@s2script/sdk/timers";
import { command } from "@s2script/sdk/commands";

/**
 * @s2script/timers — cancellable callback timers (SourceMod `CreateTimer` / `KillTimer`).
 *
 * `delay(ms)` is a Promise and cannot be cancelled; `after`/`every` return a {@link Timer} handle
 * with `kill()` and `alive`. Every timer is ledgered against the plugin, so unload kills it whether
 * or not you call `kill()` — a repeating timer can never outlive its plugin.
 */
export const timersRecipe: Recipe = {
  name: "timers",
  describe: "cancellable timers: sm_timer_once, sm_timer_repeat <ms>, sm_timer_stop, sm_timer_status",

  register() {
    let ticker: Timer | null = null;
    let ticks = 0;

    command("sm_timer_once", (cmd) => {
      const ms = cmd.argInt(0, 3000);
      const t = after(ms, () => console.log(`[cookbook] timers: one-shot fired after ${ms}ms`));
      cmd.reply(`[cookbook] timers: armed a one-shot for ${ms}ms (alive=${t.alive})`);
    });

    command("sm_timer_repeat", (cmd) => {
      const ms = Math.max(cmd.argInt(0, 1000), 1);
      ticker?.kill();                       // kill() is idempotent, so no alive-check needed
      ticks = 0;
      ticker = every(ms, () => {
        ticks++;
        console.log(`[cookbook] timers: tick ${ticks} (every ${ms}ms)`);
        // Self-kill from inside the callback is supported and must NOT re-arm.
        if (ticks >= 5) { ticker?.kill(); console.log("[cookbook] timers: self-killed at 5 ticks"); }
      });
      cmd.reply(`[cookbook] timers: repeating every ${ms}ms, self-kills after 5 ticks`);
    });

    command("sm_timer_stop", (cmd) => {
      const killed = ticker?.kill() ?? false;
      cmd.reply(`[cookbook] timers: kill() -> ${killed} (false = already dead; it is idempotent)`);
    });

    command("sm_timer_status", (cmd) => {
      cmd.reply(`[cookbook] timers: ticks=${ticks} alive=${ticker?.alive ?? false}`);
    });

    // A 0ms repeat would re-arm every drain and starve the frame, so it throws rather than degrade.
    command("sm_timer_zero", (cmd) => {
      try {
        every(0, () => {});
        cmd.reply("[cookbook] timers: UNEXPECTED — every(0) should have thrown");
      } catch (e) {
        cmd.reply(`[cookbook] timers: every(0) correctly threw ${(e as Error).constructor.name}`);
      }
    });

    // delay() still exists for the await-able case; shown so the two are contrasted in one place.
    command("sm_timer_delay", (cmd) => {
      cmd.reply("[cookbook] timers: awaiting delay(1000)…");
      delay(1000).then(() => console.log("[cookbook] timers: delay(1000) resolved (not cancellable)"));
    });
  },
};

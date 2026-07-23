import type { Recipe } from "../recipe.ts";
import { fetch } from "@s2script/sdk/http";

/**
 * fetch() runs off-thread on the shared tokio runtime and resolves back on a
 * game frame, so awaiting it never blocks the server. This fires several
 * concurrent requests; the frame counter proves the tick keeps advancing the
 * whole time they're in flight. Bodies are capped at 10MB.
 */
export const httpRecipe: Recipe = {
  name: "http",
  describe: "fire concurrent fetch()es without blocking the tick (sm_http)",
  register(ctx) {
    let frames = 0;
    ctx.server.onGameFrame(() => { frames += 1; });

    ctx.commands.register("sm_http", (cmd) => {
      const start = frames;
      const N = 10;
      cmd.reply(`firing ${N} concurrent fetches…`);
      Promise.all(
        Array.from({ length: N }, (_unused, i) =>
          fetch(`https://postman-echo.com/get?i=${i}`, { timeoutMs: 15000 })
            .then((r) => r.status)
            .catch((e: unknown) => `ERR:${String(e)}`)
        )
      ).then((results) => {
        const ok = results.filter((s) => s === 200).length;
        const elapsed = frames - start;
        console.log(`[cookbook] http: ${ok}/${N} ok; tick advanced ${elapsed} frames while the fetches were in flight`);
        // cmd is safe to hold across this await (it's a plain closure, no native handle) — but it
        // still targets the caller's SLOT, so if they disconnected mid-fetch and someone else has
        // since taken that slot, this reply lands on the new occupant instead.
        cmd.reply(`${ok}/${N} ok — tick advanced ${elapsed} frames meanwhile (see log)`);
      });
    });
  },
};

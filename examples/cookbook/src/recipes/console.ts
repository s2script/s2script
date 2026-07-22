import type { Recipe } from "../recipe.ts";
import { console } from "@s2script/sdk/console";

/**
 * The engine also injects `console` as an ambient global (see globals.d.ts —
 * every other recipe in this cookbook calls bare `console.log(...)` and never
 * imports anything). `@s2script/sdk/console` exports the identical shape as a
 * named binding for the rare file that wants the dependency explicit in its
 * imports rather than relying on ambient scope. Functionally the same: all
 * four methods stringify + space-join their arguments and write to the
 * server console and log file, differing only in the severity label attached
 * to the line.
 */
export const consoleRecipe: Recipe = {
  name: "console",
  describe: "the engine console via an explicit import, not the ambient global (cb_console)",
  register(ctx) {
    ctx.commands.register("cb_console", (cmd) => {
      console.log("[cookbook] console.log — informational");
      console.info("[cookbook] console.info — same severity as log, a semantic label only");
      console.warn("[cookbook] console.warn — flagged at warning severity");
      console.error("[cookbook] console.error — flagged at error severity");
      cmd.reply("wrote one line at each console severity — see server log");
    });
  },
};

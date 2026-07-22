import type { Recipe } from "../recipe.ts";
import { config } from "@s2script/sdk/config";

/**
 * A plugin declares its config under `s2script.config` in package.json — see this cookbook's own
 * package.json for the `greeting` string declared alongside this recipe. On first load the host
 * materializes that declaration to an operator-editable file (SourceMod-parity: named by plugin,
 * `addons/s2script/configs/<id>.jsonc`); it is never rewritten behind an operator's edits, and a
 * key the operator hasn't touched still falls back to its declared default at read time.
 *
 * `config.getString/getInt/getFloat/getBool` read the CURRENT materialized value — call them
 * fresh rather than caching the result, because `ctx.config.onChange` fires whenever an operator
 * edits the file live, and re-reading inside that handler is how a plugin picks up the edit
 * without a restart (see plugins/antiflood for a config-driven plugin doing exactly this).
 */
export const configRecipe: Recipe = {
  name: "config",
  describe: "read the plugin's operator-editable config file, live-reloadable (cb_config)",
  register(ctx) {
    ctx.config.onChange(() => {
      console.log("[cookbook] config changed — greeting=" + JSON.stringify(config.getString("greeting")));
    });

    ctx.commands.register("cb_config", (cmd) => {
      cmd.reply("greeting = " + JSON.stringify(config.getString("greeting")));
      cmd.reply("edit the materialized config file, save, then re-run cb_config to see it live-reload");
    });
  },
};

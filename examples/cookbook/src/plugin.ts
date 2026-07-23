// cookbook — one file per API, all registered under a single plugin.
//
// Browse src/recipes/ for the API you want; each file is self-contained and
// readable on its own. Run `sm_list` on a server to see everything registered.
//
// This is a DEMO plugin: it registers a lot of commands and is not part of the
// shipped release. Copy a recipe into your own plugin rather than loading this.
import { plugin } from "@s2script/sdk/plugin";
import { RECIPES } from "./recipes/index.ts";

export default plugin((ctx) => {
  for (const recipe of RECIPES) {
    recipe.register(ctx);
  }

  ctx.commands.register("sm_list", (cmd) => {
    cmd.reply(`${RECIPES.length} recipes:`);
    for (const r of RECIPES) cmd.reply(`  sm_${r.name} — ${r.describe}`);
  });

  console.log(`[cookbook] loaded ${RECIPES.length} recipes — run sm_list`);
});

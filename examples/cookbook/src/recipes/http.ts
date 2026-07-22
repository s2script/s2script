import type { Recipe } from "../recipe.ts";
import { fetch } from "@s2script/sdk/http";

/**
 * fetch() runs off-thread on the shared tokio runtime and resolves back on a
 * game frame, so awaiting it never blocks the server. Bodies are capped at 10MB.
 */
export const httpRecipe: Recipe = {
  name: "http",
  describe: "fetch() an HTTP endpoint without blocking the server",
  register(ctx) {
    ctx.commands.register("cb_http", (cmd) => {
      cmd.reply("fetching…");
      fetch("https://api.github.com/repos/s2script/s2script")
        .then((res) => res.json())
        .then((body: unknown) => {
          const repo = body as { stargazers_count?: number };
          cmd.reply(`ok — stars: ${repo.stargazers_count ?? "?"}`);
        })
        .catch((e: unknown) => cmd.reply(`failed: ${String(e)}`));
    });
  },
};

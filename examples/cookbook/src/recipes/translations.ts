import type { Recipe } from "../recipe.ts";
import { Translations } from "@s2script/sdk/translations";

/**
 * Translations.load seeds a phrase set (the built-in English default);
 * translations/<code>/<name>.phrases.json overrides it per language, and a
 * missing key falls back to the seed. cmd.replyT reads the caller's language.
 */
export const translationsRecipe: Recipe = {
  name: "translations",
  describe: "seed phrases, positional {1} formatting, and a language switch (sm_translations)",
  register(ctx) {
    Translations.load("trdemo", { Greeting: "Hello {1}", Bye: "Goodbye {1}", OnlyEn: "English only" });

    ctx.commands.register("sm_translations", (cmd) => {
      // default (root / English) — slot -1 uses the server default ("" = root)
      console.log(`[cookbook] translations en: ${Translations.translate(-1, "Greeting", "world")}`);       // Hello world
      console.log(`[cookbook] translations en missing-key: ${Translations.translate(-1, "Nope")}`);         // Nope (fallback)

      // switch the server default to German -> reads translations/de/trdemo.phrases.json (operator-seeded)
      Translations.setDefaultLanguage("de");
      console.log(`[cookbook] translations de: ${Translations.translate(-1, "Greeting", "world")}`);        // Hallo world (from de file)
      console.log(`[cookbook] translations de fallback-to-seed: ${Translations.translate(-1, "OnlyEn")}`);  // English only (de miss -> seed)
      Translations.setDefaultLanguage("");

      cmd.replyT("Greeting", "admin");
    });
  },
};

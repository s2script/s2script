// translations-demo — proves @s2script/translations: a seed English default, positional {1} formatting,
// the default-language switch reading translations/de/trdemo.phrases.json live, and the key fallback.
import { Translations } from "@s2script/translations";
import { Commands } from "@s2script/commands";

export function onLoad(): void {
  Translations.load("trdemo", { Greeting: "Hello {1}", Bye: "Goodbye {1}", OnlyEn: "English only" });

  // default (root / English) — slot -1 uses the server default ("" = root)
  console.log(`[translations-demo] en: ${Translations.translate(-1, "Greeting", "world")}`);      // Hello world
  console.log(`[translations-demo] en missing-key: ${Translations.translate(-1, "Nope")}`);        // Nope (fallback)

  // switch the server default to German -> reads translations/de/trdemo.phrases.json (operator-seeded)
  Translations.setDefaultLanguage("de");
  console.log(`[translations-demo] de: ${Translations.translate(-1, "Greeting", "world")}`);       // Hallo world (from de file)
  console.log(`[translations-demo] de fallback-to-seed: ${Translations.translate(-1, "OnlyEn")}`);  // English only (de miss -> seed)
  Translations.setDefaultLanguage("");

  // ctx.replyT from the console
  Commands.register("sm_trhello", (ctx) => { ctx.replyT("Greeting", "admin"); });
  console.log("[translations-demo] onLoad — sm_trhello registered");
}

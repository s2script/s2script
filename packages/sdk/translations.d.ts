/** @s2script/translations — SourceMod-style i18n (per-client language, phrase files, {1} formatting). */

/** A phrase set: key → template string, where `{1}`/`{2}`/… are positional substitution slots. */
export type Phrases = Record<string, string>;
/**
 * SourceMod-style translation registry: seed a default phrase set, override it per language from
 * `translations/<code>/<name>.phrases.json`, then translate a key for a given client's language.
 * @example
 * import { Translations } from "@s2script/sdk/translations";
 * Translations.load("trdemo", { Greeting: "Hello {1}", Bye: "Goodbye {1}" });
 * console.log(Translations.translate(-1, "Greeting", "world")); // "Hello world"
 */
export declare const Translations: {
  /** Register a phrase set: `seed` is the built-in English default; translations/<code>/<name>.phrases.json overrides per language. */
  load(name: string, seed: Phrases): void;
  /** Translate `key` for `slot`'s language (slot < 0 = the server default), substituting positional {1}/{2} args. */
  translate(slot: number, key: string, ...args: (string | number)[]): string;
  /** Set the server/console default language code (default "" = root/English). */
  setDefaultLanguage(code: string): void;
};

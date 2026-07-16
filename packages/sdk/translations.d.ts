/** @s2script/translations — SourceMod-style i18n (per-client language, phrase files, {1} formatting). */
export type Phrases = Record<string, string>;
export declare const Translations: {
  /** Register a phrase set: `seed` is the built-in English default; translations/<code>/<name>.phrases.json overrides per language. */
  load(name: string, seed: Phrases): void;
  /** Translate `key` for `slot`'s language (slot < 0 = the server default), substituting positional {1}/{2} args. */
  translate(slot: number, key: string, ...args: (string | number)[]): string;
  /** Set the server/console default language code (default "" = root/English). */
  setDefaultLanguage(code: string): void;
};

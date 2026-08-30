/** @s2script/translations — SourceMod-style i18n (per-client language, phrase files, {1} formatting). */
import type { PhraseKey } from "./phrases";

/** A phrase set: key → template string, where `{1}`/`{2}`/… are positional substitution slots. */
export type Phrases = Record<string, string>;

/**
 * SourceMod-style translation registry. Phrases live in `translations/<name>.phrases.json`, shipped
 * with the addon and edited by operators; `translations/<code>/<name>.phrases.json` overrides per
 * language, added by a translator with no code change.
 *
 * A plugin declares the files it uses with {@link PluginContext.translations} — nothing is loaded
 * for it automatically, the same rule SourceMod's `LoadTranslations` enforces:
 *
 * @example
 * import { command, translations } from "@s2script/sdk";
 * import { ADMFLAG } from "@s2script/sdk";
 * export function OnPluginStart(): void {
 *   translations.load("basecomm", "common");
 *   command.admin("sm_gag", ADMFLAG.CHAT, (cmd) => {
 *     cmd.replyT("Usage Gag");                                  // key-checked
 *     Chat.toSlot(p.slot, Translations.translate(p.slot, "Gagged Player", n));
 *   });
 * }
 *
 * Keys are checked against the files that plugin loads — see `@s2script/sdk/phrases`.
 */
export declare const Translations: {
  /**
   * Register a phrase set by name, populating it from `translations/<name>.phrases.json`.
   *
   * Prefer {@link PluginContext.translations}`.load(...)`, which is what the build reads to work out
   * which keys your plugin may use. This is the lower-level form, for a name computed at runtime or
   * a set with an in-code default — a `seed` here is the starting content, which the file overrides.
   *
   * Registration order is significant: `translate` takes the first hit within each of its two passes
   * (the client's language, then English), so load your own set before any shared one if you want to
   * override a shared phrase.
   */
  load(name: string, seed?: Phrases): void;
  /**
   * Translate `key` for `slot`'s language (slot < 0 = the server default), substituting positional
   * {1}/{2} args.
   *
   * `key` is checked against the phrase files this plugin loads; it widens to `string` in a plugin
   * that loads none, so this is never in the way. A key found in no loaded set returns the key
   * itself — which is what the checking exists to prevent reaching a player.
   */
  translate(slot: number, key: PhraseKey, ...args: (string | number)[]): string;
  /** Set the server/console default language code (default "" = root/English). */
  setDefaultLanguage(code: string): void;
};

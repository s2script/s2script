/**
 * @s2script/phrases — the phrase keys your plugin is allowed to use.
 *
 * **You do not import this module.** It exists so that `cmd.replyT(...)` and
 * `Translations.translate(...)` know which keys are valid in your plugin, and the build fills it in
 * from the phrase files you load.
 *
 * @example
 * import { translations } from "@s2script/sdk";
 * export function OnPluginStart(): void {
 *   translations.load("basecomm", "common");
 *
 *   cmd.replyT("Usage Gag");            // ok — in translations/basecomm.phrases.json
 *   cmd.replyT("No matching players");  // ok — in translations/common.phrases.json
 *   cmd.replyT("Usage Gagg");           // compile error — no such key in either
 * }
 *
 * ## Writing phrases
 *
 * Create `translations/<your-plugin>.phrases.json` — a flat key to English map. `{1}`, `{2}` … are
 * positional slots filled by the trailing arguments; `{green}`, `{default}` … are colour tags
 * expanded on output (see `@s2script/sdk/chat`). Keys are human-readable English on purpose: an
 * unresolved key renders as the key itself, so it should still read as a sentence.
 *
 * ```json
 * { "Gagged Player": "{green}[SM]{default} Gagged {1} player." }
 * ```
 *
 * A translator adds `translations/<code>/<your-plugin>.phrases.json` — no code change, no rebuild.
 *
 * ## How the checking works
 *
 * The build reads your `translations.load(...)` call and writes `src/phrases.generated.d.ts`,
 * filling in {@link PhraseKeys} with the keys of exactly those files. Load a file and its keys
 * become valid; don't, and they don't — so a forgotten `load` is a compile error rather than raw key
 * text printed to a player at runtime. SourceMod enforces the same rule, but only when it happens.
 *
 * That file is derived and gitignored. Until it has been written — a brand-new plugin, say — keys
 * fall back to plain `string` and go unchecked, so nothing is blocked on the build having run.
 */

/**
 * The set of valid phrase keys, filled in per plugin by its generated declaration.
 *
 * Empty here deliberately. Each plugin's `src/phrases.generated.d.ts` augments this interface, and
 * interface merging turns that into the exact union {@link PhraseKey} resolves to. Never add keys
 * here by hand — this is the shared module, and a key added here would apply to every plugin.
 */
export interface PhraseKeys {}

/**
 * Every phrase key valid in this plugin — the union of the keys in every file it loads.
 *
 * Falls back to `string` when nothing has augmented {@link PhraseKeys}, so a plugin that loads no
 * phrases (or whose generated declaration has not been written yet) compiles unchecked rather than
 * failing to compile at all.
 */
export type PhraseKey = [keyof PhraseKeys] extends [never] ? string : Extract<keyof PhraseKeys, string>;

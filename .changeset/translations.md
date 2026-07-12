---
"@s2script/translations": patch
"@s2script/commands": patch
---

New `@s2script/translations` package: SourceMod-style i18n — per-client language (via `cl_language`), JSON phrase files (a root English default + `translations/<code>/` per-language folders), positional `{1}` formatting, `Translations.translate` / `Translations.load`. `@s2script/commands`' `CommandContext` gains `replyT(key, ...args)` — reply to the caller in their language.

# @s2script/commands

## 0.1.2

### Patch Changes

- 015c020: New `@s2script/translations` package: SourceMod-style i18n — per-client language (via `cl_language`), JSON phrase files (a root English default + `translations/<code>/` per-language folders), positional `{1}` formatting, `Translations.translate` / `Translations.load`. `@s2script/commands`' `CommandContext` gains `replyT(key, ...args)` — reply to the caller in their language.

## 0.1.1

### Patch Changes

- 5fcc41f: Initial public npm release of the `@s2script/*` types packages and CLI (Changesets pipeline).

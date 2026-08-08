---
"@s2script/sdk": patch
---

Mark `Translations.load`'s `seed` parameter optional in `packages/sdk/translations.d.ts`, matching
the runtime, which has always treated a missing/non-object `seed` as an empty starting set.

The type declaration previously required `seed`, but the shared-phrases design this branch ships
depends on `Translations.load(name)` with no seed at all — a phrase set populated entirely from
`translations/<name>.phrases.json` (SourceMod's `LoadTranslations`, for a shared or third-party
file with no in-code default). Without this fix, that call was a `tsc` error even though it always
worked at runtime.

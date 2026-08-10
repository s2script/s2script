---
"@s2script/sdk": minor
---

Phrase keys are now checked at build time, and a plugin declares the phrase files it uses.

`cmd.replyT(key)` and `Translations.translate(slot, key)` no longer take a bare `string`. Their key
is checked against the phrase files the plugin loads, so a typo — or a key from a file the plugin
forgot to load — is a compile error instead of raw key text printed to a player at runtime.
SourceMod enforces the same rule, but only when the line is reached.

A plugin declares what it uses on its context, in load order:

```ts
export default plugin((ctx) => {
  ctx.translations.load("basecomm", "common");
  // ...
});
```

Nothing is loaded automatically, including the shared `common` set. Order is significant:
`translate` takes the first hit within each of its two passes (the client's language, then English),
so a plugin lists its own set before any shared one to be able to override a shared phrase.

**New:** `ctx.translations` (`CtxTranslations`), and `@s2script/sdk/phrases`, which exports the
`PhraseKey` type. You do not import that module in normal use — it exists so the checking has
somewhere to live, and for the rare helper that takes a phrase key as a parameter:

```ts
function usage(cmd: CommandInvocation, key: PhraseKey) { cmd.replyT(key); }
```

**Also:** `Translations.load`'s `seed` parameter is optional, matching the runtime, which has always
treated a missing seed as an empty starting set. A phrase set is now normally populated entirely
from its file.

**Compatibility.** A plugin that loads no phrase files sees `PhraseKey` widen to `string` and is
unaffected. Existing plugins that call `Translations.load(name, seed)` keep working; they gain key
checking by moving the call to `ctx.translations.load(name)` and putting the phrases in
`translations/<name>.phrases.json`.

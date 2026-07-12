# `@s2script/translations` — i18n — design

**Date:** 2026-07-12
**Status:** approved (design)
**Slice:** translations (SourceMod-style i18n — per-client language, phrase files, formatted messages)

## Goal

SourceMod-style translations: each client sees messages in **their own language**, plugins declare phrases in JSON files (a root/default file + per-language subfolders), positional args are substituted, and `ctx.replyT` answers an admin in their language. A tight primitive + a demo; base-plugin retrofit is a follow-up.

## Background — the pieces that already exist

- **`ctx.reply`** (built in `__s2cmd_ctx`, the engine-generic commands module) is the command reply path — where a localized `ctx.replyT` plugs in.
- **The client's language is reachable with a small engine call** — the shim already calls `s_pEngine->GetClientConVarValue(slot, "name")` for the client name; reading `cl_language` the same way gives each client's UI language (SM's `GetClientLanguage`).
- **The config bridge** (`__s2_config_read_raw`/`__s2_config_write_raw`) shows the file-load pattern (auto-generate a template when absent, as `admins.json` does) — but it reads `configs/` and sanitizes `/` away, so it **cannot** reach a language subfolder; translations need their own read op.

## Scope decisions (locked)

- **Primitive + demo.** Build `@s2script/translations` + a demo exercising the full API. Base-plugin retrofit deferred (a large, mechanical, incremental follow-up).
- **File layout = SourceMod's:** a root file is the default (English); each other language lives in a language-code subfolder.
- **Fully engine-generic** — no CS2 / `packages/cs2` change.
- **Deferred:** typed format specifiers (`{1:d}`); an operator config for the server-default language; base-plugin retrofit; a directory-scan / eager-load-all-languages; `%T`-style explicit-target-language formatting beyond `translate(slot, …)`.

## A. The API — `@s2script/translations`

```ts
Translations.load(name, seed)                        // seed = the inline English default (in-memory);
                                                      //   also reads an OPTIONAL root translations/<name>.phrases.json override
Translations.translate(slot, key, ...args): string   // pick slot's language, look up, substitute, fallback
Translations.setDefaultLanguage(code)                // server/console default (default "" = root/English)
ctx.replyT(key, ...args)                             // reply to the caller in THEIR language (sugar; soft-deps translations)
```

- `seed` is the plugin's built-in English phrase map (`{ key: "text {1}" }`) — the plugin ships English **in code** (functional with zero files); translators add language folders with **no code change** ("layout is data" charter). No file is written by the runtime.
- `Translations.translate` is the primitive; `ctx.replyT(key, ...args)` = `ctx.reply(Translations.translate(ctx.callerSlot, key, ...args))`. `replyT` lives in the commands module and soft-references `globalThis.__s2pkg_translations` — if translations isn't loaded, it degrades to replying the key.

## B. Files + languages (SourceMod layout)

```
addons/s2script/translations/<name>.phrases.json        → root = default (English)
addons/s2script/translations/de/<name>.phrases.json     → German
addons/s2script/translations/ru/<name>.phrases.json     → Russian   …
```
Each file is a **flat `key → text` map** (the folder is the language dimension):
```json
// translations/mytest.phrases.json         (root = en)
{ "Slapped": "Slapped {1}", "NoAccess": "You do not have access." }
// translations/de/mytest.phrases.json
{ "Slapped": "{1} geohrfeigt", "NoAccess": "Kein Zugriff." }
```
- **Load:** `Translations.load(name, seed)` stores `seed` as the in-memory English default, then reads an **optional** root `translations/<name>.phrases.json` — if present, it overrides the seed (an operator can edit the English strings without touching the plugin); if absent, the seed stands. **The runtime never writes a phrases file.**
- **Per-language files are read lazily on first use and cached** — the first `translate` for a German client reads `de/<name>.phrases.json` (no directory-scan op needed).
- **Lookup / fallback chain:** the slot's language file → the root/default (the seed, or the root-file override) → the key itself. Never crashes, never empty.

## C. Two new engine-generic ops (shim)

1. **`translations_read(lang, name) → string | null`** — reads `<addon>/translations/[<lang>/]<name>.phrases.json`. The shim resolves the `translations/` dir via the same `dladdr`+`dirname` walk `ConfigPath` uses, appends the optional `<lang>/` folder, then `<name>.phrases.json`. `lang == ""` → the root file. Each segment (`lang`, `name`) is sanitized to `[A-Za-z0-9._-]` and `..`/empty is rejected (no traversal). Returns the file text or null (absent/unreadable → null, degrade). **Read-only — the runtime never writes a phrases file** (the `seed` is the in-memory default).
2. **`client_language(slot) → string`** — `s_pEngine->GetClientConVarValue(slot, "cl_language")` (the `client_name` sibling). Returns the client's language string (`"english"`, `"german"`, …), or `""` for a bot / no value.

Both ABI-appended, shim-side, engine-generic → one sniper rebuild. Exposed as `__s2_translations_read`/`__s2_client_language`.

## D. Format + language mapping + fallback

- **Format:** positional `{1}`, `{2}`, … replaced by `String(args[i-1])`; an out-of-range `{n}` → left as-is (or empty — pick empty). No typed specifiers.
- **`cl_language` → folder code** via a small built-in table: `english → ""` (root), `german → de`, `russian → ru`, `french → fr`, `spanish → es`, `schinese → zh`, `tchinese → zh`, `portuguese → pt`, `polish → pl`, … (the common Steam UI languages). An unmapped/empty language → the server default.
- **Fallback chain** (in `translate`): `slot < 0` (console/rcon) → the server default language; else `code(client_language(slot))`. Look up the key in that language's file → the root/default file → the key string. Missing args substitute to empty.

## E. Boundary + packaging

- **Fully engine-generic:** `@s2script/translations` is a core prelude module (the phrase registry, format, fallback, the `cl_language`→code map); the two ops are shim/core; `ctx.replyT` is the engine-generic commands module. **No CS2 / `packages/cs2` change.**
- **`packages/translations` is a NEW types-only package** → this slice changes `packages/*`, so: **branch → PR → with a Changesets changeset** (`@s2script/translations`).
- Two new `S2EngineOps` (ABI-appended after the current last op, byte-identical across the C header + Rust mirror + both test op-structs + the shim assignment) → **shim change → one sniper rebuild**.

## F. Testing

**In-isolate (pure, no shim):** the ops degrade in tests (no engine), so test the pure logic by exposing helper hooks and injecting phrase maps:
- Format substitution: `{1}`/`{2}` with in-range args, an out-of-range `{3}`, no args.
- The `cl_language`→code map (`german→de`, `english→` root, unknown→default).
- The fallback chain: a key present only in the root; a key overridden in a language; a missing key → the key; the server-default path (`slot < 0`).
- Parse of a phrases file + `seed`-as-default (a malformed root/lang file → WARN + fall back to the seed, degrade).

**Live gate:** a demo `Translations.load("nettest", { Greeting: "Hello {1}", ... })` (seed = the in-memory English default; no root file needed), plus a hand-seeded `translations/de/nettest.phrases.json`; the demo logs `translate(-1, "Greeting", "world")` (root/en), then `setDefaultLanguage("de")` + `translate(-1, ...)` (reads the `de/` file → German), and a console `ctx.replyT`. Proves load + auto-gen + the `de/` lazy read + format + fallback + the default-language path. **Real per-client language** — a human client with `cl_language="german"` getting German output — is the **human-client deferral** (bots have no `cl_language`); the primitive/format/fallback/default-language are all proven without a human. `GAMEDATA n/0`, `RestartCount=0`, no crash.

## Out of scope (do not build ahead)

- Typed format specifiers (`{1:d}`/`{1:f}`); `%T`-style explicit-language formatting beyond `translate(slot, …)`.
- An operator config file for the server-default language (programmatic `setDefaultLanguage` only this slice).
- Auto-generating the root phrases file from the seed (a `translations_write` op) — the seed is the in-memory default, so a root file is an optional operator override this slice.
- Eager-load-all-languages / a directory-scan op; hot-reload of phrase files.
- Base-plugin retrofit (every base plugin's strings → phrases) — the incremental follow-up.
- A `languages.cfg`-style operator-editable `cl_language`→code map (the built-in table this slice).

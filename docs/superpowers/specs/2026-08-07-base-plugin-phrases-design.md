# Base-plugin i18n retrofit + operator-configurable chat colors — design

**Date:** 2026-08-07
**Status:** approved (design)
**Slice:** base-plugin-phrases (move all 18 shipped plugins onto `@s2script/translations`; make chat color an operator edit)

## Goal

Every user-facing string in every shipped plugin becomes a translatable phrase, and every color in
those strings becomes something a server operator can change by editing a file — not by forking a
plugin. This is the "base-plugin retrofit" the translations slice
(`2026-07-12-translations-design.md`) explicitly deferred as "a large, mechanical, incremental
follow-up".

## Background — what exists, and what is quietly dead

The translations primitive is **complete and entirely unused by shipped code**:

- `Translations.load(name, seed)` / `translate(slot, key, …)` / `setDefaultLanguage(code)` and
  `ctx.replyT` all work, backed by two engine ops (`translations_read`, `client_language`).
- The **only** consumer in the repo is `examples/cookbook/src/recipes/translations.ts`.
- There are **zero** `translations/` directories and **zero** `.phrases.json` files anywhere.

Three facts discovered while designing this slice, each of which changes the shape of the work:

1. **`scripts/package-addon.sh` never creates or copies `translations/`.** Line 59 creates
   `plugins/`, `configs/` and `data/` and nothing else; the word "translations" appears in no build
   script and no Makefile target. In a packaged install `translations_read` therefore always returns
   null, every phrase falls back to its in-code seed, and the entire file mechanism is dead while
   appearing to work. Packaging is a required deliverable of this slice, not a detail of it.
2. **The `<lang>/` path has never been exercised by a real file.** The cookbook comments that
   `translations/de/trdemo.phrases.json` is "operator-seeded" — it is not shipped and does not
   exist. Only the `__s2_tr_injectLang` test hook has ever populated a language map.
3. **`translate` has a set-ordering bug** that is unreachable with one phrase set and becomes
   reachable the moment any plugin loads two. See §D.

Existing pieces this design builds on. **All of the prelude JS now lives in `core/js/prelude.js`**,
a real JavaScript file baked in at compile time by `include_str!` (`core/src/v8host.rs:1240`) — the
result of the #90–#94 extraction series. That is materially convenient here: the functions this
slice changes can be `node:test`ed directly, with no Rust `eval_in_context` harness.

- **`__s2_chatLine()`** (`core/js/prelude.js:678`) is the single funnel for `Chat.toSlot`,
  `Chat.toAll` **and** `ctx.replyToChat` (which routes through `Chat.toSlot`).
- **`__s2cmd_stripCtl()`** (`core/js/prelude.js:900`) is the single funnel for both console reply
  paths (`:934` and `:942`), and already strips `[\x00-\x1F\x7F]` — which is to say, it already
  strips color control bytes.
- **`__s2_tr_format()`** (`core/js/prelude.js:608`) does positional substitution; the `translate`
  set loop is at `:640-647`.
- **`s2script_core_register_package()`** (`core/src/ffi.rs:568`) already injects a game package's JS
  into every plugin context at load, with core never learning which game it is.
- **`ChatColors`** is defined in `games/cs2/js/pawn.js:610` as a frozen `\x01`–`\x10` map — the
  table the expander needs, already assembled.
- **`scripts/ci-js.sh:20-25`** globs `scripts/check-*-generated.sh`, so a new freshness gate wires
  itself in with no edit to the suite.

## Scope decisions (locked)

- **All 18 shipped plugins, one PR** — 14 in `plugins/` (including `zones`) and 4 in
  `plugins/disabled/`. One atomic slice; no half-translated intermediate state.
- **English ships; other languages do not.** The full mechanism is built so a translator can add a
  language folder with no code change, but this slice ships no machine-generated translation.
  Terse admin-tool strings translate badly by machine — gendered nouns, imperative mood and `{1}`
  slot order all fail in ways an English reviewer cannot catch. Real translations arrive as
  community PRs.
- **The language chain is proven by a shipped fixture**, not by a shipped product language: the
  cookbook's promised `de/trdemo.phrases.json` becomes real, so the packaged addon contains a
  language folder and the live gate reads a phrase through the real op off a real file.
- **Color is configured by tags inside phrase text**, not by config keys. One mechanism covers
  wording and color; no new config plumbing; a phrase can recolor mid-sentence.
- **Deferred:** a shared operator palette file (`chatcolors.json` mapping semantic roles); typed
  format specifiers; per-plugin default-language config; any non-English product translation.

## A. Color tags

### Syntax

`{name}` inside phrase text, where `name` is a key in the game package's color table —
`{green}`, `{default}`, `{lightred}`. Case-insensitive, matched as `/\{([a-z]+)\}/gi`.

Positional args are `{1}`, `{2}` — digits, never letters — so the two namespaces cannot collide.

### Expansion happens at output, in exactly two places

Because `stripCtl` already removes control bytes, expansion needs **no destination awareness**.
Expand `{green}` → `\x04` uniformly and the console path deletes it exactly as it deletes a
hand-written `ChatColors.Green` today.

| Funnel | Covers | Change |
| --- | --- | --- |
| `__s2_chatLine()` (`prelude.js:678`) | `Chat.toSlot`, `Chat.toAll`, `ctx.replyToChat` (slot ≥ 0) | expand tags → bytes |
| `__s2cmd_stripCtl()` (`prelude.js:900`) | `ctx.replyToConsole`, `ctx.replyToChat` (slot < 0) | expand **then** strip |

`stripCtl` must expand before stripping: it removes control bytes, and an unexpanded `{green}` is
not one. Making `stripCtl` expand-then-strip fixes all three console call sites from one edit.

**Ordering inside `__s2_chatLine` is load-bearing.** That function prepends `Chat.color`, then
inspects `body.charAt(0)` to decide whether the invisible U+200B lead-in is needed. Expansion must
run **before** that inspection: a line starting `{green}` must present as `\x04` when the check
runs, or the ZWSP logic reads a `{` and the leading color byte gets swallowed by the chat box —
the exact failure the ZWSP exists to prevent.

`Chat.color` is untouched — it remains the caller-owned opaque prefix it is today. A tag placed in
`Chat.color` expands like any other, since it is concatenated before expansion runs.

### The table crosses at runtime, never by import

The `@s2script/cs2` prelude registers its `{name} → control byte` map through the existing
`s2script_core_register_package` injection. Core performs a map lookup on a table it was handed; it
still never picks or knows a color, and `scripts/check-core-boundary.sh` sees no new import. This is
the same shape as every other engine fact in the codebase.

### Unknown tags

An unrecognized tag — a typo, or any tag at all when no game package registered a table — is
**deleted from the output**, and the first sighting of each distinct unknown name logs one
`[s2script] WARN:` to the server console (the pattern the malformed-phrases-file path already uses).

Players never see stray braces; the operator gets a diagnostic. This also means the mechanism has no
hard dependency on `@s2script/cs2`: a plugin running without a game package degrades to correct
uncolored text.

## B. Phrase files, keys, and the shared set

### `src/phrases.ts` per plugin

English lives in code as the seed, in its own module exporting a plain object:

```ts
// plugins/basechat/src/phrases.ts
export const phrases = {
  "Say All":     "{green}(ALL) {1}: {default}{2}",
  "Say Admins":  "{green}(ADMINS) {1}: {default}{2}",
  "Say Private": "{green}(private to {1}) {2}: {default}{3}",
};
```

A separate module so the generator (§C) can read it without parsing `plugin.ts`, and so `plugin.ts`
stays about behavior.

### Key naming

Human-readable English phrases as keys, SourceMod-style (`"Player not found"`, not
`"ERR_NO_PLAYER"`). The key is the ultimate fallback when every lookup misses, so a miss should read
as English rather than as a symbol.

### A shared `common` set

SourceMod ships `common.phrases.txt`; our plugins already duplicate `"No matching players"` and
`"Multiple players match"` verbatim across files. A `common` set holds the cross-plugin phrases:
target-resolution failures, `"You do not have access to this command."`, usage scaffolding.

**Its source of truth is one hand-authored drop-in file**, `translations/common.phrases.json`. It has
no `src/phrases.ts` seed — nothing generates it, and `scripts/gen-phrases.mjs` never touches it. Every
plugin calls a *seedless* `Translations.load("common")` (no second argument) after loading its own
set; that call reads only the root file — SourceMod's `LoadTranslations("common.phrases")` cadence,
exactly. `translations/` stays a plain drop-in folder, exactly like SourceMod's
`addons/sourcemod/translations/`: operators and third-party plugin authors add files to it directly,
and `gen-phrases.mjs` reserves the plugin-directory name `"common"` (`gen-phrases.mjs:157-169`) so a
future plugin can never generate a file that collides with — and silently overwrites — the
hand-authored one.

**Rejected: a shared library package.** An earlier iteration put the shared set at
`packages/phrases-common/`, declared under `s2script.libraries` and loaded via a second,
*seeded* `Translations.load("common", commonPhrases)`. That does not work: the SDK bundles a
plugin with esbuild, and every `@s2script/*` name is marked `external` in both `build.ts` and
`build-library.ts` — those names are framework builtins resolved by core at runtime, never inlined.
`tsc`'s path mapping *also* always resolves `@s2script/*` to a builtin `.d.ts`, so a library
declared under that scope would typecheck cleanly (its `paths` entry wins) while esbuild still
externalizes the name — the bundle would ship a bare `require("@s2script/phrases-common")` with none
of the library's code inlined, and it would die at load. `packages/sdk/src/libraries.ts`'s
`assertLibrariesResolved` refuses any `s2script.libraries` entry under the `@s2script/*` scope
outright, precisely to stop tsc and esbuild from silently disagreeing about what the name means —
which is what this design hit in practice before it was replaced with the drop-in file above.
`packages/` today ships only `cs2`, `eslint-plugin`, and `sdk`; no `phrases-common` package exists.

**Load order is load-bearing.** Within each of `translate`'s two passes — every loaded set in the
client's language, then every loaded set in English (see D1 below) — the first hit across sets wins,
so each plugin loads **its own set first, `common` second**. A plugin-specific phrase then shadows a
common phrase of the same key *at the same tier* — the precedence you want. A test pins this rule
(§F); it is only useful if something enforces it.

### Prefixes move into phrase text

`"[SM] Kicked {1}"` rather than `"[SM] " + t("Kicked", …)`. Rebranding the tag to `[MyServer]`
becomes an operator edit of one string instead of a fork.

## C. Generated English files + a freshness gate

The shipped English file is **generated from the seed**, never hand-written.

- `scripts/gen-phrases.mjs` imports each plugin's `src/phrases.ts` under
  `node --experimental-strip-types` — the same mechanism `ci-js.sh` already uses to run the SDK's
  test suite — and writes `translations/<name>.phrases.json`. Importing rather than text-parsing
  means the seed is validated as real TypeScript by the act of generating from it.
- **`<name>` is the plugin's directory name** (`basechat` → `translations/basechat.phrases.json`),
  and is the same string the plugin passes to `Translations.load`. A test asserts the three agree,
  because a mismatch fails silently: the file is simply never read and English seeds stand.
- `scripts/check-phrases-generated.sh` regenerates into a temp dir and diffs; drift fails the gate.
  It self-wires via the `check-*-generated.sh` glob in `ci-js.sh`.

This gives operators a real, discoverable file to edit — the root-override path means their edit
beats the seed — while making seed-vs-file drift structurally impossible. It is the repo's existing
codegen-freshness pattern applied to a new artifact.

`scripts/package-addon.sh` gains `translations/` to its runtime-dir creation and copies the
generated files in. Without this the whole feature is inert (see Background).

## D. Two correctness fixes this slice makes necessary

### D1. Set-ordering in `translate`

Current (`core/js/prelude.js:640-647`): for each set, check that set's language map, then check
**that same set's** English default, before moving to the next set.

With one loaded set this is invisible. With every plugin now loading `common` plus its own, a key
present in both means **set one's English beats set two's German**.

Fix: sweep all sets for the language, then sweep all sets for the default. Two sequential loops
instead of one nested pass. The per-set language file read is already lazily cached, so this adds no
I/O.

### D2. Arg substitution is a color-injection vector

`translate` substitutes `{1}` first; expansion runs later at output. A *substituted value*
containing `{red}` therefore gets expanded. The dominant argument across these plugins is a player
name.

A player renaming themselves to `{red}…{default}` would recolor admin chat and every message that
names them. This is cosmetic spoofing rather than privilege escalation, but it is exactly the shape
that arrives later as a "chat color exploit" report.

Fix: strip `{` and `}` from substituted argument values in `__s2_tr_format`. A player name loses a
brace; nobody recolors anyone else's chat.

## E. Testing and gates

**New gates**

- `check-phrases-generated.sh` — seed-vs-shipped-file drift (self-wiring, §C).
- A `node:test` suite over the pure pieces — `core/js/prelude.test.js`, the first test file that
  directory has ever had, following the `games/cs2/js/activity.test.js` pattern:
  - known / unknown / adjacent tags; empty table; no table at all
  - `{1}` positionals survive expansion untouched
  - brace-stripping in substituted args (D2)
  - `translate` set-ordering: language in a later set beats English in an earlier one (D1)
  - expansion precedes the ZWSP decision in `__s2_chatLine`: a line opening with `{green}` comes
    out ZWSP-led, not brace-led (§A)
- A test pinning the own-set-before-`common` load order and its override precedence (§B).
- A test asserting directory name, `Translations.load` name and generated filename agree for all
  18 plugins (§C) — the failure mode is silent.

**Existing gates that cover this for free**

- `check-plugins-typecheck.sh` — the TypeScript rewrite across all 18 plugins.
- `check-core-boundary.sh` — proves the color table introduced no core → games import.

**Live gate** (`make docker-test` + `scripts/rcon.py`)

- `sm_say` / `sm_psay` — colors render in chat.
- The same commands over rcon — console replies come back byte-clean, no stray braces, no bytes.
- One German phrase read from the shipped cookbook fixture, through the real op off a real file:
  the first coverage that path has ever had.

## F. Rollout

18 plugins, mechanical and independent per plugin. Sizes, by a conservative count (a string literal
passed directly as the first argument to a reply/chat/print call) and by total non-trivial string
literals — the true phrase count lands between the two and is not knowable until the rewrite:

| Plugin | Direct call sites | String literals |
| --- | --- | --- |
| basecommands | 30 | 95 |
| zones | 30 | 43 |
| basebans | 12 | 46 |
| playercommands | 12 | 34 |
| basechat | 6 | 13 |
| basevotes | 5 | 22 |
| disabled/funvotes | 5 | 20 |
| disabled/nextmap | 4 | 25 |
| adminhelp · adminmenu · basecomm · funcommands | 3 each | 8–15 each |
| disabled/nominations | 2 | 42 |
| antiflood · basetriggers · clientprefs · reservedslots · disabled/rockthevote | 0 | 4–66 |

The zero-call-site plugins are not stringless — they build messages by concatenation or via
`Chat.toAll` with a variable, which the conservative pattern does not match. `rockthevote` at 66
literals is the clearest example.

**Changesets.** If `packages/sdk`'s `chat.d.ts` / `translations.d.ts` or `packages/cs2` gain tag
documentation, `check-changeset.sh` requires a changeset. Patch level — no contract breaks. The 18
plugins are `private: true` and do not.

## G. Order of work

1. Color expansion in `core/js/prelude.js` (`__s2_chatLine`, `__s2cmd_stripCtl`) + the cs2 table
   registration.
2. D1 and D2 correctness fixes, with their tests.
3. `src/phrases.ts` + `gen-phrases.mjs` + `check-phrases-generated.sh` + `package-addon.sh`.
4. The `common` set.
5. The 18-plugin rewrite.
6. The cookbook `de/` fixture.
7. Live gate.

Steps 1–4 are the mechanism and carry all the risk; step 5 is volume. Landing them in this order
means the mechanical work is done against a mechanism already proven by its own tests.

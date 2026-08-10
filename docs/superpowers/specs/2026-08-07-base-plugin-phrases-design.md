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

## B. Phrase files

### One file per plugin, and it is the only copy

`translations/<plugin>.phrases.json` is the single source of truth: hand-authored, checked in,
shipped in the addon, edited by operators. There is no in-code copy of any phrase.

```json
// translations/basecomm.phrases.json
{
  "Usage Gag": "Usage: sm_gag <target>",
  "Gagged Player": "{green}[SM]{default} Gagged {1} player."
}
```

*This is a revision.* The slice first shipped English twice — a `src/phrases.ts` seed per plugin
plus a JSON generated from it — on the reasoning that a compiled-in default meant a plugin worked
with zero files. That bought a real property and cost 35 files, a generator, and a freshness gate
whose only job was keeping two copies of the same strings in step. SourceMod does this with one
file, and so do we now. The consequence is SourceMod's: a missing phrase file means keys render as
themselves, so `Translations.load` warns on a seedless load whose file is absent (§D3).

### Loading is explicit — nothing is automatic

A plugin declares the files it uses, in load order:

```ts
ctx.translations.load("basecomm", "common");
```

Nothing is loaded for a plugin automatically, including the shared `common` set. This is
SourceMod's rule ("Plugins must call LoadTranslations on each translation file they wish to use").

SourceMod's stated *reason* — stopping one plugin's file leaking into another — does not apply
here: each plugin runs in its own V8 context with its own phrase registry, so cross-plugin clashing
is impossible by construction (pinned by
`phrases_module_binds_to_the_calling_plugins_own_set`-style context isolation). Explicitness is
kept for the other reasons: one obvious way to do it, familiarity for SM authors, no reads for a
plugin that uses none, and a visible line saying which files a plugin depends on.

**Order is significant and is the plugin's to state.** `translate` takes the first hit within each
of its two passes (the client's language, then English), so a plugin lists its own set before any
shared one to be able to override a shared phrase. Stating it as one variadic call rather than N
statements keeps the order visible in one place.

### The shared set

`translations/common.phrases.json` is a hand-authored file with no generator involvement — the
cross-plugin phrases (target-resolution failures, "no access"). It is SourceMod's
`common.phrases.txt`: a data file any plugin loads by name, including third-party ones. Every base
plugin loads it explicitly.

`sync-phrase-types.mjs` reserves the plugin-directory name `"common"` so a plugin cannot be created
that would overwrite it.

### Key naming

Human-readable English phrases as keys (`"Player not found"`, not `"ERR_NO_PLAYER"`). The key is
what renders when every lookup misses, so a miss should read as English rather than a symbol.

### Prefixes live in phrase text

`"[SM] Kicked {1}"` rather than `"[SM] " + t("Kicked", …)`. Rebranding the tag to `[MyServer]`
becomes an operator edit of one string instead of a fork.

## C. Keys are checked at build time

`cmd.replyT(key)` and `Translations.translate(slot, key)` take a `PhraseKey`, not a `string`. The
valid keys are resolved per plugin from **the files that plugin actually loads**.

- `scripts/sync-phrase-types.mjs` parses each plugin's `ctx.translations.load(...)` call with the
  TypeScript compiler API — not a regex, which matches the same text inside a comment or a string,
  and a conversion of this shape leaves commented-out calls behind. Only literal arguments are
  collected; a name built at runtime is skipped rather than guessed at.
- It writes `plugins/<name>/src/phrases.generated.d.ts`, which augments the `PhraseKeys` interface
  in `packages/sdk/phrases.d.ts` with the union of those files' keys. Interface merging turns that
  into the exact literal union `PhraseKey` resolves to.
- That file carries key **names** only, never phrase text. It is derived and gitignored — the
  `.svelte-kit/types` model, not a second copy of the data.
- `PhraseKey` falls back to `string` when nothing has augmented it, so a plugin that loads no
  phrases — or whose declaration has not been written yet — compiles unchecked rather than failing
  to compile at all.

Two failures this catches that no runtime check can:

```ts
ctx.translations.load("basecomm", "common");
cmd.replyT("Usage Gagg");            // compile error — typo

ctx.translations.load("basecomm");   // common not loaded
cmd.replyT("No matching players");   // compile error — key not in any loaded file
```

SourceMod fails the second at runtime, printing the raw key to a player when the line is reached.
Here it fails the build.

`scripts/check-phrase-types.sh` gates declaration freshness. Because the declarations are gitignored,
`ci-js.sh` runs the sync **before** any plugin typecheck — otherwise keys would silently widen to
`string` and the check would pass vacuously.

`scripts/package-addon.sh` ships `translations/`. Without it the directory does not exist in an
install, every phrase resolves to its own key, and the whole mechanism is dead while appearing to
work — the original defect this slice was written to fix.


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

### D3. A seedless load whose file is missing renders raw keys

With no in-code copy, a phrase set whose file is absent is empty, and every key resolved against it
falls through to the key itself — visible to players, with nothing in the log. `Translations.load`
therefore warns, naming the set, when a load supplies no seed **and** finds no file. Only that
combination: a load with a seed and no file is the normal degrade and stays silent.

This is the same failure mode as SourceMod's, and it is the price of one file per plugin. The
mitigations are that `package-addon.sh` ships the directory and the warning names the set.

## E. Testing and gates

**New gates**

- `check-phrase-types.sh` — the generated key declarations match the files each plugin loads (§C).
- The declarations are gitignored, so `ci-js.sh` runs `sync-phrase-types.mjs` BEFORE any plugin
  typecheck — otherwise keys widen to `string` and the typecheck passes vacuously.
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
3. `translations/<plugin>.phrases.json` + `sync-phrase-types.mjs` + `check-phrase-types.sh` +
   `package-addon.sh`.
4. The `common` set.
5. The 18-plugin rewrite.
6. The cookbook `de/` fixture.
7. Live gate.

Steps 1–4 are the mechanism and carry all the risk; step 5 is volume. Landing them in this order
means the mechanical work is done against a mechanism already proven by its own tests.

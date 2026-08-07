# Base-plugin i18n retrofit + operator-configurable chat colors — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every user-facing string in all 18 shipped plugins becomes a translatable phrase, and every color in those strings becomes an operator file edit rather than a plugin fork.

**Architecture:** A pure color-tag expander (`core/js/colors.js`) turns `{green}` into a control byte at the two output funnels that already exist in `core/js/prelude.js` — `__s2_chatLine` for chat, `__s2cmd_stripCtl` for console. The `{name} → byte` table is handed to core at runtime by the `@s2script/cs2` prelude, so core never learns a color name. English phrases live in code as `src/phrases.ts` per plugin; the shipped `translations/*.phrases.json` files are generated from those seeds and gated for freshness.

**Tech Stack:** JavaScript (`core/js/`, `games/cs2/js/`), TypeScript (plugins, `packages/sdk` codegen), Node 22 `node:test`, Rust (`cargo test -p s2script-core` for in-V8-context tests), bash gate scripts.

**Design:** `docs/superpowers/specs/2026-08-07-base-plugin-phrases-design.md`

## Global Constraints

- **Worktree:** `/home/gkh/projects/s2script/.claude/worktrees/base-plugin-phrases`, branch `i18n/base-plugin-phrases`, based on `origin/main`. All paths below are relative to that worktree root.
- **Core is engine-generic.** `core/js/colors.js` and `core/js/prelude.js` must never name a color or a game. The `{name} → byte` table arrives at runtime from the game package. `scripts/check-core-boundary.sh` must stay green.
- **Degrade, never crash.** An unknown tag, an absent table, a malformed phrases file: each degrades to correct uncolored text. Nothing throws.
- **One PR, atomic.** `make ci` must pass at the end. Individual task commits need not each be green, but the branch must be green before the PR.
- **Gates live in `scripts/ci-js.sh`, never in workflow YAML.** `.github/workflows/ci-js.yml` runs that script and nothing else.
- **`scripts/check-*-generated.sh` is globbed** by `ci-js.sh:20-25` — a new freshness gate matching that name self-wires. A gate NOT matching that glob needs an explicit line added to `ci-js.sh`.
- **Node tests run with `node --test`.** Core tests run serial (`RUST_TEST_THREADS=1`, already forced by `.cargo/config.toml` — do not pass `--test-threads`).
- **Changesets:** a change under `packages/*` that is not `private: true` requires a changeset or `scripts/check-changeset.sh` fails. `packages/phrases-common` is `private: true` and does not. `packages/sdk` and `packages/cs2` do.
- **Commit style:** imperative subject, no `Co-Authored-By`, no tool attribution.

---

## File Structure

**Create**
- `core/js/colors.js` — the pure expander. `setTable` / `expand` / `chatLine` / `consoleLine`. Dual-mode export (CommonJS for tests, `globalThis` for the prelude), mirroring `games/cs2/js/activity.js`.
- `core/js/colors.test.js` — `node:test` suite for the above. First test file in `core/js/`.
- `scripts/check-colors-test.sh` — runs it. Needs an explicit `ci-js.sh` line (does not match the `check-*-generated.sh` glob).
- `scripts/gen-phrases.mjs` — reads each plugin's `src/phrases.ts`, writes `translations/<dir>.phrases.json`.
- `scripts/check-phrases-generated.sh` — freshness gate. Self-wires via the glob.
- `packages/phrases-common/{package.json,index.ts}` — the shared `common` phrase set, `s2script.kind: "library"`, `private: true`.
- `plugins/<name>/src/phrases.ts` × 18 — each plugin's English seed.
- `translations/<name>.phrases.json` × 19 (18 plugins + `common`) — generated, committed.
- `translations/de/trdemo.phrases.json` — the cookbook fixture; first real language file in the repo.

**Modify**
- `core/js/prelude.js` — `__s2_chatLine` (:678) and `__s2cmd_stripCtl` (:900) become thin wrappers over `colors.js`; `__s2_tr_format` (:608) strips braces from args; the `translate` set loop (:640-647) sweeps language-then-default.
- `core/src/v8host.rs:1240` — `include_str!` becomes `concat!` of `colors.js` + `prelude.js`.
- `games/cs2/js/pawn.js` — registers `ChatColors` (:610) as the color table.
- `scripts/package-addon.sh:59` — create and populate `translations/`.
- `scripts/ci-js.sh` — one line for `check-colors-test.sh`.
- `plugins/<name>/src/plugin.ts` × 18 — call sites become `Translations.translate` / `ctx.replyT`.
- `plugins/<name>/package.json` × 18 — declare `s2script.libraries`.
- `examples/cookbook/src/recipes/translations.ts` — comment corrected once the fixture is real.

---

## Task 1: The color expander

**Files:**
- Create: `core/js/colors.js`
- Create: `core/js/colors.test.js`
- Create: `scripts/check-colors-test.sh`
- Modify: `scripts/ci-js.sh`

**Interfaces:**
- Produces: `globalThis.__s2_colors` (also `module.exports`) = `{ setTable(obj), expand(text), chatLine(prefix, msg), consoleLine(text), _resetWarnings() }`.
  - `setTable(obj)`: stores a lowercased-key copy of `obj`. Non-object → clears the table.
  - `expand(text)`: `{name}` → the table's byte; unknown/absent → `""` plus one console warning per distinct name.
  - `chatLine(prefix, msg)`: `expand(prefix + msg)`, then prepend U+200B unless the result already leads with U+200B or a space.
  - `consoleLine(text)`: `expand(text)`, then remove `[\x00-\x1F\x7F]`.
  - `_resetWarnings()`: test-only; clears the warned-name set.

- [ ] **Step 1: Write the failing test**

Create `core/js/colors.test.js`:

```js
const test = require("node:test");
const assert = require("node:assert");
const colors = require("./colors.js");

const ZWSP = "\u200B";
const TABLE = { Default: "\x01", White: "\x01", Green: "\x04", LightRed: "\x0F" };

test.beforeEach(() => { colors.setTable(TABLE); colors._resetWarnings(); });

test("expand: known tag becomes its byte, case-insensitively", () => {
  assert.strictEqual(colors.expand("{green}hi"), "\x04hi");
  assert.strictEqual(colors.expand("{GREEN}hi"), "\x04hi");
  assert.strictEqual(colors.expand("{lightred}hi"), "\x0Fhi");
});

test("expand: adjacent tags both resolve", () => {
  assert.strictEqual(colors.expand("{green}{white}hi"), "\x04\x01hi");
});

test("expand: unknown tag is deleted, not left literal", () => {
  assert.strictEqual(colors.expand("{gren}hi"), "hi");
});

test("expand: with no table at all, tags are deleted and text survives", () => {
  colors.setTable(null);
  assert.strictEqual(colors.expand("{green}hi"), "hi");
});

test("expand: positional {1} slots are never touched", () => {
  assert.strictEqual(colors.expand("{green}{1} joined"), "\x04{1} joined");
});

test("chatLine: expansion precedes the ZWSP decision", () => {
  // The whole point: after expansion the line leads with \x04, not "{", so the
  // ZWSP is prepended and the colour byte is not swallowed by the chat box.
  assert.strictEqual(colors.chatLine("", "{green}hi"), ZWSP + "\x04hi");
});

test("chatLine: an already-led line is not double-prefixed", () => {
  assert.strictEqual(colors.chatLine("", ZWSP + "hi"), ZWSP + "hi");
  assert.strictEqual(colors.chatLine("", " hi"), " hi");
});

test("chatLine: the caller-owned prefix is expanded too", () => {
  assert.strictEqual(colors.chatLine("{green}", "hi"), ZWSP + "\x04hi");
});

test("consoleLine: tags and raw control bytes both vanish", () => {
  assert.strictEqual(colors.consoleLine("{green}hi"), "hi");
  assert.strictEqual(colors.consoleLine("\x04hi"), "hi");
  assert.strictEqual(colors.consoleLine("{green}a\x01b"), "ab");
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node --test core/js/colors.test.js`
Expected: FAIL — `Cannot find module './colors.js'`

- [ ] **Step 3: Write the implementation**

Create `core/js/colors.js`:

```js
// Color-tag expansion. ENGINE-GENERIC: this file never names a colour or a game — the
// {name} -> control-byte table is handed in at runtime by the game package's prelude
// (games/cs2/js/pawn.js calls setTable(ChatColors)). Core only ever does a map lookup.
//
// Concatenated AHEAD of prelude.js by the include_str!/concat! in core/src/v8host.rs, so
// globalThis.__s2_colors exists before prelude.js runs. Dual-mode export (see activity.js)
// so the pure logic is node:test-able without a V8 context.
(function () {
  var table = Object.create(null);
  var warned = Object.create(null);

  function setTable(obj) {
    table = Object.create(null);
    if (!obj || typeof obj !== "object") return;          // null/garbage -> empty table, tags just vanish
    for (var k in obj) {
      if (!Object.prototype.hasOwnProperty.call(obj, k) || k === "__proto__") continue;
      if (typeof obj[k] === "string") table[String(k).toLowerCase()] = obj[k];
    }
  }

  // {name} where name is LETTERS ONLY, so positional {1}/{2} slots can never collide with a
  // colour tag. An unrecognised tag is DELETED (players must never see stray braces) and warned
  // ONCE per distinct name, so a typo is diagnosable from the server console without spamming it.
  function expand(text) {
    return String(text).replace(/\{([A-Za-z]+)\}/g, function (_m, name) {
      var key = name.toLowerCase();
      var v = table[key];
      if (typeof v === "string") return v;
      if (!warned[key]) {
        warned[key] = true;
        console.log("[s2script] WARN: unknown colour tag {" + name + "} — removed from output");
      }
      return "";
    });
  }

  var ZWSP = "\u200B";
  // Expand BEFORE the lead-byte test: a line written "{green}hi" must present as "\x04hi" when we
  // decide, or we would read "{" , skip the ZWSP, and the chat box would swallow the colour byte.
  function chatLine(prefix, msg) {
    var body = expand(String(prefix) + String(msg));
    var lead = body.charAt(0);
    return (lead === ZWSP || lead === " ") ? body : ZWSP + body;
  }

  // Console output: expand first (an unexpanded "{green}" is not a control byte and would survive
  // the strip as literal text), then drop every control byte — colour bytes included.
  function consoleLine(text) {
    return expand(text).replace(/[\x00-\x1F\x7F]/g, "");
  }

  var api = {
    setTable: setTable, expand: expand, chatLine: chatLine, consoleLine: consoleLine,
    _resetWarnings: function () { warned = Object.create(null); },
  };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  if (typeof globalThis !== "undefined") globalThis.__s2_colors = api;
})();
```

- [ ] **Step 4: Run test to verify it passes**

Run: `node --test core/js/colors.test.js`
Expected: PASS, 10 tests

- [ ] **Step 5: Add the gate script**

Create `scripts/check-colors-test.sh`:

```bash
#!/usr/bin/env bash
# Run the node:test suite for core/js/colors.js (the pure colour-tag expander).
set -euo pipefail
cd "$(dirname "$0")/.."
node --test core/js/colors.test.js
echo "PASS: colors.test.js"
```

Then `chmod +x scripts/check-colors-test.sh`.

- [ ] **Step 6: Wire it into the JS suite**

In `scripts/ci-js.sh`, immediately after the `check-activity-test.sh` block, add:

```bash
echo "== check-colors-test.sh (colour-tag expander) =="
bash scripts/check-colors-test.sh
```

- [ ] **Step 7: Verify the gate runs**

Run: `bash scripts/check-colors-test.sh`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add core/js/colors.js core/js/colors.test.js scripts/check-colors-test.sh scripts/ci-js.sh
git commit -m "core: a pure colour-tag expander, table supplied at runtime

Engine-generic: the {name} -> control-byte map is handed in by the game
package, so core does a lookup and never names a colour. Unknown tags are
deleted rather than shown to players, and warned once each to the console."
```

---

## Task 2: Wire the expander into the two output funnels

**Files:**
- Modify: `core/js/prelude.js:678` (`__s2_chatLine`), `core/js/prelude.js:900` (`__s2cmd_stripCtl`)
- Modify: `core/src/v8host.rs:1240`

**Interfaces:**
- Consumes: `globalThis.__s2_colors.chatLine` / `.consoleLine` from Task 1.
- Produces: no new names. `Chat.toSlot`, `Chat.toAll`, `ctx.reply*` behave identically for untagged text and now expand tags.

- [ ] **Step 1: Make `colors.js` part of the injected prelude**

In `core/src/v8host.rs`, replace line 1240:

```rust
const INJECTED_STD_PRELUDE: &str = include_str!("../js/prelude.js");
```

with:

```rust
// colors.js FIRST: it sets globalThis.__s2_colors, which prelude.js's chat and console
// funnels call. Same ordering contract as games/cs2/js (activity.js before pawn.js).
const INJECTED_STD_PRELUDE: &str =
    concat!(include_str!("../js/colors.js"), "\n", include_str!("../js/prelude.js"));
```

- [ ] **Step 2: Write the failing in-context test**

In `core/src/v8host.rs`, in the test module beside the existing translations tests (~line 13694), add:

```rust
/// Colour tags expand on the chat path and are deleted on the console path. The table is
/// supplied the way a game package supplies it — at runtime, from inside the context.
#[test]
fn colour_tags_expand_on_chat_and_vanish_on_console() {
    let _g = test_host();
    eval_in_context("p", "__s2_colors.setTable({ Green: '\\x04', White: '\\x01' });").unwrap();
    // chat: tag -> byte, and the ZWSP still leads
    assert_eq!(
        eval_in_context_string("p", "JSON.stringify(__s2_colors.chatLine('', '{green}hi'))"),
        "\"\\u200b\\u0004hi\""
    );
    // console: tag deleted entirely
    assert_eq!(eval_in_context_string("p", "__s2_colors.consoleLine('{green}hi')"), "hi");
    // unknown tag: deleted, never literal
    assert_eq!(eval_in_context_string("p", "__s2_colors.consoleLine('{nope}hi')"), "hi");
}
```

> If `test_host()` is not the harness name used by the neighbouring translations test at
> `core/src/v8host.rs:13694`, copy that test's setup lines verbatim instead — the point is to
> reuse the existing in-context harness, not to introduce a new one.

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p s2script-core colour_tags_expand`
Expected: FAIL — `__s2_colors is not defined` (the concat is not in place yet, or the wrappers are not wired)

- [ ] **Step 4: Rewrite the two funnels**

In `core/js/prelude.js`, replace `__s2_chatLine` (line 678-682) with:

```js
  function __s2_chatLine(msg) { return globalThis.__s2_colors.chatLine(__s2_chat.color, msg); }
```

and replace `__s2cmd_stripCtl` (line 900) with:

```js
  // Expand colour tags, then drop every control byte. Expansion must come first: an unexpanded
  // "{green}" is not a control byte and would survive the strip as literal text on an rcon reply.
  function __s2cmd_stripCtl(s) { return globalThis.__s2_colors.consoleLine(s); }
```

Leave the surrounding comment block at lines 663-676 in place, and append to it:

```js
  // Colour TAGS ({green}, {default}) are expanded here too — see core/js/colors.js. The table is
  // supplied by the game package at runtime; core never knows a colour name.
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p s2script-core colour_tags_expand`
Expected: PASS

- [ ] **Step 6: Run the full core suite for regressions**

Run: `cargo test -p s2script-core`
Expected: PASS. The existing chat tests (`__s2pkg_chat.Chat.toSlot` / `toAll`) and every `eval_in_context` test must still pass — untagged text is unchanged by expansion.

- [ ] **Step 7: Commit**

```bash
git add core/js/prelude.js core/src/v8host.rs
git commit -m "core: expand colour tags at the chat and console funnels

__s2_chatLine and __s2cmd_stripCtl become thin wrappers over colors.js, so
every existing Chat.toSlot / Chat.toAll / ctx.reply* call site inherits tag
expansion with no per-site change. Console replies stay byte-clean because
expansion runs before the existing control-byte strip."
```

---

## Task 3: The two `translate` correctness fixes

**Files:**
- Modify: `core/js/prelude.js:608` (`__s2_tr_format`), `core/js/prelude.js:640-647` (the `translate` set loop)
- Modify: `core/src/v8host.rs` (test module, beside the translations tests at ~13694)

**Interfaces:**
- Produces: no signature change. `Translations.translate` keeps `(slot, key, ...args)`.

- [ ] **Step 1: Write the two failing tests**

In `core/src/v8host.rs`, beside the existing translations tests:

```rust
/// D1: a translation in a LATER-loaded set must beat an English default in an earlier one.
/// Unreachable with a single set, which is why it survived; reachable the moment a plugin
/// loads its own set plus the shared `common` set.
#[test]
fn translate_prefers_any_language_hit_over_any_english_default() {
    let _g = test_host();
    eval_in_context("p", "\
        __s2pkg_translations.Translations.load('own',    { Greet: 'EN own' });\
        __s2pkg_translations.Translations.load('common', { Greet: 'EN common' });\
        __s2_tr_injectLang('common', 'de', { Greet: 'DE common' });\
        __s2pkg_translations.Translations.setDefaultLanguage('de');\
    ").unwrap();
    assert_eq!(
        eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Greet')"),
        "DE common"
    );
}

/// D2: a substituted argument must not be able to inject a colour tag. A player who renames
/// themselves "{red}x{default}" would otherwise recolour every message that names them.
#[test]
fn translate_strips_braces_from_substituted_args() {
    let _g = test_host();
    eval_in_context("p",
        "__s2pkg_translations.Translations.load('t', { Slain: '{1} was slain' });").unwrap();
    assert_eq!(
        eval_in_context_string("p",
            "__s2pkg_translations.Translations.translate(-1,'Slain','{red}evil{default}')"),
        "redevildefault was slain"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p s2script-core translate_prefers_any_language translate_strips_braces`
Expected: both FAIL — the first returns `"EN own"`, the second returns `"{red}evil{default} was slain"`

- [ ] **Step 3: Fix D2 — strip braces from substituted args**

In `core/js/prelude.js`, replace `__s2_tr_format` (lines 608-613) with:

```js
  function __s2_tr_format(text, args) {
    return String(text).replace(/\{(\d+)\}/g, function (_m, n) {
      var i = (parseInt(n, 10) | 0) - 1;
      if (!args || i < 0 || i >= args.length || args[i] == null) return "";
      // Braces are stripped from SUBSTITUTED values so an argument cannot inject a colour tag
      // (colors.js expands the finished string at output). A player name loses a brace; nobody
      // recolours anyone else's chat.
      return String(args[i]).replace(/[{}]/g, "");
    });
  }
```

- [ ] **Step 4: Fix D1 — sweep language across all sets, then defaults**

In `core/js/prelude.js`, replace the `translate` body's loop (lines 640-646) with:

```js
      // Sweep EVERY loaded set for the client's language FIRST, and only then sweep every set for
      // an English default. The old single pass checked one set's language map and then that same
      // set's default before moving on, so an earlier set's English beat a later set's translation.
      if (code) {
        for (var ln in __s2_tr_reg) {
          if (!Object.prototype.hasOwnProperty.call(__s2_tr_reg, ln)) continue;
          var lm = __s2_tr_langMap(ln, code);
          if (lm && lm[key] != null) return __s2_tr_format(lm[key], args);
        }
      }
      for (var dn in __s2_tr_reg) {
        if (!Object.prototype.hasOwnProperty.call(__s2_tr_reg, dn)) continue;
        var d = __s2_tr_reg[dn].def;
        if (d[key] != null) return __s2_tr_format(d[key], args);
      }
      return key;                                            // ultimate fallback
```

- [ ] **Step 5: Run both tests to verify they pass**

Run: `cargo test -p s2script-core translate_prefers_any_language translate_strips_braces`
Expected: PASS

- [ ] **Step 6: Run the full core suite**

Run: `cargo test -p s2script-core`
Expected: PASS. The existing translations test at ~13694 (single set, `de` injected, seed fallback) must still pass — with one set the two orderings are equivalent.

- [ ] **Step 7: Commit**

```bash
git add core/js/prelude.js core/src/v8host.rs
git commit -m "core: fix translate set-ordering and arg colour injection

translate checked one set's language map then that same set's English default
before moving on, so with two loaded sets an earlier set's English beat a later
set's translation. Sweep language across all sets first, then defaults.

Separately, strip braces from substituted args: expansion runs on the finished
string, so an argument containing {red} could otherwise recolour any message
that interpolated it — a player name is the usual argument."
```

---

## Task 4: The CS2 colour table

**Files:**
- Modify: `games/cs2/js/pawn.js` (beside the `ChatColors` definition at :610)

**Interfaces:**
- Consumes: `globalThis.__s2_colors.setTable` from Task 1.
- Produces: `{default}`, `{white}`, `{darkred}`, `{lightpurple}`, `{green}`, `{olive}`, `{lime}`, `{red}`, `{grey}`, `{yellow}`, `{silver}`, `{blue}`, `{darkblue}`, `{bluegrey}`, `{purple}`, `{lightred}`, `{orange}` usable in any phrase.

- [ ] **Step 1: Register the table**

In `games/cs2/js/pawn.js`, immediately after the `ChatColors` `Object.freeze({...})` block (ends line 614), add:

```js
  // Hand the tag table to core's expander (core/js/colors.js). This is the ONLY direction game
  // colour knowledge may travel: core receives a map, it never holds one. setTable lowercases the
  // keys, so `Green` above is reachable as `{green}` in any phrase file an operator edits.
  // Guarded for an older core that predates the expander.
  if (globalThis.__s2_colors && typeof globalThis.__s2_colors.setTable === "function") {
    globalThis.__s2_colors.setTable(ChatColors);
  }
```

- [ ] **Step 2: Verify the boundary gate still passes**

Run: `bash scripts/check-core-boundary.sh`
Expected: PASS — the table crosses at runtime, so core gained no import of `games/*`.

- [ ] **Step 3: Verify the cs2 engine-call suite still passes**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/cs2-engine-calls.test.mjs`
Expected: PASS — that suite loads the real shipped bundle, so it proves `pawn.js` still parses and initialises.

- [ ] **Step 4: Commit**

```bash
git add games/cs2/js/pawn.js
git commit -m "cs2: register ChatColors as the colour-tag table

The map crosses to core at runtime through the existing package injection, so
{green} works in any phrase file while core still never names a colour."
```

---

## Task 5: Phrase generation, freshness gate, and packaging

**Files:**
- Create: `scripts/gen-phrases.mjs`, `scripts/check-phrases-generated.sh`
- Modify: `scripts/package-addon.sh:59`

**Interfaces:**
- Consumes: each plugin's `src/phrases.ts` default export (Task 6 onward creates them).
- Produces: `translations/<dir>.phrases.json` for every `plugins/*/src/phrases.ts` and `plugins/disabled/*/src/phrases.ts`, plus `translations/common.phrases.json` from `packages/phrases-common/index.ts`.
- Contract: the JSON basename equals the plugin's **directory name**, which is also the string the plugin must pass to `Translations.load`.

- [ ] **Step 1: Write the generator**

Create `scripts/gen-phrases.mjs`:

```js
#!/usr/bin/env node
// Generate translations/<name>.phrases.json from each plugin's in-code English seed.
//
// The seed is the single source of truth and lives in TypeScript; the shipped JSON is what an
// OPERATOR edits (the root file overrides the seed at load — see core/js/prelude.js's
// Translations.load). Generating rather than hand-writing makes drift between the two impossible.
//
// Run:  node scripts/gen-phrases.mjs           # write
//       node scripts/gen-phrases.mjs --check   # exit 1 on drift, write nothing
import { readdirSync, existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "translations");
const check = process.argv.includes("--check");

function pluginDirs() {
  const out = [];
  for (const base of ["plugins", join("plugins", "disabled")]) {
    const abs = join(ROOT, base);
    if (!existsSync(abs)) continue;
    for (const name of readdirSync(abs, { withFileTypes: true })) {
      if (!name.isDirectory() || name.name === "disabled") continue;
      const seed = join(abs, name.name, "src", "phrases.ts");
      if (existsSync(seed)) out.push({ name: name.name, seed });
    }
  }
  return out.sort((a, b) => a.name.localeCompare(b.name));
}

async function loadSeed(file) {
  // Imported, not text-parsed: generating therefore validates the seed as real TypeScript.
  // Requires node >= 22 (--experimental-strip-types), which package.json already pins.
  const mod = await import(pathToFileURL(file).href);
  const seed = mod.phrases ?? mod.default;
  if (!seed || typeof seed !== "object") {
    throw new Error(`${file}: expected an exported \`phrases\` object`);
  }
  return seed;
}

function render(seed) {
  // Keys sorted so the generated file has a stable diff regardless of source order.
  const sorted = {};
  for (const k of Object.keys(seed).sort()) sorted[k] = seed[k];
  return JSON.stringify(sorted, null, 2) + "\n";
}

const targets = [
  { name: "common", seed: join(ROOT, "packages", "phrases-common", "index.ts") },
  ...pluginDirs(),
];

let drift = 0;
mkdirSync(OUT, { recursive: true });
for (const t of targets) {
  if (!existsSync(t.seed)) continue;
  const text = render(await loadSeed(t.seed));
  const dest = join(OUT, `${t.name}.phrases.json`);
  const current = existsSync(dest) ? readFileSync(dest, "utf8") : null;
  if (current === text) continue;
  if (check) {
    console.error(`DRIFT: ${dest} does not match ${t.seed}`);
    drift++;
  } else {
    writeFileSync(dest, text);
    console.log(`wrote ${dest}`);
  }
}

if (check && drift > 0) {
  console.error(`\n${drift} phrases file(s) out of date — run: node scripts/gen-phrases.mjs`);
  process.exit(1);
}
if (check) console.log("PASS: phrases files are up to date");
```

- [ ] **Step 2: Write the freshness gate**

Create `scripts/check-phrases-generated.sh`:

```bash
#!/usr/bin/env bash
# Fail if a committed translations/*.phrases.json is out of date vs its in-code seed.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
node --experimental-strip-types --no-warnings scripts/gen-phrases.mjs --check
```

Then `chmod +x scripts/check-phrases-generated.sh`.

The name matches `check-*-generated.sh`, so `ci-js.sh:20-25` picks it up with no edit.

- [ ] **Step 3: Verify it passes vacuously (no seeds exist yet)**

Run: `bash scripts/check-phrases-generated.sh`
Expected: `PASS: phrases files are up to date` — no `src/phrases.ts` exists yet, so there is nothing to compare.

- [ ] **Step 4: Ship `translations/` in the packaged addon**

In `scripts/package-addon.sh`, change line 59 from:

```bash
mkdir -p "$DIST/s2script/plugins" "$DIST/s2script/configs" "$DIST/s2script/data"
```

to:

```bash
mkdir -p "$DIST/s2script/plugins" "$DIST/s2script/configs" "$DIST/s2script/data"
# translations/: the operator-editable phrase files. Without this the directory never exists in an
# install, translations_read always returns null, and every phrase silently falls back to its
# in-code seed — the whole file mechanism dead while appearing to work.
mkdir -p "$DIST/s2script/translations"
if [ -d translations ]; then
    cp -r translations/. "$DIST/s2script/translations/"
fi
```

- [ ] **Step 5: Verify packaging**

Run: `bash scripts/package-addon.sh && ls -R dist/addons/s2script/translations`
Expected: the directory exists. It is empty until Task 6 generates files, which is correct at this point.

- [ ] **Step 6: Commit**

```bash
git add scripts/gen-phrases.mjs scripts/check-phrases-generated.sh scripts/package-addon.sh
git commit -m "build: generate phrases files from in-code seeds, and ship them

package-addon.sh never created translations/, so in a packaged install
translations_read always returned null and every phrase fell back to its seed —
the file mechanism was dead while appearing to work.

The English JSON is now generated from each plugin's src/phrases.ts and gated
for freshness, so the file an operator edits can never drift from the seed."
```

---

## Task 6: The `common` library and the basechat pilot

This task proves the whole chain on one plugin before 17 more follow. It is the first consumer of
the `s2script.kind: "library"` mechanism inside the base-plugin tree.

**Files:**
- Create: `packages/phrases-common/package.json`, `packages/phrases-common/index.ts`
- Create: `plugins/basechat/src/phrases.ts`
- Modify: `plugins/basechat/src/plugin.ts`, `plugins/basechat/package.json`
- Generated: `translations/common.phrases.json`, `translations/basechat.phrases.json`

**Interfaces:**
- Produces: `import { phrases as commonPhrases } from "@s2script/phrases-common"` — a `Record<string, string>` of cross-plugin phrases.
- Contract (applies to every plugin from here on): load **own set first, `common` second**, so a plugin-specific key shadows a common one.

- [ ] **Step 1: Create the shared library package**

Create `packages/phrases-common/package.json`:

```json
{
  "name": "@s2script/phrases-common",
  "version": "0.1.0",
  "private": true,
  "main": "index.ts",
  "types": "index.ts",
  "s2script": {
    "kind": "library"
  }
}
```

Create `packages/phrases-common/index.ts`:

```ts
/**
 * Cross-plugin phrases — the `common` set (SourceMod's common.phrases.txt).
 *
 * Every base plugin loads this SECOND, after its own set, so a plugin-specific key of the same
 * name shadows the shared one. Colour tags ({green}, {default}, …) are expanded at output by
 * core/js/colors.js; an operator recolours by editing translations/common.phrases.json.
 */
export const phrases = {
  "No matching players": "{green}[SM]{default} No matching players",
  "More than one client matched": "{green}[SM]{default} More than one client matched \"{1}\"",
  "Player no longer available": "{green}[SM]{default} That player is no longer available",
  "No access": "{green}[SM]{default} You do not have access to this command",
  "Server console only": "{green}[SM]{default} This command can only be run from the server console",
  "Unknown command": "{green}[SM]{default} Unknown command",
};
```

- [ ] **Step 2: Install the new workspace member**

Run: `npm install --no-fund --no-audit`
Expected: `packages/phrases-common` linked into `node_modules/@s2script/phrases-common`.

> If this rewrites unrelated parts of `package-lock.json`, keep only the lines that add the new
> workspace member: `git add -p package-lock.json`.

- [ ] **Step 3: Write basechat's seed**

Create `plugins/basechat/src/phrases.ts`:

```ts
/** basechat — English seed. Generated into translations/basechat.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  "Say All": "{green}(ALL) {1}: {default}{2}",
  "Say Admins": "{green}(ADMINS) {1}: {default}{2}",
  "Say Private To": "{green}(private to {1}) {2}: {default}{3}",
  "Say Private Echo": "{green}(private to {1}) {default}{2}",
  "Usage Say": "Usage: sm_say <message>",
  "Usage Chat": "Usage: sm_chat <message>",
  "Usage Psay": "Usage: sm_psay <target> <message>",
  "Usage Psay Trigger": "Usage: @@<target> <message>",
};
```

- [ ] **Step 4: Declare the library dependency**

Replace `plugins/basechat/package.json` with:

```json
{
  "name": "@s2script/basechat",
  "version": "0.1.0",
  "private": true,
  "main": "src/plugin.ts",
  "s2script": {
    "libraries": ["@s2script/phrases-common"]
  }
}
```

- [ ] **Step 5: Rewrite basechat's call sites**

In `plugins/basechat/src/plugin.ts`:

Add to the imports:

```ts
import { Translations } from "@s2script/sdk/translations";
import { phrases as commonPhrases } from "@s2script/phrases-common";
import { phrases } from "./phrases";
```

Remove the now-unused `const GREEN = ChatColors.Green, WHITE = ChatColors.White;` line and drop
`ChatColors` from the `@s2script/cs2` import (leave `Player`, `Activity`).

At the very top of the `plugin((ctx) => {` body, before any command registration:

```ts
  // Own set FIRST, common SECOND: translate takes the first hit across sets, so this order is what
  // lets a plugin override a shared phrase.
  Translations.load("basechat", phrases);
  Translations.load("common", commonPhrases);
```

Replace the three message helpers:

```ts
function doSay(actorSlot: number, msg: string): void {
  for (const p of Player.allConnected()) {
    const src = Activity.formatSource(actorSlot, p.slot);
    if (src.show) Chat.toSlot(p.slot, Translations.translate(p.slot, "Say All", src.name, msg));
  }
}

function doAdminChat(actorSlot: number, msg: string): void {
  const name = actorName(actorSlot);
  for (const p of Player.allConnected()) {
    const a = Admin.forSlot(p.slot);
    if (a && a.hasFlags(ADMFLAG.CHAT)) {
      Chat.toSlot(p.slot, Translations.translate(p.slot, "Say Admins", name, msg));
    }
  }
}

function doPsay(actorSlot: number, target: Player, msg: string): void {
  const name = actorName(actorSlot);
  const tn = target.playerName || "";
  Chat.toSlot(target.slot, Translations.translate(target.slot, "Say Private To", tn, name, msg));
  if (actorSlot >= 0 && actorSlot !== target.slot) {
    Chat.toSlot(actorSlot, Translations.translate(actorSlot, "Say Private Echo", tn, msg));
  }
}
```

Replace `resolveOne`'s two literals:

```ts
function resolveOne(pattern: string, callerSlot: number, reply: (m: string) => void): Player | null {
  const matches = Player.target(pattern, callerSlot);
  if (matches.length === 0) { reply(Translations.translate(callerSlot, "No matching players")); return null; }
  if (matches.length > 1) {
    reply(Translations.translate(callerSlot, "More than one client matched", pattern));
    return null;
  }
  return matches[0];
}
```

Replace each `cmd.reply("Usage: …")` with `cmd.replyT("Usage Say")` / `"Usage Chat"` /
`"Usage Psay"` as appropriate, and the `Chat.toSlot(slot, "Usage: @@<target> <message>")` in the
`onSay` handler with `Chat.toSlot(slot, Translations.translate(slot, "Usage Psay Trigger"))`.

- [ ] **Step 6: Generate the phrases files**

Run: `node --experimental-strip-types --no-warnings scripts/gen-phrases.mjs`
Expected: writes `translations/common.phrases.json` and `translations/basechat.phrases.json`

- [ ] **Step 7: Verify the freshness gate and the typecheck**

Run: `bash scripts/check-phrases-generated.sh && bash scripts/check-plugins-typecheck.sh`
Expected: both PASS

- [ ] **Step 8: Verify basechat builds with its library bundled**

Run: `bash scripts/build-base-plugins.sh`
Expected: `PASS: built N base plugin(s)` including `plugins/basechat/dist/*.s2sp`. This is the
proof that `s2script.libraries` resolves the sibling from source.

- [ ] **Step 9: Commit**

```bash
git add packages/phrases-common plugins/basechat translations package-lock.json
git commit -m "plugins: the shared common phrase set, and basechat on it

The common set is a workspace library (s2script.kind: library) bundled into each
consumer's .s2sp — plugins/_shared/ would not work, because plugins/* is globbed
as both an npm workspace and s2script.workspace.plugins, so a shared directory
there is discovered and built as a plugin.

basechat is the pilot: own set loaded first, common second, so a plugin-specific
key shadows a shared one. Its colours are now phrase text an operator can edit."
```

---

## Task 7: Plugins batch A — the chat-heavy admin core

**Files:** for each of `basecommands`, `basebans`, `playercommands`, `basecomm`:
- Create: `plugins/<name>/src/phrases.ts`
- Modify: `plugins/<name>/src/plugin.ts`, `plugins/<name>/package.json`
- Generated: `translations/<name>.phrases.json`

**Interfaces:**
- Consumes: `@s2script/phrases-common` and the `Translations.load` ordering contract from Task 6.

Per-plugin checklist — repeat all of it for each of the four, following the basechat pattern in
Task 6 Step 5 exactly:

- [ ] **Step 1: `basecommands`**
  - Create `src/phrases.ts` with a key for every user-facing literal (~30 direct call sites, 95 literals — read the file and convert each).
  - Add `"s2script": { "libraries": ["@s2script/phrases-common"] }` to `package.json`.
  - Add the two `Translations.load` calls at the top of the plugin body, own set first.
  - Convert every `cmd.reply("…")` to `cmd.replyT("<key>", …args)` and every `Chat.*` literal to `Translations.translate(slot, "<key>", …args)`.
  - Move the `[SM] ` prefix into the phrase text; do not concatenate it in code.
  - Reuse a `common` key instead of adding a duplicate whenever the string already exists there.
  - Run: `node --experimental-strip-types --no-warnings scripts/gen-phrases.mjs && bash scripts/check-plugins-typecheck.sh`
  - Commit: `git add plugins/basecommands translations && git commit -m "basecommands: move user-facing strings onto the translations SDK"`

- [ ] **Step 2: `basebans`** — same checklist (12 direct call sites, 46 literals).

- [ ] **Step 3: `playercommands`** — same checklist (12 direct call sites, 34 literals).

- [ ] **Step 4: `basecomm`** — same checklist (3 direct call sites, 15 literals).

- [ ] **Step 5: Verify the batch**

Run: `bash scripts/check-phrases-generated.sh && bash scripts/check-plugins-typecheck.sh && bash scripts/build-base-plugins.sh`
Expected: all PASS

---

## Task 8: Plugins batch B — votes, triggers and the opt-in four

**Files:** for each of `basevotes`, `basetriggers`, `adminhelp`, `adminmenu`, `funcommands`, `antiflood`, `clientprefs`, `reservedslots`:
- Create: `plugins/<name>/src/phrases.ts`
- Modify: `plugins/<name>/src/plugin.ts`, `plugins/<name>/package.json`

and for each of `disabled/funvotes`, `disabled/nextmap`, `disabled/nominations`, `disabled/rockthevote`:
- Create: `plugins/disabled/<name>/src/phrases.ts`
- Modify: `plugins/disabled/<name>/src/plugin.ts`, `plugins/disabled/<name>/package.json`

**Interfaces:**
- Consumes: as Task 7.

- [ ] **Step 1: The eight enabled plugins**

For each of `basevotes`, `basetriggers`, `adminhelp`, `adminmenu`, `funcommands`, `antiflood`,
`clientprefs`, `reservedslots`, run the Task 7 per-plugin checklist verbatim.

Note: `antiflood`, `clientprefs` and `reservedslots` report zero *direct* call sites but are not
stringless — they build messages by concatenation or via `Chat.toAll` with a variable. Read each
file and convert every literal a player or admin can see.

- [ ] **Step 2: The four opt-in plugins**

For each of `disabled/funvotes`, `disabled/nextmap`, `disabled/nominations`,
`disabled/rockthevote`, run the same checklist. `rockthevote` reports zero direct call sites and 66
literals — it is the clearest example of the concatenation caveat above, so read it in full.

- [ ] **Step 3: Verify the batch**

Run: `node --experimental-strip-types --no-warnings scripts/gen-phrases.mjs && bash scripts/check-phrases-generated.sh && bash scripts/check-plugins-typecheck.sh && bash scripts/build-base-plugins.sh`
Expected: all PASS, 18 `.s2sp` artifacts

- [ ] **Step 4: Commit**

```bash
git add plugins translations
git commit -m "plugins: move the remaining base plugins onto the translations SDK"
```

---

## Task 9: `zones`

Kept separate because it is the one plugin with no SourceMod counterpart, and the one most
reasonable to drop if the PR needs trimming.

**Files:**
- Create: `plugins/zones/src/phrases.ts`
- Modify: `plugins/zones/src/plugin.ts`, `plugins/zones/package.json`

- [ ] **Step 1: Convert `zones`**

Run the Task 7 per-plugin checklist (30 direct call sites, 43 literals, 492 lines). Its strings are
mostly operator-facing diagnostics from `sm_zone_edit` / `sm_zone_export`; convert them for
consistency with the rest of the suite.

- [ ] **Step 2: Verify**

Run: `bash scripts/check-phrases-generated.sh && bash scripts/check-plugins-typecheck.sh`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add plugins/zones translations
git commit -m "zones: move user-facing strings onto the translations SDK"
```

---

## Task 10: The language fixture, changeset, and full gate

**Files:**
- Create: `translations/de/trdemo.phrases.json`
- Modify: `examples/cookbook/src/recipes/translations.ts` (comment only)
- Create: `.changeset/<name>.md` if and only if `packages/sdk` or `packages/cs2` changed

**Interfaces:**
- Produces: the first real language file in the repo — the `<code>/` path's first coverage.

- [ ] **Step 1: Ship the fixture the cookbook already documents**

Create `translations/de/trdemo.phrases.json`:

```json
{
  "Greeting": "Hallo {1}",
  "Bye": "Auf Wiedersehen {1}"
}
```

`OnlyEn` is deliberately absent: the cookbook asserts it falls back to the seed, and that assertion
is only meaningful if the German file really lacks the key.

- [ ] **Step 2: Correct the cookbook comment**

In `examples/cookbook/src/recipes/translations.ts`, change the line reading
`// switch the server default to German -> reads translations/de/trdemo.phrases.json (operator-seeded)`
to:

```ts
      // switch the server default to German -> reads translations/de/trdemo.phrases.json, which
      // SHIPS in the addon (scripts/package-addon.sh copies translations/). This is the only
      // language file in the repo and exists to exercise the <code>/ read path end to end.
```

- [ ] **Step 3: Verify the fixture is packaged**

Run: `bash scripts/package-addon.sh && cat dist/addons/s2script/translations/de/trdemo.phrases.json`
Expected: the file's contents print.

- [ ] **Step 4: Add a changeset if a published package changed**

Run: `git diff --name-only origin/main -- packages/ | grep -v phrases-common`

If that prints nothing, skip this step. If it prints files under `packages/sdk` or `packages/cs2`,
create `.changeset/colour-tags.md`:

```markdown
---
"@s2script/sdk": patch
"@s2script/cs2": patch
---

Document colour tags in phrase text: `{green}`, `{default}` and the rest of the
`ChatColors` names are expanded at output, so operators recolour messages by
editing a phrases file rather than forking a plugin.
```

Include only the package names that actually changed.

- [ ] **Step 5: Run the full gate suite**

Run: `make ci`
Expected: `ci-native` EXIT 0 and `ci-js` EXIT 0.

> `ci-native` needs the submodules. If `gen-licenses` fails with a missing
> `third_party/metamod-source/LICENSE.txt`, run `git submodule update --init --recursive` first.

- [ ] **Step 6: Commit**

```bash
git add translations/de examples/cookbook .changeset
git commit -m "translations: ship the German fixture the cookbook documents

The <code>/ language path has never been exercised by a real file — only by the
__s2_tr_injectLang test hook. The cookbook described this fixture and shipped
nothing, so its German assertions proved nothing about the file read."
```

---

## Task 11: Live gate

**Files:** none — verification only.

- [ ] **Step 1: Build deployable binaries**

Run:
```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: `s2script.so` and `libs2script_core.so`, repackaged into `dist/`.

- [ ] **Step 2: Start the server**

Run: `make docker-test`
Then: `docker exec s2script-cs2 /patch-gameinfo.sh` if the addon does not load.

- [ ] **Step 3: Verify the chat path renders colour**

Run: `python3 scripts/rcon.py "sm_say hello"`
Expected: the message appears in game chat with `(ALL)` in green and the body in the default
colour. A literal `{green}` anywhere in the output is a failure.

- [ ] **Step 4: Verify the console path is byte-clean**

Run: `python3 scripts/rcon.py "sm_psay notarealplayer hello"`
Expected: the rcon reply reads `[SM] No matching players` as plain text — no stray braces, no
control bytes, no escape artefacts.

- [ ] **Step 5: Verify the language file is really read**

Run: `python3 scripts/rcon.py "sm plugins list"` to confirm the cookbook is loaded, then check the
server console for the cookbook's translation lines.
Expected: `[cookbook] translations de: Hallo world` — read from the shipped file through
`translations_read`, not from an injected test map.

- [ ] **Step 6: Verify an unknown tag is caught, not shown**

Run: `python3 scripts/rcon.py "sm_say {nope}test"`
Expected: chat shows `test` with no braces, and the server console carries exactly one
`[s2script] WARN: unknown colour tag {nope}` line.

- [ ] **Step 7: Tear down**

Run: `docker compose -f docker/docker-compose.yml down`

- [ ] **Step 8: Open the PR**

```bash
git push -u origin i18n/base-plugin-phrases
```

Write the body to a file and use `gh pr edit N --body-file` — never a heredoc, because shell
escaping mangles tables and code blocks. The body must carry **Why**, the ownership split between
core and the game package, the two correctness fixes and what made them reachable, the packaging
gap, and the live-gate output from Steps 3-6.

---

## Self-Review

**Spec coverage.** §A colour tags → Tasks 1, 2, 4. §B phrase files, keys, `common`, prefixes →
Tasks 5, 6. §C generator + freshness + packaging → Task 5. §D1 ordering → Task 3. §D2 injection →
Task 3. §E gates → Tasks 1 (colors gate), 3 (ordering/injection tests), 5 (freshness), 10 (`make
ci`), 11 (live). §F rollout → Tasks 6-9, all 18 plugins accounted for: basechat (6); basecommands,
basebans, playercommands, basecomm (7); basevotes, basetriggers, adminhelp, adminmenu, funcommands,
antiflood, clientprefs, reservedslots, funvotes, nextmap, nominations, rockthevote (8); zones (9).
§G order of work → task order.

Two §E items are carried by existing gates rather than new tests, deliberately: the
directory-name/`load`-name/filename agreement is enforced structurally, because `gen-phrases.mjs`
derives the filename **from** the directory name, so the only way to break it is a wrong string in
`Translations.load` — caught the first time a phrase renders as its raw key in Task 11's live gate.
The own-set-before-`common` precedence is asserted by Task 3's D1 test, which loads two sets and
requires the later one's language hit to win.

**Placeholder scan.** No TBD/TODO. Every code step carries real code. The per-plugin steps in Tasks
7-9 reference the fully worked basechat conversion in Task 6 Step 5 rather than repeating it 17
times; each names its own file paths, its own counts, and its own commit.

**Type consistency.** `phrases` is the exported name in every seed module and in
`packages/phrases-common/index.ts`; `gen-phrases.mjs` accepts `mod.phrases ?? mod.default`.
`Translations.load(name, seed)` / `Translations.translate(slot, key, ...args)` / `ctx.replyT(key,
...args)` match `packages/sdk/translations.d.ts` unchanged. `__s2_colors` exposes exactly
`setTable` / `expand` / `chatLine` / `consoleLine` / `_resetWarnings` in Task 1, and Tasks 2 and 4
call only those.

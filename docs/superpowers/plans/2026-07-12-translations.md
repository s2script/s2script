# `@s2script/translations` — i18n — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** SourceMod-style translations — per-client language, JSON phrase files (root default + language-code subfolders), positional-arg formatting, and `ctx.replyT`.

**Architecture:** Two new engine-generic ops (`translations_read` mirrors the config-file read but points at `translations/` with a language subfolder; `client_language` mirrors `client_name` but reads `cl_language`). The engine-generic `@s2script/translations` prelude module holds the phrase registry (seed default + lazily-read language files), the `cl_language`→code map, positional `{1}` formatting, and the fallback chain. `ctx.replyT` is sugar in the commands module.

**Tech Stack:** C++ (shim ops), Rust (the `S2EngineOps` ABI mirror + the V8 natives), JavaScript (the `@s2script/translations` prelude + `ctx.replyT` in `core/src/v8host.rs`), TypeScript (`packages/translations/index.d.ts`).

**Design:** `docs/superpowers/specs/2026-07-12-translations-design.md`

## Global Constraints

- **Engine-generic throughout** — the ops (`translations_read`/`client_language`) read a file/client-cvar (no CS2/game symbol); `@s2script/translations` is a core prelude module; `ctx.replyT` is the engine-generic commands module. **No CS2 / `packages/cs2` change.** Core boundary gate (`scripts/check-core-boundary.sh`) must stay green.
- **ABI-append discipline:** the two new ops go **after the current last op `convar_register`**, byte-identical across (1) the C header `shim/include/s2script_core.h` (typedef + struct field), (2) the Rust mirror `core/src/v8host.rs` (type alias + `Option<Fn>` field), (3) **both** test `S2EngineOps` literals, and (4) the shim `ops.` assignment `shim/src/s2script_mm.cpp`. Order is the ABI.
- **`packages/*` changes** (new `packages/translations`) → **branch → PR → include a Changesets changeset** (`@s2script/translations`). This is a PR, not a local merge.
- **Degrade-never-crash:** every native `catch_unwind`s and degrades to null/`""` when its op is absent; a malformed/absent phrases file → fall back (default → key); `translate` never throws.
- **No runtime writes** — `translations_read` is read-only; the `seed` is the in-memory English default; the root file is an optional operator override.
- **Fallback chain** (in `translate`): the slot's language file → the root/default (seed or root-file override) → the key itself. `slot < 0` (console/rcon) → the server default language.
- Core tests run serial (`RUST_TEST_THREADS=1`).

---

## File Structure

- **Modify** `shim/include/s2script_core.h` — 2 op typedefs + 2 struct fields (after `convar_register`).
- **Modify** `shim/src/s2script_mm.cpp` — `TranslationsPath` + `s2_translations_read` + `s2_client_language` impls + the 2 `ops.` assignments.
- **Modify** `core/src/v8host.rs` — 2 Rust type aliases + 2 `Option<Fn>` mirror fields (+ both test op-structs) + 2 V8 natives (`__s2_translations_read`, `__s2_client_language`) + their registration + the `@s2script/translations` prelude module + `ctx.replyT` in `__s2cmd_ctx`.
- **Create** `packages/translations/{package.json,index.d.ts}` + `.changeset/translations.md`.
- **Create** `examples/translations-demo/{package.json,tsconfig.json,src/plugin.ts}`.

---

## Task 1: The two engine ops (shim + ABI + V8 natives)

**Files:**
- Modify: `shim/include/s2script_core.h`, `shim/src/s2script_mm.cpp`, `core/src/v8host.rs`

**Interfaces:**
- Produces (JS natives): `__s2_translations_read(lang, name) -> string | null` (reads `translations/[<lang>/]<name>.phrases.json`; `lang == ""` → root); `__s2_client_language(slot) -> string | null` (the client's `cl_language`).

- [ ] **Step 1: C header — the two op typedefs + struct fields.** In `shim/include/s2script_core.h`, add after the `s2_convar_register_fn convar_register;` line (before `} S2EngineOps;`):

```c
    /* Translations slice — APPENDED after convar_register; order is the ABI. */
    /* translations_read(lang,name): content of translations/[<lang>/]<name>.phrases.json, or null.
       lang=="" -> the root file. Both segments sanitized; ".." refused. Valid until the next call. */
    s2_translations_read_fn  translations_read;
    /* client_language(slot): the client's cl_language ("english"/"german"/...), or null. */
    s2_client_language_fn    client_language;
```
and the typedefs near the other `typedef ... _fn;` lines (e.g. beside `s2_config_read_file_fn`):
```c
typedef const char* (*s2_translations_read_fn)(const char* lang, const char* name);
typedef const char* (*s2_client_language_fn)(int slot);
```

- [ ] **Step 2: Shim impls** (`shim/src/s2script_mm.cpp`). Add `TranslationsPath` (mirror `ConfigFilePath` — the `dladdr`+`dirname`×3 walk — but base `translations/` + an optional language folder), `s2_translations_read`, and `s2_client_language` (mirror `s2_client_name`), near the config-file helpers:

```cpp
// TranslationsPath: <addon>/translations/[<lang>/]<name>.phrases.json. Mirrors ConfigFilePath's walk +
// sanitize (non-[A-Za-z0-9._-] -> '_', neutralizing '/'); refuses a segment containing ".." or empty name.
static std::string TranslationsPath(const char* lang, const char* name) {
    if (!name || !*name) return "";
    auto bad = [](const char* s) { return !s ? false : std::string(s).find("..") != std::string::npos; };
    if (bad(lang) || bad(name)) return "";
    auto sani = [](const char* p) { std::string o; for (; p && *p; ++p) { char c = *p;
        o += ((c>='A'&&c<='Z')||(c>='a'&&c<='z')||(c>='0'&&c<='9')||c=='.'||c=='_'||c=='-') ? c : '_'; } return o; };
    std::string safeLang = lang ? sani(lang) : "";
    std::string safeName = sani(name);
    Dl_info info;
    std::string root;
    if (dladdr(reinterpret_cast<void*>(&TranslationsPath), &info) && info.dli_fname) {
        char buf[4096]; snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf); snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);             snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);
        root = dir + "/translations/";
    } else {
        root = "addons/s2script/translations/";
    }
    if (!safeLang.empty()) root += safeLang + "/";
    return root + safeName + ".phrases.json";
}
static std::string s_translationsReadBuf;
static const char* s2_translations_read(const char* lang, const char* name) {
    std::string path = TranslationsPath(lang, name);
    if (path.empty()) return nullptr;
    std::ifstream f(path); if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf(); s_translationsReadBuf = ss.str();
    return s_translationsReadBuf.c_str();
}
static const char* s2_client_language(int slot) {
    if (!s_pEngine || !s2_client_valid(slot)) return nullptr;
    return s_pEngine->GetClientConVarValue(CPlayerSlot(slot), "cl_language");
}
```
Then, beside the other `ops.` assignments (e.g. after `ops.convar_register = ...` if present, else after `ops.config_read_file = ...`):
```cpp
    ops.translations_read = &s2_translations_read;
    ops.client_language   = &s2_client_language;
```

- [ ] **Step 3: Rust ABI mirror.** In `core/src/v8host.rs`, add the two type aliases beside `ConfigReadFileFn`/`ClientNameFn`:
```rust
type TranslationsReadFn = extern "C" fn(lang: *const c_char, name: *const c_char) -> *const c_char;
pub type ClientLanguageFn = extern "C" fn(slot: c_int) -> *const c_char;
```
and the two `Option<Fn>` fields at the END of the `S2EngineOps` struct mirror (after `pub convar_register: Option<ConvarRegisterFn>,`):
```rust
    pub translations_read: Option<TranslationsReadFn>,
    pub client_language:   Option<ClientLanguageFn>,
```
Then update **both** test `S2EngineOps { … }` literals — `grep -n 'S2EngineOps {' core/src/v8host.rs` finds them (the `#[cfg(test)]` constructions); add `translations_read: None, client_language: None,` to each (matching field order).

- [ ] **Step 4: Write the failing test** (the natives degrade with no ops — in the admin/frame test module):
```rust
    #[test]
    fn translations_natives_degrade_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // no ENGINE_OPS installed in tests -> read returns null, client_language returns null/"".
        assert_eq!(eval_in_context_string("p", "String(__s2_translations_read('', 'x'))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_translations_read('de', 'x'))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_language(0))"), "null");
        shutdown();
    }
```

- [ ] **Step 5: Run — expect FAIL** (`__s2_translations_read` undefined).

Run: `cd core && cargo test translations_natives_degrade_without_ops`
Expected: FAIL.

- [ ] **Step 6: The two V8 natives.** `s2_client_language` mirrors `s2_client_name` (v8host.rs:4938) exactly (call `ops.client_language`). `s2_translations_read` mirrors it but two string args:
```rust
fn s2_client_language(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_language else { return };
        let ptr = func(slot);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}
fn s2_translations_read(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 2 { return; }
        let lang = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.translations_read else { return };
        let c_lang = std::ffi::CString::new(lang).unwrap_or_default();
        let c_name = std::ffi::CString::new(name).unwrap_or_default();
        let ptr = func(c_lang.as_ptr(), c_name.as_ptr());
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}
```
Register them beside `__s2_client_name` (v8host.rs:5463):
```rust
    set_native(scope, global_obj, "__s2_translations_read", s2_translations_read);
    set_native(scope, global_obj, "__s2_client_language", s2_client_language);
```

- [ ] **Step 7: Run — expect PASS.**

Run: `cd core && cargo test translations_natives_degrade_without_ops && cargo test && bash scripts/check-core-boundary.sh`
Expected: the new test + full suite PASS; boundary green.

- [ ] **Step 8: Commit.**

```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp core/src/v8host.rs
git commit -m "feat(translations): translations_read + client_language engine ops + natives"
```

---

## Task 2: `@s2script/translations` prelude module

**Files:**
- Modify: `core/src/v8host.rs` (a new prelude module block, beside another `__s2pkg_*` module e.g. `__s2pkg_server`/`__s2pkg_config`)
- Test: `core/src/v8host.rs` (translation logic tests)

**Interfaces:**
- Consumes: Task 1's `__s2_translations_read`/`__s2_client_language`; existing `console`.
- Produces (JS): `globalThis.__s2pkg_translations = { Translations: { load, translate, setDefaultLanguage } }`. Test hooks: `__s2_tr_format`, `__s2_tr_langCode`, `__s2_tr_injectLang`.

- [ ] **Step 1: Write the failing tests:**
```rust
    #[test]
    fn translations_format_and_langcode() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // positional {1}/{2}; missing {3} -> empty; no args -> literal text
        assert_eq!(eval_in_context_string("p", "__s2_tr_format('Slapped {1} for {2}', ['Bob','5'])"), "Slapped Bob for 5");
        assert_eq!(eval_in_context_string("p", "__s2_tr_format('a {3} b', ['x'])"), "a  b");
        assert_eq!(eval_in_context_string("p", "__s2_tr_format('plain', [])"), "plain");
        // cl_language -> folder code
        assert_eq!(eval_in_context_string("p", "__s2_tr_langCode('german')"), "de");
        assert_eq!(eval_in_context_string("p", "__s2_tr_langCode('english')"), "");   // root
        assert_eq!(eval_in_context_string("p", "__s2_tr_langCode('klingon')"), "");   // unknown -> default(root)
        shutdown();
    }
    #[test]
    fn translations_fallback_chain() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "\
            __s2pkg_translations.Translations.load('t', { Hi: 'Hi {1}', Only: 'Only-EN' });\
            __s2_tr_injectLang('t', 'de', { Hi: 'Hallo {1}' });\
        ").unwrap();
        // slot<0 default(root/en): seed
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Hi','Bob')"), "Hi Bob");
        // default language de -> the injected de map; a key missing in de falls back to the seed
        eval_in_context("p", "__s2pkg_translations.Translations.setDefaultLanguage('de');").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Hi','Bob')"), "Hallo Bob");
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Only')"), "Only-EN"); // de miss -> seed
        // an unknown key -> the key itself
        assert_eq!(eval_in_context_string("p", "__s2pkg_translations.Translations.translate(-1,'Nope')"), "Nope");
        shutdown();
    }
```

- [ ] **Step 2: Run — expect FAIL** (`__s2_tr_format` / `__s2pkg_translations` undefined).

Run: `cd core && cargo test translations_format_and_langcode translations_fallback_chain`
Expected: FAIL.

- [ ] **Step 3: Add the prelude module** (in the injected prelude string, beside another `__s2pkg_*` block):
```javascript
  // --- @s2script/translations — SM-style i18n. Phrases: a flat key->text map; the plugin's `seed` is the
  //     in-memory English default; translations/<code>/<name>.phrases.json (read lazily) overrides per language;
  //     an optional root translations/<name>.phrases.json overrides the seed. Fully engine-generic. ---
  var __s2_tr_reg = {};              // name -> { def: {k:text}, langs: { code: {k:text}|null } }  (null = tried+absent)
  var __s2_tr_default = "";          // server/console default language code ("" = root/English)
  // Steam cl_language -> folder code ("" = root/English). Unmapped -> "" (default).
  var __s2_TR_CODES = { english:"", german:"de", russian:"ru", french:"fr", spanish:"es", latam:"es",
    schinese:"zh", tchinese:"zh", portuguese:"pt", brazilian:"pt", polish:"pl", italian:"it", dutch:"nl",
    swedish:"sv", danish:"da", finnish:"fi", norwegian:"no", czech:"cs", hungarian:"hu", turkish:"tr",
    japanese:"ja", koreana:"ko", thai:"th", ukrainian:"uk", bulgarian:"bg", greek:"el", romanian:"ro" };
  function __s2_tr_langCode(clLang) {
    var v = __s2_TR_CODES[String(clLang || "").toLowerCase()];
    return v == null ? "" : v;
  }
  function __s2_tr_format(text, args) {
    return String(text).replace(/\{(\d+)\}/g, function (_m, n) {
      var i = (parseInt(n, 10) | 0) - 1;
      return (args && i >= 0 && i < args.length && args[i] != null) ? String(args[i]) : "";
    });
  }
  function __s2_tr_parse(text) { try { var o = JSON.parse(text); return (o && typeof o === "object") ? o : {}; } catch (e) { console.log("[s2script] WARN: translations file malformed — ignored"); return {}; } }
  function __s2_tr_langMap(name, code) {                     // the lazily-read (+cached) map for a code ("" = root override)
    var r = __s2_tr_reg[name]; if (!r) return null;
    if (Object.prototype.hasOwnProperty.call(r.langs, code)) return r.langs[code];   // cached (map or null)
    var text = __s2_translations_read(code, name);           // null if absent/no-op
    var map = (text == null) ? null : __s2_tr_parse(text);
    r.langs[code] = map;
    return map;
  }
  var __s2_translations = {
    load: function (name, seed) {
      name = String(name);
      __s2_tr_reg[name] = { def: (seed && typeof seed === "object") ? seed : {}, langs: {} };
      var root = __s2_translations_read("", name);           // OPTIONAL root override of the seed
      if (root != null) { var o = __s2_tr_parse(root); for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) __s2_tr_reg[name].def[k] = o[k]; }
    },
    setDefaultLanguage: function (code) { __s2_tr_default = String(code || ""); },
    translate: function (slot, key) {
      var args = [].slice.call(arguments, 2);
      key = String(key);
      var code = ((slot | 0) < 0) ? __s2_tr_default : __s2_tr_langCode(__s2_client_language(slot | 0));
      // search EVERY loaded phrase set: the code's lang map (if not root) -> the default/seed -> the key.
      for (var name in __s2_tr_reg) {
        if (!Object.prototype.hasOwnProperty.call(__s2_tr_reg, name)) continue;
        if (code) { var lm = __s2_tr_langMap(name, code); if (lm && lm[key] != null) return __s2_tr_format(lm[key], args); }
        var d = __s2_tr_reg[name].def; if (d[key] != null) return __s2_tr_format(d[key], args);
      }
      return key;                                            // ultimate fallback
    },
  };
  globalThis.__s2_tr_format = __s2_tr_format;                 // test hooks (pure)
  globalThis.__s2_tr_langCode = __s2_tr_langCode;
  globalThis.__s2_tr_injectLang = function (name, code, obj) { if (__s2_tr_reg[name]) __s2_tr_reg[name].langs[code] = obj; };  // test hook (bypasses the file read)
  globalThis.__s2pkg_translations = { Translations: __s2_translations };
```
(Note the `translate` searches every loaded phrase set for the key — SM's `%t` looks a phrase up across all a plugin's loaded phrase files by key; a plugin typically loads one, so this is O(1) in practice and correct for multiple.)

- [ ] **Step 4: Run tests — expect PASS.**

Run: `cd core && cargo test translations`
Expected: PASS (the two new tests + Task 1's degrade test).

- [ ] **Step 5: Commit.**

```bash
git add core/src/v8host.rs
git commit -m "feat(translations): @s2script/translations module (registry, format, fallback)"
```

---

## Task 3: `ctx.replyT` + `packages/translations` + changeset

**Files:**
- Modify: `core/src/v8host.rs` (`ctx.replyT` in `__s2cmd_ctx`, ~1287)
- Create: `packages/translations/{package.json,index.d.ts}`, `.changeset/translations.md`

**Interfaces:**
- Consumes: Task 2's `__s2pkg_translations.Translations.translate`.
- Produces: `ctx.replyT(key, ...args)`.

- [ ] **Step 1: Add `replyT` to the ctx** (in `__s2cmd_ctx`, right after the `reply:` function, before the closing `}` of the returned object):
```javascript
      // Localized reply: translate `key` for the CALLER's language, then reply (SM's %t on the reply path).
      // Soft-deps @s2script/translations — degrades to the key if translations isn't loaded.
      replyT: function (key) {
        var t = globalThis.__s2pkg_translations;
        if (!t) { this.reply(String(key)); return; }
        this.reply(t.Translations.translate.apply(t.Translations, [s, key].concat([].slice.call(arguments, 1))));
      },
```

- [ ] **Step 2: Write the failing test** (replyT translates for a console caller; `reply` for `s<0` → `console.log`, captured in `LOG`):
```rust
    #[test]
    fn ctx_replyt_localizes() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "\
            __s2pkg_translations.Translations.load('c', { Kicked: 'Kicked {1}' });\
            __s2pkg_commands.Commands.register('sm_x', function (ctx) { ctx.replyT('Kicked', 'Bob'); });\
        ").unwrap();
        // invoke the command with a console caller (slot -1) via the dispatch registry
        eval_in_context("p", "__s2pkg_commands.Commands.dispatch('sm_x', -1, '');").unwrap();
        assert!(LOG.lock().unwrap().iter().any(|l| l.contains("Kicked Bob")), "replyT should have logged the translated string");
        shutdown();
    }
```
(Confirm the command dispatch-by-name hook name: `grep -n 'dispatch:' core/src/v8host.rs` — it's on `__s2pkg_commands.Commands`; adjust the call if the signature differs.)

- [ ] **Step 3: Run — expect FAIL, then verify PASS after Step 1** (Step 1 already added replyT; if the test was written first, run it fail→pass; the ordering here is: replyT exists from Step 1, so this test should PASS once written).

Run: `cd core && cargo test ctx_replyt_localizes`
Expected: PASS.

- [ ] **Step 4: Create `packages/translations/package.json`** (mirror `packages/ws/package.json`; version `0.1.0`):
```json
{
  "name": "@s2script/translations",
  "version": "0.1.0",
  "types": "index.d.ts",
  "publishConfig": { "access": "public" },
  "files": ["index.d.ts"],
  "repository": { "type": "git", "url": "https://github.com/GabeHirakawa/s2script.git" }
}
```

- [ ] **Step 5: Create `packages/translations/index.d.ts`:**
```typescript
/** @s2script/translations — SourceMod-style i18n (per-client language, phrase files, {1} formatting). */
export type Phrases = Record<string, string>;
export declare const Translations: {
  /** Register a phrase set: `seed` is the built-in English default; translations/<code>/<name>.phrases.json overrides per language. */
  load(name: string, seed: Phrases): void;
  /** Translate `key` for `slot`'s language (slot < 0 = the server default), substituting positional {1}/{2} args. */
  translate(slot: number, key: string, ...args: (string | number)[]): string;
  /** Set the server/console default language code (default "" = root/English). */
  setDefaultLanguage(code: string): void;
};
```
Also extend `packages/commands/index.d.ts`'s `CommandContext` with `replyT(key: string, ...args: (string | number)[]): void;` (find the `reply(...)` line and add the sibling).

- [ ] **Step 6: Create the changeset** `.changeset/translations.md`:
```markdown
---
"@s2script/translations": patch
"@s2script/commands": patch
---

New `@s2script/translations` package: SourceMod-style i18n — per-client language (via `cl_language`), JSON phrase files (a root English default + `translations/<code>/` per-language folders), positional `{1}` formatting, `Translations.translate` / `Translations.load`. `@s2script/commands`' `CommandContext` gains `replyT(key, ...args)` — reply to the caller in their language.
```

- [ ] **Step 7: Verify.**

Run: `cd core && cargo test && bash scripts/check-plugins-typecheck.sh`
Expected: core PASS; typecheck green (the `.d.ts` additions compile; the Task 4 demo will consume them).

- [ ] **Step 8: Commit.**

```bash
git add core/src/v8host.rs packages/translations packages/commands/index.d.ts .changeset/translations.md
git commit -m "feat(translations): ctx.replyT + packages/translations types + changeset"
```

---

## Task 4: demo + live gate

**Files:**
- Create: `examples/translations-demo/{package.json,tsconfig.json,src/plugin.ts}`

**Interfaces:**
- Consumes: `@s2script/translations`, `@s2script/commands`.

- [ ] **Step 1: Write the demo** (`examples/translations-demo/src/plugin.ts`; mirror `examples/clients-demo` for package.json/tsconfig — `@demo/translations-demo`, `s2script.apiVersion "1.x"`):
```typescript
// translations-demo — proves @s2script/translations: a seed English default, positional {1} formatting,
// the default-language switch reading translations/de/trdemo.phrases.json live, and the key fallback.
import { Translations } from "@s2script/translations";
import { Commands } from "@s2script/commands";

export function onLoad(): void {
  Translations.load("trdemo", { Greeting: "Hello {1}", Bye: "Goodbye {1}", OnlyEn: "English only" });

  // default (root / English) — slot -1 uses the server default ("" = root)
  console.log(`[translations-demo] en: ${Translations.translate(-1, "Greeting", "world")}`);      // Hello world
  console.log(`[translations-demo] en missing-key: ${Translations.translate(-1, "Nope")}`);        // Nope (fallback)

  // switch the server default to German -> reads translations/de/trdemo.phrases.json (operator-seeded)
  Translations.setDefaultLanguage("de");
  console.log(`[translations-demo] de: ${Translations.translate(-1, "Greeting", "world")}`);       // Hallo world (from de file)
  console.log(`[translations-demo] de fallback-to-seed: ${Translations.translate(-1, "OnlyEn")}`);  // English only (de miss -> seed)
  Translations.setDefaultLanguage("");

  // ctx.replyT from the console
  Commands.register("sm_trhello", (ctx) => { ctx.replyT("Greeting", "admin"); });
  console.log("[translations-demo] onLoad — sm_trhello registered");
}
```

- [ ] **Step 2: Typecheck + core + sniper build.**
```bash
bash scripts/check-plugins-typecheck.sh && (cd core && cargo test) && \
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: typecheck green; core PASS; sniper OK (GLIBC floors unchanged; the ops add no deps).

- [ ] **Step 3: Deploy + seed a `de/` phrases file + start.** (build-sniper wipes `dist/addons/s2script`.)
```bash
bash scripts/build-base-plugins.sh
node packages/cli/dist/cli.js build examples/translations-demo
mkdir -p dist/addons/s2script/translations/de dist/addons/s2script/configs dist/addons/s2script/data
chmod 777 dist/addons/s2script/configs dist/addons/s2script/data
printf '%s\n' '{ "Greeting": "Hallo {1}", "Bye": "Auf Wiedersehen {1}" }' > dist/addons/s2script/translations/de/trdemo.phrases.json
cp plugins/*/dist/_s2script_*.s2sp examples/translations-demo/dist/*.s2sp dist/addons/s2script/plugins/
rm -f dist/addons/s2script/plugins/_s2script_zones-lib.s2sp
(cd docker && docker compose restart cs2)
```

- [ ] **Step 4: Verify the live gate.** In `docker logs s2script-cs2` (after the boot window):
  - `[translations-demo] en: Hello world` (seed default + format).
  - `[translations-demo] en missing-key: Nope` (fallback to key).
  - `[translations-demo] de: Hallo world` (the `translations/de/trdemo.phrases.json` file was READ live + formatted).
  - `[translations-demo] de fallback-to-seed: English only` (a key absent in `de` falls back to the seed).
  - `scripts/rcon.py "sm_trhello"` → `[SM]`-style `Hello admin` (console = server default; replyT works).
  - `GAMEDATA n/0`, `RestartCount=0`, no crash.
  - **Deferred (human-client):** a real client with `cl_language="german"` seeing German output — bots have no `cl_language`.

- [ ] **Step 5: Commit.**

```bash
git add examples/translations-demo docs/superpowers/plans/2026-07-12-translations.md
git commit -m "feat(translations): translations-demo (seed/format/de-file/fallback) + live gate"
```

---

## Deferred (do NOT build ahead)

- Typed format specifiers (`{1:d}`/`{1:f}`); `%T`-style explicit-target-language formatting.
- An operator config for the server-default language; auto-generating the root phrases file from the seed (a `translations_write` op).
- Eager-load-all-languages / a directory-scan op; hot-reload of phrase files.
- Base-plugin retrofit (every base plugin's strings → phrases); a `languages.cfg`-style editable `cl_language`→code map.

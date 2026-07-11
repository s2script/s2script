# Admin groups, immunity & overrides — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring SourceMod's admin group / immunity / command-override model to s2script — JSON-shaped, SteamID64, SM semantics.

**Architecture:** Groups resolve at load time in the `@s2script/admin` JS prelude module; the *results* (effective mask, immunity, per-admin + global overrides) land in the host-global core admin cache so isolated plugin contexts share them. Command gating consults overrides via a command-name-aware check hook; immunity filters targets via a new `Player.target(pattern, callerSlot, filterImmunity)` param wired into the destructive base commands.

**Tech Stack:** Rust (core `thread_local!` cache + `set_native` natives), JavaScript (the engine-generic admin prelude in `core/src/v8host.rs`; the CS2 `Player.target` in `games/cs2/js/pawn.js`), TypeScript (base plugins).

**Design:** `docs/superpowers/specs/2026-07-11-admin-groups-immunity-design.md`

## Global Constraints

- **Boundary:** the core cache + natives are engine-generic (SteamID64 → record; no CS2 symbol). `@s2script/admin` is an engine-generic prelude module. Only `Player.target` immunity filtering + base-command wiring touch the CS2 layer (`pawn.js`, `plugins/*`). Both boundary gates must stay green.
- **No shim change, no new `S2EngineOps` op** — every new native is `set_native`'d in core. One sniper rebuild (core `.so` only): shim GLIBC ≤2.14 / core GLIBC ≤2.30.
- **Degrade-never-crash:** every native `catch_unwind`s; malformed/missing files WARN + ignore (never throw into the engine).
- **Core tests run serial** (`.cargo/config.toml` sets `RUST_TEST_THREADS=1`); mirror the existing admin tests' `eval_in_context` / `eval_in_context_string` style.
- **Bots read SteamID `"0"`** → never an admin → immunity 0 (retain the existing `forSlot` `"0"` guard).
- **Flag-letter map (no remapping needed):** `a`=RESERVATION(1<<0) … `n`=CHEATS(1<<13), `z`=ROOT(1<<14).

---

## File Structure

- `core/src/v8host.rs`
  - **Cache** (~L526–535): add 4 `thread_local!`s — `ADMIN_FILE_IMMUNITY`, `ADMIN_RUNTIME_IMMUNITY` (`HashMap<String,i32>`), `ADMIN_OVERRIDES` (`HashMap<String,HashMap<String,(u64,bool)>>`), `ADMIN_GLOBAL_OVERRIDES` (`HashMap<String,(u64,bool)>`).
  - **Natives** (~L5747–5800): extend `s2_admin_set` (immunity arg), `s2_admin_remove`, `s2_admin_clear_file`; add `s2_admin_get_immunity`, `s2_admin_add_override`, `s2_admin_set_global_override`, `s2_admin_override`. Register them (~L5300).
  - **Admin prelude module** (~L1352–1409): the flag parser, group/admin/override resolution, the immunity + groups API, `canTarget`, the override-aware `__s2_admin_check`, and `__s2_admin_can_target`.
  - **Commands prelude** (L1300–1312): `registerAdmin` passes the command name to the check.
  - **Tests** (~L9760, ~L9868): update 3-arg `__s2_admin_set` calls; add new-native + resolution tests.
- `games/cs2/js/pawn.js` (L109): `Player.target` gains a `filterImmunity` param.
- `plugins/{playercommands,basecommands,basebans,basecomm,funcommands,basevotes}/src/plugin.ts`: pass `filterImmunity=true` at destructive call sites.
- `examples/admin-groups-demo/` (new): a live-gate demo that logs resolved records + override lookups.

---

## Task 1: Core cache + natives (immunity + overrides)

**Files:**
- Modify: `core/src/v8host.rs` (cache thread_locals ~L526; natives ~L5747–5800; registration ~L5300; low-level tests ~L9760, ~L9868; prelude call sites L1374, L1389)

**Interfaces:**
- Consumes: existing `ADMIN_FILE`/`ADMIN_RUNTIME` (`HashMap<String,u64>`), `set_native`.
- Produces (JS natives):
  - `__s2_admin_set(sid, mask, immunity, runtime)` — now 4-arg (was 3).
  - `__s2_admin_get_immunity(sid) -> number` — `max(file, runtime)`, 0 if absent.
  - `__s2_admin_add_override(sid, cmd, mask, isPublic)` — a per-admin (file-tier) override.
  - `__s2_admin_set_global_override(cmd, mask, isPublic)` — a global (file-tier) override.
  - `__s2_admin_override(sid, cmd) -> string` — `""` (none) / `"public"` / decimal mask; per-admin beats global.
  - `__s2_admin_remove(sid, runtime)`, `__s2_admin_clear_file()` — now also clear immunity/overrides.

- [ ] **Step 1: Add the cache thread_locals.** In the `thread_local!` block (after `ADMIN_FILE_LOADED`, ~L535):

```rust
    /// Slice admin-groups: per-tier immunity levels (mirrors ADMIN_FILE/ADMIN_RUNTIME). get = max(file, runtime).
    static ADMIN_FILE_IMMUNITY:    std::cell::RefCell<std::collections::HashMap<String, i32>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static ADMIN_RUNTIME_IMMUNITY: std::cell::RefCell<std::collections::HashMap<String, i32>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Per-admin command overrides (file tier only — the resolver merges an admin's groups' override
    /// blocks). sid -> cmd -> (required_mask, is_public). is_public true => anyone (flag "").
    static ADMIN_OVERRIDES: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashMap<String, (u64, bool)>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Global command overrides (admin_overrides.json). cmd -> (required_mask, is_public).
    static ADMIN_GLOBAL_OVERRIDES: std::cell::RefCell<std::collections::HashMap<String, (u64, bool)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
```

- [ ] **Step 2: Write the failing test** (in the admin test module, alongside the existing admin test ~L9760):

```rust
    #[test]
    fn admin_immunity_and_overrides() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // immunity: max across tiers
        eval_in_context("p", "__s2_admin_set('222', 4, 30, false); __s2_admin_set('222', 8, 70, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('222'))"), "12");        // 4|8
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('222'))"), "70"); // max(30,70)
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('999'))"), "0");  // absent
        // overrides: per-admin beats global; public sentinel
        eval_in_context("p", "__s2_admin_set_global_override('sm_x', 2, false); __s2_admin_add_override('222','sm_x',4,false);").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_x')"), "4");    // per-admin wins
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('other','sm_x')"), "2");  // falls to global
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','nope')"), "");     // no override
        eval_in_context("p", "__s2_admin_set_global_override('sm_pub', 0, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_pub')"), "public");
        // clear_file wipes file immunity + overrides + global overrides; runtime immunity survives
        eval_in_context("p", "__s2_admin_clear_file();").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('222'))"), "70"); // runtime kept
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_x')"), "");
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('other','sm_x')"), "");
        shutdown();
    }
```

- [ ] **Step 3: Run it — expect FAIL** (`__s2_admin_get_immunity` etc. undefined):

Run: `cd core && cargo test admin_immunity_and_overrides -- --nocapture`
Expected: FAIL (ReferenceError / undefined native).

- [ ] **Step 4: Extend `s2_admin_set`** (add the immunity arg + write the tier's immunity map):

```rust
/// `__s2_admin_set(steamid, flags, immunity, runtime)` — set/overwrite a SteamID's flags + immunity in
/// the file(false)/runtime(true) tier.
fn s2_admin_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let flags = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let immunity = args.get(2).number_value(scope).unwrap_or(0.0) as i32;
        let runtime = args.get(3).boolean_value(scope);
        if runtime {
            ADMIN_RUNTIME.with(|m| { m.borrow_mut().insert(sid.clone(), flags); });
            ADMIN_RUNTIME_IMMUNITY.with(|m| { m.borrow_mut().insert(sid, immunity); });
        } else {
            ADMIN_FILE.with(|m| { m.borrow_mut().insert(sid.clone(), flags); });
            ADMIN_FILE_IMMUNITY.with(|m| { m.borrow_mut().insert(sid, immunity); });
        }
    }));
}
```

- [ ] **Step 5: Add the immunity + override natives** (after `s2_admin_get`):

```rust
/// `__s2_admin_get_immunity(steamid) -> number` — max immunity across both tiers (0 = none).
fn s2_admin_get_immunity(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { rv.set_double(0.0); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let f = ADMIN_FILE_IMMUNITY.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        let r = ADMIN_RUNTIME_IMMUNITY.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        rv.set_double(f.max(r) as f64);
    }));
}

/// `__s2_admin_add_override(steamid, cmd, mask, isPublic)` — a per-admin (file-tier) command override.
fn s2_admin_add_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let cmd = args.get(1).to_rust_string_lossy(scope);
        let mask = args.get(2).number_value(scope).unwrap_or(0.0) as u64;
        let is_public = args.get(3).boolean_value(scope);
        ADMIN_OVERRIDES.with(|m| {
            m.borrow_mut().entry(sid).or_default().insert(cmd, (mask, is_public));
        });
    }));
}

/// `__s2_admin_set_global_override(cmd, mask, isPublic)` — a global (file-tier) command override.
fn s2_admin_set_global_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let cmd = args.get(0).to_rust_string_lossy(scope);
        let mask = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let is_public = args.get(2).boolean_value(scope);
        ADMIN_GLOBAL_OVERRIDES.with(|m| { m.borrow_mut().insert(cmd, (mask, is_public)); });
    }));
}

/// `__s2_admin_override(steamid, cmd) -> string` — "" (none) / "public" / decimal mask. Per-admin beats global.
fn s2_admin_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { let s = v8::String::new(scope, "").unwrap(); rv.set(s.into()); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let cmd = args.get(1).to_rust_string_lossy(scope);
        let hit = ADMIN_OVERRIDES.with(|m| m.borrow().get(&sid).and_then(|c| c.get(&cmd).copied()))
            .or_else(|| ADMIN_GLOBAL_OVERRIDES.with(|m| m.borrow().get(&cmd).copied()));
        let out = match hit {
            None => String::new(),
            Some((_, true)) => "public".to_string(),
            Some((mask, false)) => mask.to_string(),
        };
        let s = v8::String::new(scope, &out).unwrap();
        rv.set(s.into());
    }));
}
```

- [ ] **Step 6: Extend `s2_admin_remove` + `s2_admin_clear_file`** to cover the new maps:

```rust
// in s2_admin_remove, inside each branch:
//   runtime:  ADMIN_RUNTIME.with(...remove); ADMIN_RUNTIME_IMMUNITY.with(|m| { m.borrow_mut().remove(&sid); });
//   file:     ADMIN_FILE.with(...remove);    ADMIN_FILE_IMMUNITY.with(|m| { m.borrow_mut().remove(&sid); });

// s2_admin_clear_file — wipe the file tier + file-tier immunity + overrides + global overrides:
fn s2_admin_clear_file(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ADMIN_FILE.with(|m| m.borrow_mut().clear());
        ADMIN_FILE_IMMUNITY.with(|m| m.borrow_mut().clear());
        ADMIN_OVERRIDES.with(|m| m.borrow_mut().clear());
        ADMIN_GLOBAL_OVERRIDES.with(|m| m.borrow_mut().clear());
    }));
}
```

(For `s2_admin_remove`, edit the two existing tier branches to also remove from the matching immunity map, as commented above.)

- [ ] **Step 7: Register the new natives** (in the `set_native` block ~L5301, after `__s2_admin_get`):

```rust
    set_native(scope, global_obj, "__s2_admin_get_immunity", s2_admin_get_immunity);
    set_native(scope, global_obj, "__s2_admin_add_override", s2_admin_add_override);
    set_native(scope, global_obj, "__s2_admin_set_global_override", s2_admin_set_global_override);
    set_native(scope, global_obj, "__s2_admin_override", s2_admin_override);
```

- [ ] **Step 8: Keep the prelude + existing tests green under the new 4-arg `set`.** Update the two prelude call sites and the two existing test call sites to pass immunity in slot 3 (behavior identical — Task 2 rewrites the prelude callers fully):
  - L1374 `__s2_admin_set(String(sid), mask, false);` → `__s2_admin_set(String(sid), mask, 0, false);`
  - L1389 `add: function (steamId, flags) { __s2_admin_set(String(steamId), flags | 0, true); },` → `add: function (steamId, flags) { __s2_admin_set(String(steamId), flags | 0, 0, true); },`
  - L9760 `__s2_admin_set('111', 4, false); __s2_admin_set('111', 1, true);` → `__s2_admin_set('111', 4, 0, false); __s2_admin_set('111', 1, 0, true);`
  - L9868 `__s2_admin_set('0', __s2pkg_admin.ADMFLAG.ROOT, true);` → `__s2_admin_set('0', __s2pkg_admin.ADMFLAG.ROOT, 0, true);`

- [ ] **Step 9: Run tests — expect PASS.**

Run: `cd core && cargo test admin -- --nocapture`
Expected: PASS (`admin_immunity_and_overrides` + the existing `admin*` tests).

- [ ] **Step 10: Commit.**

```bash
git add core/src/v8host.rs
git commit -m "feat(admin): core cache immunity + command overrides (natives)"
```

---

## Task 2: The `@s2script/admin` module — resolution, immunity/groups API, override-aware gating

**Files:**
- Modify: `core/src/v8host.rs` (admin prelude module ~L1352–1409)
- Test: `core/src/v8host.rs` (admin test module)

**Interfaces:**
- Consumes: Task 1 natives; existing `__s2_config_read_raw`/`__s2_config_write_raw`, `__s2_client_steamid`, `__s2_admin_mark_loaded`, `__s2_admin_clear_file`.
- Produces (JS): `ADMFLAG` (unchanged), `Admin.{add(steamId,flags,immunity?),remove,get,forSlot,reload,canTarget(cs,ts),getGroup(name)}`, `AdminInfo.{immunity, groups, hasFlags}`, `globalThis.__s2_admin_check(slot, mask, cmdName?)` (override-aware), `globalThis.__s2_admin_can_target(cs, ts)`. Test hooks on `globalThis`: `__s2_admin_parseFlags`, `__s2_admin_parseGroups`, `__s2_admin_parseAdmins`, `__s2_admin_resolveEntry`, `__s2_canTargetImm`.

- [ ] **Step 1: Write the failing tests** (in the admin test module):

```rust
    #[test]
    fn admin_flag_parser() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('bcdefg'))"), "126"); // bits 1..6
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags(['kick','ban']))"), "12"); // KICK|BAN
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('kick'))"), "4");   // whole string = a name
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('z'))"), "16384");  // ROOT
        shutdown();
    }

    #[test]
    fn admin_group_resolution() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "__s2_admin_parseGroups('{\"G\":{\"flags\":\"cd\",\"immunity\":50}}');").unwrap();
        // own immunity 10 loses to group 50; flags = own(none) ∪ group(KICK|BAN)=12; groups=['G']
        assert_eq!(eval_in_context_string("p",
            "(function(){var r=__s2_admin_resolveEntry({groups:['G'],immunity:10}); return r.mask+'/'+r.immunity+'/'+r.groups.join(',');})()"),
            "12/50/G");
        // unknown group skipped, own flags kept
        assert_eq!(eval_in_context_string("p",
            "(function(){var r=__s2_admin_resolveEntry({groups:['Nope'],flags:['slay']}); return r.mask+'/'+r.groups.length;})()"),
            "32/0");
        // full push: parseGroups then parseAdmins(pushCore) -> Admin.get reads immunity + groups from core+registry
        eval_in_context("p", "__s2_admin_parseAdmins('{\"111\":{\"groups\":[\"G\"],\"immunity\":5}}', true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('111').immunity)"), "50");
        assert_eq!(eval_in_context_string("p", "__s2pkg_admin.Admin.get('111').groups.join(',')"), "G");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('nobody'))"), "null");
        shutdown();
    }

    #[test]
    fn admin_can_target_immunity() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(-1, 0, 100))"), "true");  // console infinite
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 0, 0))"), "true");      // non-immune target
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 50, 100))"), "false");  // punch up blocked
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 100, 50))"), "true");   // punch down
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 50, 50))"), "true");    // equal can target
        shutdown();
    }
```

- [ ] **Step 2: Run — expect FAIL** (`__s2_admin_parseFlags` etc. undefined).

Run: `cd core && cargo test admin_flag_parser admin_group_resolution admin_can_target_immunity`
Expected: FAIL.

- [ ] **Step 3: Replace the admin prelude module** (the block from `var __s2_ADMFLAG = {` through `globalThis.__s2pkg_admin = {...};`, ~L1352–1409) with:

```javascript
  // --- admin module (engine-generic; ADMFLAG + Admin API + group/immunity/override resolution) ---
  var __s2_ADMFLAG = {
    RESERVATION: 1<<0, GENERIC: 1<<1, KICK: 1<<2, BAN: 1<<3, UNBAN: 1<<4, SLAY: 1<<5, CHANGEMAP: 1<<6,
    CONVARS: 1<<7, CONFIG: 1<<8, CHAT: 1<<9, VOTE: 1<<10, PASSWORD: 1<<11, RCON: 1<<12, CHEATS: 1<<13, ROOT: 1<<14,
  };
  function __s2_hasFlags(flags, req) { return ((flags & __s2_ADMFLAG.ROOT) !== 0) || ((flags & req) === req); }

  // ---- flag-token parsing (a name, a single SM letter, or a compact letter-string) ----
  function __s2_flag_letterBit(ch) {
    if (ch === "z" || ch === "Z") return __s2_ADMFLAG.ROOT;
    var i = String(ch).charCodeAt(0) - 97;                 // 'a'
    return (i >= 0 && i <= 13) ? (1 << i) : 0;
  }
  function __s2_flag_token(tok) {                           // name OR single letter -> bit (0 = unknown)
    var up = String(tok).toUpperCase();
    if (__s2_ADMFLAG[up] != null) return __s2_ADMFLAG[up];
    var s = String(tok);
    return (s.length === 1) ? __s2_flag_letterBit(s) : 0;
  }
  function __s2_parseFlags(value) {                         // array of tokens | a name | a letter-string -> mask
    var mask = 0;
    if (Array.isArray(value)) {
      for (var i = 0; i < value.length; i++) {
        var b = __s2_flag_token(value[i]);
        if (b) mask |= b; else if (String(value[i]).length) console.log("[s2script] WARN: unknown admin flag '" + value[i] + "' — skipped");
      }
    } else if (typeof value === "string") {
      var up = value.toUpperCase();
      if (__s2_ADMFLAG[up] != null) return __s2_ADMFLAG[up];   // the whole string is a flag name
      for (var j = 0; j < value.length; j++) {
        var c = value.charAt(j), lb = __s2_flag_letterBit(c);
        if (lb) mask |= lb; else console.log("[s2script] WARN: unknown admin flag letter '" + c + "' — skipped");
      }
    }
    return mask;
  }
  function __s2_parseOverrideToken(v) {                     // "" -> public; else a required mask
    if (v === "" || v == null) return { public: true, mask: 0 };
    return { public: false, mask: __s2_parseFlags(v) };
  }

  // ---- registries (per-context; populated from the files at prelude time) ----
  var __s2_groups = {};        // name -> { flags, immunity, overrides: {cmd:{public,mask}} }
  var __s2_adminGroups = {};   // sid  -> [groupName]

  function __s2_admin_parseGroups(text) {
    __s2_groups = {};
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admin_groups.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var name in obj) {
      if (name === "_help" || !Object.prototype.hasOwnProperty.call(obj, name)) continue;
      var g = obj[name]; if (!g || typeof g !== "object") continue;
      var ov = {};
      if (g.overrides && typeof g.overrides === "object")
        for (var cmd in g.overrides) if (Object.prototype.hasOwnProperty.call(g.overrides, cmd)) ov[cmd] = __s2_parseOverrideToken(g.overrides[cmd]);
      __s2_groups[name] = { flags: __s2_parseFlags(g.flags), immunity: (typeof g.immunity === "number") ? (g.immunity | 0) : 0, overrides: ov };
    }
  }

  function __s2_admin_resolveEntry(entry) {                 // -> { mask, immunity, groups:[], overrides:{cmd:{public,mask}} }
    var mask = 0, immunity = 0, groups = [], overrides = {};
    if (Array.isArray(entry)) {
      mask = __s2_parseFlags(entry);
    } else if (entry && typeof entry === "object") {
      if (entry.flags != null) mask |= __s2_parseFlags(entry.flags);
      if (typeof entry.immunity === "number") immunity = Math.max(immunity, entry.immunity | 0);
      if (Array.isArray(entry.groups)) for (var i = 0; i < entry.groups.length; i++) {
        var gn = entry.groups[i], g = __s2_groups[gn];
        if (!g) { console.log("[s2script] WARN: admins.json references unknown group '" + gn + "' — skipped"); continue; }
        mask |= g.flags; immunity = Math.max(immunity, g.immunity); groups.push(gn);
        for (var c in g.overrides) if (Object.prototype.hasOwnProperty.call(g.overrides, c)) overrides[c] = g.overrides[c];
      }
    }
    return { mask: mask, immunity: immunity, groups: groups, overrides: overrides };
  }

  function __s2_admin_parseAdmins(text, pushCore) {
    __s2_adminGroups = {};
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admins.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var sid in obj) {
      if (sid === "_help" || !Object.prototype.hasOwnProperty.call(obj, sid)) continue;
      var r = __s2_admin_resolveEntry(obj[sid]);
      __s2_adminGroups[String(sid)] = r.groups;
      if (pushCore) {
        __s2_admin_set(String(sid), r.mask, r.immunity, false);
        for (var cmd in r.overrides) if (Object.prototype.hasOwnProperty.call(r.overrides, cmd)) {
          var ov = r.overrides[cmd]; __s2_admin_add_override(String(sid), cmd, ov.mask | 0, !!ov.public);
        }
      }
    }
  }

  function __s2_admin_parseOverrides(text) {                // global admin_overrides.json (pushCore path only)
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admin_overrides.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var cmd in obj) {
      if (cmd === "_help" || !Object.prototype.hasOwnProperty.call(obj, cmd)) continue;
      var ov = __s2_parseOverrideToken(obj[cmd]); __s2_admin_set_global_override(cmd, ov.mask | 0, !!ov.public);
    }
  }

  var __s2_GROUPS_TEMPLATE = '{\n  "_help": "Group name -> { flags, immunity, overrides }. flags: SM letters (\\"bcdefg\\") or names ([\\"kick\\",\\"ban\\"]); immunity: integer; overrides: { command: flag | \\"\\" for anyone }. e.g. \\"Full Admins\\": { \\"flags\\": \\"bcdefgjk\\", \\"immunity\\": 50 }"\n}\n';
  var __s2_ADMINS_TEMPLATE = '{\n  "_help": "SteamID64 -> [\\"flag\\",...] (flags only), or { groups:[\\"Group\\"], flags:[...], immunity:N }. Flags: reservation generic kick ban unban slay changemap convars config chat vote password rcon cheats root (or SM letters a-n,z)."\n}\n';
  var __s2_OVERRIDES_TEMPLATE = '{\n  "_help": "command -> required flag (name or SM letter), or \\"\\" for everyone. e.g. \\"sm_slap\\": \\"generic\\", \\"sm_who\\": \\"\\""\n}\n';
  function __s2_admin_readOrTemplate(name, template) {
    var t = __s2_config_read_raw(name);
    if (t == null) { __s2_config_write_raw(name, template); return "{}"; }
    return t;
  }
  function __s2_admin_reloadAll(pushCore) {
    __s2_admin_parseGroups(__s2_admin_readOrTemplate("admin_groups", __s2_GROUPS_TEMPLATE));
    __s2_admin_parseAdmins(__s2_admin_readOrTemplate("admins", __s2_ADMINS_TEMPLATE), pushCore);
    if (pushCore) __s2_admin_parseOverrides(__s2_admin_readOrTemplate("admin_overrides", __s2_OVERRIDES_TEMPLATE));
  }

  // ---- AdminInfo + the Admin API ----
  function __s2_adminInfo(steamId, flags, immunity) {
    return {
      steamId: String(steamId), flags: flags | 0, immunity: immunity | 0,
      groups: (__s2_adminGroups[String(steamId)] || []).slice(),
      hasFlags: function (req) { return __s2_hasFlags(flags | 0, req | 0); },
    };
  }
  function __s2_canTargetImm(callerSlot, callerImm, targetImm) {   // the pure immunity comparison (test hook)
    if ((callerSlot | 0) < 0) return true;                        // server console / rcon = infinite
    if ((targetImm | 0) <= 0) return true;                        // non-immune target
    return (callerImm | 0) >= (targetImm | 0);                    // SM default: equal can target
  }
  var __s2_admin = {
    add: function (steamId, flags, immunity) { __s2_admin_set(String(steamId), flags | 0, immunity | 0, true); },
    remove: function (steamId) { __s2_admin_remove(String(steamId), true); },
    get: function (steamId) {
      var sid = String(steamId), m = __s2_admin_get(sid), im = __s2_admin_get_immunity(sid);
      if (!m && !im) return null;
      return __s2_adminInfo(sid, m, im);
    },
    forSlot: function (slot) {
      var sid = __s2_client_steamid(slot | 0);
      if (sid === "0" || !sid) return null;                        // bot / mid-auth -> never an admin
      return __s2_admin.get(sid);
    },
    canTarget: function (callerSlot, targetSlot) {
      var t = __s2_admin.forSlot(targetSlot | 0), ti = t ? t.immunity : 0;
      var c = __s2_admin.forSlot(callerSlot | 0), ci = c ? c.immunity : 0;
      return __s2_canTargetImm(callerSlot | 0, ci, ti);
    },
    getGroup: function (name) {
      var g = __s2_groups[String(name)];
      return g ? { name: String(name), flags: g.flags, immunity: g.immunity, overrides: g.overrides } : null;
    },
    reload: function () { __s2_admin_clear_file(); __s2_admin_reloadAll(true); },
  };

  // test hooks (safe to expose; pure helpers)
  globalThis.__s2_admin_parseFlags = __s2_parseFlags;
  globalThis.__s2_admin_parseGroups = __s2_admin_parseGroups;
  globalThis.__s2_admin_parseAdmins = __s2_admin_parseAdmins;
  globalThis.__s2_admin_resolveEntry = __s2_admin_resolveEntry;
  globalThis.__s2_canTargetImm = __s2_canTargetImm;

  // Parse the registries in EVERY context (cheap, idempotent — makes getGroup / AdminInfo.groups work
  // everywhere); push the resolved admins + overrides into the shared core cache ONCE (first context).
  __s2_admin_reloadAll(!__s2_admin_mark_loaded());

  // Override-aware gating hook. A "public" override (flag "") grants ANYONE — even a non-admin; a flag
  // override changes the requirement; else the command's default mask. (registerAdmin already lets
  // callerSlot<0 / console through as root before reaching here.)
  globalThis.__s2_admin_check = function (slot, requiredMask, cmdName) {
    var sid = __s2_client_steamid(slot | 0);
    var ov = cmdName ? __s2_admin_override(sid || "", String(cmdName)) : "";
    if (ov === "public") return true;
    var a = __s2_admin.forSlot(slot | 0);
    if (!a) return false;
    if (ov !== "") return a.hasFlags(parseInt(ov, 10) | 0);
    return a.hasFlags(requiredMask | 0);
  };
  // Immunity targeting hook (consumed by the CS2 Player.target immunity filter, without importing this module).
  globalThis.__s2_admin_can_target = function (cs, ts) { return __s2_admin.canTarget(cs | 0, ts | 0); };
  globalThis.__s2pkg_admin = { ADMFLAG: __s2_ADMFLAG, Admin: __s2_admin };
```

- [ ] **Step 4: Run tests — expect PASS** (the new three + the existing `admin*`).

Run: `cd core && cargo test admin`
Expected: PASS. (The existing `admin_*` tests that call `__s2_admin_set` with 4 args still pass from Task 1.)

- [ ] **Step 5: Update `packages/admin/index.d.ts`** — add the new API surface:

```typescript
export interface AdminInfo {
  readonly steamId: string;
  readonly flags:   number;
  /** Immunity level (0 = none). A lower-immunity admin cannot target a higher one. */
  readonly immunity: number;
  /** Names of the groups this admin belongs to. */
  readonly groups:  readonly string[];
  hasFlags(required: number): boolean;
}

export interface AdminGroup {
  readonly name: string;
  readonly flags: number;
  readonly immunity: number;
  readonly overrides: Readonly<Record<string, { public: boolean; mask: number }>>;
}

export declare const Admin: {
  add(steamId: string, flags: number, immunity?: number): void;
  remove(steamId: string): void;
  get(steamId: string): AdminInfo | null;
  forSlot(slot: number): AdminInfo | null;
  /** True if `callerSlot` may act on `targetSlot` (console = infinite; blocked iff target immunity > caller). */
  canTarget(callerSlot: number, targetSlot: number): boolean;
  /** A resolved group by name (from admin_groups.json), or null. */
  getGroup(name: string): AdminGroup | null;
  reload(): void;
};
```

- [ ] **Step 6: Commit.**

```bash
git add core/src/v8host.rs packages/admin/index.d.ts
git commit -m "feat(admin): group/immunity/override resolution + Admin.canTarget/getGroup"
```

---

## Task 3: Enforcement plumbing — command-name in the gate + `Player.target` immunity filter

**Files:**
- Modify: `core/src/v8host.rs` (commands prelude, L1310)
- Modify: `games/cs2/js/pawn.js` (L109 `Player.target`)
- Modify: `packages/cs2/index.d.ts` (the `Player.target` signature)

**Interfaces:**
- Consumes: Task 2's `__s2_admin_check(slot, mask, cmdName)` + `__s2_admin_can_target(cs, ts)`.
- Produces: `Player.target(pattern, callerSlot, filterImmunity?)` — CS2 layer; default `false` (unchanged for existing callers).

- [ ] **Step 1: Pass the command name into the gate.** In the `registerAdmin` wrapper (L1310), change:

```javascript
        if (check(ctx.callerSlot, flags | 0)) { handler(ctx); }
```
to:
```javascript
        if (check(ctx.callerSlot, flags | 0, name)) { handler(ctx); }
```

(Backward-compatible: a check hook without the 3rd param ignores it. The `callerSlot < 0` root short-circuit above it is unchanged.)

- [ ] **Step 2: Add the `filterImmunity` param to `Player.target`** (`games/cs2/js/pawn.js`, replace L109–134):

```javascript
  // Player.target(pattern, callerSlot, filterImmunity) -> Player[] — SM target-string resolution.
  //   "#<userid>" -> that player; "@all" -> allConnected; "@me" -> the caller (empty from console);
  //   otherwise a case-insensitive name match (exact wins, else all partials). Empty on no match.
  //   filterImmunity (default false): drop targets the caller can't act on (admin immunity); used by
  //   the destructive base commands. Degrades to no-filter if @s2script/admin isn't loaded.
  Player.target = function (pattern, callerSlot, filterImmunity) {
    if (typeof pattern !== "string" || pattern.length === 0) return [];
    var res;
    if (pattern === "@all") {
      res = Player.allConnected();
    } else if (pattern === "@me") {
      if (typeof callerSlot !== "number" || callerSlot < 0) return [];
      var me = Player._fromSlotUnchecked(callerSlot);
      res = me ? [me] : [];
    } else if (pattern.charAt(0) === "#") {
      var uid = parseInt(pattern.slice(1), 10);
      if (isNaN(uid)) return [];
      var p = Player.fromUserId(uid);
      res = p ? [p] : [];
    } else {
      var needle = pattern.toLowerCase();
      var conn = Player.allConnected();
      var exact = [], partial = [];
      for (var i = 0; i < conn.length; i++) {
        var nm = conn[i].playerName;
        if (typeof nm !== "string") continue;
        var low = nm.toLowerCase();
        if (low === needle) exact.push(conn[i]);
        else if (low.indexOf(needle) !== -1) partial.push(conn[i]);
      }
      res = exact.length ? exact : partial;
    }
    if (filterImmunity && typeof globalThis.__s2_admin_can_target === "function") {
      var ct = globalThis.__s2_admin_can_target, out = [];
      for (var k = 0; k < res.length; k++) if (ct(callerSlot | 0, res[k].slot)) out.push(res[k]);
      return out;
    }
    return res;
  };
```

- [ ] **Step 3: Update the `Player.target` type** in `packages/cs2/index.d.ts` (find the current `target(pattern: string, callerSlot: number): Player[];` and add the optional third param):

```typescript
    target(pattern: string, callerSlot: number, filterImmunity?: boolean): Player[];
```

- [ ] **Step 4: Verify core + typecheck.**

Run: `cd core && cargo test admin && cd .. && bash scripts/check-plugins-typecheck.sh`
Expected: core PASS; typecheck green (no call site broke — the new param is optional).

- [ ] **Step 5: Commit.**

```bash
git add core/src/v8host.rs games/cs2/js/pawn.js packages/cs2/index.d.ts
git commit -m "feat(admin): thread command name into the gate + Player.target immunity filter"
```

---

## Task 4: Wire immunity filtering into the destructive base commands

**Files:**
- Modify: `plugins/playercommands/src/plugin.ts` (L45, L56, L68)
- Modify: `plugins/basecommands/src/plugin.ts` (L23 — sm_kick)
- Modify: `plugins/basebans/src/plugin.ts` (L51 — sm_ban)
- Modify: `plugins/basecomm/src/plugin.ts` (the shared `forTargets` helper + its call sites)
- Modify: `plugins/funcommands/src/plugin.ts` (the shared `forEachPawn` helper + its call sites)
- Modify: `plugins/basevotes/src/plugin.ts` (L50 — sm_votekick)

**Interfaces:**
- Consumes: `Player.target(pattern, callerSlot, filterImmunity)` (Task 3).
- Produces: destructive commands refuse higher-immunity targets; un-punish commands (ungag/unmute/unsilence/unfreeze) stay unfiltered.

- [ ] **Step 1: playercommands** (sm_slap L45, sm_slay L56, sm_rename L68) — each `Player.target(targetStr, ctx.callerSlot)` → `Player.target(targetStr, ctx.callerSlot, true)`.

- [ ] **Step 2: basecommands** (sm_kick L23): `Player.target(targetStr, ctx.callerSlot)` → `Player.target(targetStr, ctx.callerSlot, true)`. (Leave sm_who's non-target lookups alone.)

- [ ] **Step 3: basebans** (sm_ban L51): `Player.target(target, ctx.callerSlot)` → `Player.target(target, ctx.callerSlot, true)`.

- [ ] **Step 4: basevotes** (sm_votekick L50): `Player.target(targetStr, ctx.callerSlot)` → `Player.target(targetStr, ctx.callerSlot, true)`.

- [ ] **Step 5: basecomm** — thread a `filterImmunity` flag through the shared `forTargets`. Change its signature (L22) to add a trailing param and pass it through:

```typescript
function forTargets(pat: string, callerSlot: number, reply: (m: string) => void, verb: string, usage: string, act: (p: Player) => void, filterImmunity: boolean): void {
  if (!pat) { reply("Usage: " + usage); return; }
  const targets = Player.target(pat, callerSlot, filterImmunity);
  ...
}
```

Then pass `true` for the punish commands and `false` for the un- commands (append the arg to each `forTargets(...)` call): `sm_gag`→true, `sm_ungag`→false, `sm_mute`→true, `sm_unmute`→false, `sm_silence`→true, `sm_unsilence`→false.

- [ ] **Step 6: funcommands** — thread `filterImmunity` through `forEachPawn`. Change its signature (L24) and the resolve call (L30):

```typescript
function forEachPawn(ctx: CommandContext, usage: string, verb: string, fn: (p: Player, pw: Pawn) => void, filterImmunity: boolean): void {
  ...
  const targets = Player.target(pattern, ctx.callerSlot, filterImmunity);
  ...
}
```

Pass `true` for the effect commands (`sm_gravity`, `sm_blind`, `sm_noclip`, `sm_freeze`) and `false` for `sm_unfreeze` (append the arg to each `forEachPawn(...)` call).

- [ ] **Step 7: Build each plugin + typecheck-gate.**

Run: `for p in playercommands basecommands basebans basecomm funcommands basevotes; do (cd plugins/$p && npx s2script build) || exit 1; done`
Expected: each emits its `.s2sp` (build typechecks full strict).

- [ ] **Step 8: Commit.**

```bash
git add plugins/{playercommands,basecommands,basebans,basecomm,funcommands,basevotes}/src/plugin.ts
git commit -m "feat(admin): immunity-filter targets in the destructive base commands"
```

---

## Task 5: Demo, build, live gate

**Files:**
- Create: `examples/admin-groups-demo/` (package.json, tsconfig.json, src/plugin.ts)
- Modify: `docs/superpowers/plans/2026-07-11-admin-groups-immunity.md` (check off steps)

**Interfaces:**
- Consumes: `@s2script/admin` (`Admin.get`, `Admin.getGroup`), `@s2script/commands`.

- [ ] **Step 1: Write the demo** (`examples/admin-groups-demo/src/plugin.ts`) — logs resolved records + override lookups at load so the live gate can observe resolution at the data level (bots are non-admins, so admin-vs-admin immunity is a human-client deferral):

```typescript
// admin-groups-demo — logs the resolved admin model so the live gate can verify group/immunity/override
// resolution at the DATA level (bots read SteamID "0" → never admins, so real targeting is a human test).
import { Admin } from "@s2script/admin";
import { Commands } from "@s2script/commands";

// A synthetic SteamID64 the operator seeds into admins.json + admin_groups.json for the gate.
const SEED = "76561199000000001";

export function onLoad(): void {
  const a = Admin.get(SEED);
  console.log(`[admin-groups-demo] Admin.get(seed) = ${a ? `flags=${a.flags} immunity=${a.immunity} groups=[${a.groups.join(",")}]` : "null"}`);
  const g = Admin.getGroup("Full Admins");
  console.log(`[admin-groups-demo] getGroup('Full Admins') = ${g ? `flags=${g.flags} immunity=${g.immunity}` : "null"}`);
  // override lookups via the global native (proves admin_overrides.json + per-group overrides loaded)
  const ov = (globalThis as any).__s2_admin_override;
  if (typeof ov === "function") {
    console.log(`[admin-groups-demo] override sm_slap(global) = "${ov("", "sm_slap")}"`);
    console.log(`[admin-groups-demo] override sm_kick(seed)  = "${ov(SEED, "sm_kick")}"`);
  }
  Commands.register("sm_admingroups_dump", (ctx) => {
    const x = Admin.get(SEED);
    ctx.reply(x ? `seed flags=${x.flags} imm=${x.immunity} groups=${x.groups.join(",")}` : "seed not an admin");
  });
}
```

`package.json` + `tsconfig.json` mirror an existing `examples/*` demo (e.g. `examples/clients-demo`): name `@s2script/admin-groups-demo`, `s2script.pluginDependencies` on `@s2script/admin` + `@s2script/commands`, tsconfig extends `../../tsconfig.base.json`.

- [ ] **Step 2: Build the demo + full typecheck gate.**

Run: `(cd examples/admin-groups-demo && npx s2script build) && bash scripts/check-plugins-typecheck.sh && cd core && cargo test`
Expected: `.s2sp` emitted; typecheck green; core tests PASS.

- [ ] **Step 3: Sniper rebuild** (the new/extended natives + prelude — core `.so` only):

Run: `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`
Expected: `libs2script_core.so` (GLIBC ≤2.30) rebuilt; no shim change needed.

- [ ] **Step 4: Deploy + recreate the writable configs dir** (build-sniper wipes `dist/addons/s2script`):

```bash
mkdir -p dist/addons/s2script/configs && \
cp plugins/*/dist/*.s2sp examples/admin-groups-demo/dist/*.s2sp dist/addons/s2script/plugins/ && \
cd docker && docker compose restart cs2
```

- [ ] **Step 5: Seed the config files + reload, then verify the live gate.** Write a seed `admin_groups.json` (`"Full Admins": {"flags":"bcdefgjk","immunity":50,"overrides":{"sm_kick":""}}`, `"Root": {"flags":"z","immunity":100}`), `admins.json` (`"76561199000000001": {"groups":["Full Admins"],"immunity":80}`), and `admin_overrides.json` (`"sm_slap":"generic"`) into `dist/addons/s2script/configs/`, then restart. Verify via `docker logs` + `python3 scripts/rcon.py`:
  - First boot with **no** files auto-generated all three as **valid JSON** (`_help` templates).
  - `[admin-groups-demo] Admin.get(seed) = flags=... immunity=80 groups=[Full Admins]` (own 80 > group 50; flags = the group's `bcdefgjk`).
  - `getGroup('Full Admins') = flags=... immunity=50`.
  - `override sm_slap(global) = "<generic-mask>"`, `override sm_kick(seed) = "public"` (per-group override).
  - `GAMEDATA VALIDATION: <N> ok, 0 FAILED`; console/root still runs commands; `RestartCount=0`; no crash.

- [ ] **Step 6: Commit the demo.**

```bash
git add examples/admin-groups-demo docs/superpowers/plans/2026-07-11-admin-groups-immunity.md
git commit -m "feat(admin): admin-groups live-gate demo + plan"
```

---

## Deferred (do NOT build ahead)

- Immunity *groups* (group-A-immune-to-group-B) and `sm_immunity_mode` variants.
- A KeyValues / `admins_simple.ini` / `STEAM_2/3` import path; SQL admin sources; `admins.cfg` rich blocks; per-admin passwords.
- Precise "that player is immune" per-command messaging (empty-result reply suffices for v1).
- Command *groups* in overrides (SM's category tokens) — only literal command names in v1.
- The human-client live test: true admin-vs-admin immunity blocking + a non-admin running a `""`-override command (bots are SteamID `"0"` → never admins).

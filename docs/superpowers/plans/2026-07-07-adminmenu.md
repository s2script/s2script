# adminmenu (TopMenu framework) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the SourceMod adminmenu — an extensible, flag-gated `sm_admin` menu built on `@s2script/menu`, backed by a core TopMenu registry that command plugins register their actions into.

**Architecture:** A host-global core TopMenu registry (categories + owner-tracked items, mirroring `CONCOMMANDS`) exposed via `@s2script/topmenu`; item `onSelect` handlers are held as `v8::Global`s and invoked in the registering plugin's context. Because a menu's `onSelect` runs under the isolate borrow, the cross-context select dispatch is **post-drain deferred** (mirrors `dispatch_pending_cookie_cached`). The `adminmenu` CS2 plugin renders the flag-filtered registry with `@s2script/menu`; a `pickPlayer` helper drives target selection.

**Tech Stack:** Rust (core registry + natives + prelude JS in `core/src/v8host.rs`, `ffi.rs` wiring), JavaScript (`@s2script/topmenu` prelude module, `games/cs2/js/pawn.js`), TypeScript (`.d.ts` + plugins via `s2script build`).

## Global Constraints

- **Charter boundary — core engine-generic; games are packages; deps game→core.** The TopMenu registry + `@s2script/topmenu` carry NO game identifiers (categories/items/ids/flags are plain data; owner-dispatch mirrors `CONCOMMANDS`). CS2 facts (`Player`, `pawn.*`, `Bans`, `Server`, `ADMFLAG` usage) live in `adminmenu`, `pickPlayer` (`pawn.js`), and the command plugins. `scripts/check-core-boundary.sh` + `scripts/test-boundary-nameleak.sh` must stay green.
- **Select dispatch is POST-DRAIN, never synchronous.** A menu `onSelect` runs under `HOST.borrow_mut()`; a synchronous cross-context dispatch would double-borrow. `__s2_topmenu_select` only QUEUES; `dispatch_pending_topmenu_select` (called from `ffi.rs` after `frame_async_drain()`) fans out with HOST free. Mirror `dispatch_pending_cookie_cached` exactly (snapshot, `try_borrow_mut`, `is_live`, per-call `TryCatch`).
- **Owner-scoped, no new op/shim.** Items are owned by `current_plugin`, dropped on unload (beside the `CONCOMMANDS` cleanup), cleared on `shutdown`. All natives via `set_native` — no new `S2EngineOps` op, no shim change (the cookies pattern).
- **Naming:** PascalCase types (`TopMenu`, `Menu`), camelCase methods/props (`addItem`, `onSelect`).
- **Test running:** core tests serial (`cd core && cargo test`); in-isolate prelude tests use `eval_std(...)` / `load_plugin_js(...)` in `v8host.rs` `frame_tests`. Full spec: `docs/superpowers/specs/2026-07-07-adminmenu-design.md`.

---

### Task 1: Core TopMenu registry + natives + `@s2script/topmenu`

The engine-generic foundation: the registry, the four natives, the post-drain select dispatch, the prelude module, and the `.d.ts`.

**Files:**
- Modify: `core/src/v8host.rs` — registry `thread_local`s; `s2_topmenu_add_category`/`add_item`/`snapshot`/`select` natives + registration; `dispatch_pending_topmenu_select`; the `@s2script/topmenu` prelude module; unload + shutdown cleanup; `frame_tests`.
- Modify: `core/src/ffi.rs` — call `dispatch_pending_topmenu_select()` after `frame_async_drain()`.
- Create: `packages/topmenu/package.json`, `packages/topmenu/index.d.ts`.

**Interfaces:**
- Consumes: `current_plugin`, `PLUGINS`, `REGISTRY.is_live`, `HOST` (all in `v8host.rs`); the `dispatch_pending_cookie_cached` pattern (v8host.rs:2527) and `s2_concommand` (v8host.rs:2273).
- Produces:
  - `globalThis.__s2pkg_topmenu = { TopMenu }` where `TopMenu` = `{ addCategory(name), addItem(category, {id, name, flags, onSelect}), snapshot(): {categories: string[], items: [{id, category, name, flags}]}, select(id, slot) }`.
  - Natives `__s2_topmenu_add_category(name)`, `__s2_topmenu_add_item(category, id, name, flags, fn)`, `__s2_topmenu_snapshot() -> {categories, items}`, `__s2_topmenu_select(id, slot)`.
  - `dispatch_pending_topmenu_select()` (pub(crate)).

- [ ] **Step 1: Write the failing in-isolate tests**

Add to `frame_tests` in `core/src/v8host.rs`:

```rust
#[test]
fn topmenu_add_snapshot_and_owner_scoped() {
    init(dummy_logger()).unwrap();
    // A plugin adds a category + two items; snapshot returns them (metadata only).
    load_plugin_js("tm_a", r#"
        var { TopMenu } = globalThis.__s2pkg_topmenu;
        TopMenu.addCategory("Player Commands");
        TopMenu.addItem("Player Commands", { id: "a:kick", name: "Kick", flags: 8, onSelect: function(){} });
        TopMenu.addItem("Player Commands", { id: "a:slap", name: "Slap", flags: 16, onSelect: function(){} });
    "#).unwrap();
    let out = eval_std("q1", r#"
        var s = globalThis.__s2pkg_topmenu.TopMenu.snapshot();
        JSON.stringify({ cats: s.categories, ids: s.items.map(function(i){return i.id;}).sort(),
                         kick: s.items.filter(function(i){return i.id==="a:kick";})[0] });
    "#);
    assert_eq!(out, r#"{"cats":["Player Commands"],"ids":["a:kick","a:slap"],"kick":{"id":"a:kick","category":"Player Commands","name":"Kick","flags":8}}"#);
    shutdown();
}

#[test]
fn topmenu_select_dispatches_to_owner_post_drain() {
    init(dummy_logger()).unwrap();
    load_plugin_js("tm_b", r#"
        var { TopMenu } = globalThis.__s2pkg_topmenu;
        globalThis.__tm_picked = null;
        TopMenu.addItem("Player Commands", { id: "b:kick", name: "Kick", flags: 8,
            onSelect: function(slot){ globalThis.__tm_picked = "b:kick@" + slot; } });
    "#).unwrap();
    // select QUEUES; it must NOT have fired yet (synchronous would double-borrow).
    eval_std("q2", r#" globalThis.__s2pkg_topmenu.TopMenu.select("b:kick", 3); "#);
    // fan out post-drain (HOST free) — dispatch runs the owner's onSelect.
    dispatch_pending_topmenu_select();
    let out = eval_in_context_string("tm_b", r#" String(globalThis.__tm_picked) "#);
    assert_eq!(out, "b:kick@3");
    shutdown();
}

#[test]
fn topmenu_unload_drops_owner_items() {
    init(dummy_logger()).unwrap();
    load_plugin_js("tm_c", r#"
        var { TopMenu } = globalThis.__s2pkg_topmenu;
        TopMenu.addItem("Player Commands", { id: "c:ban", name: "Ban", flags: 2, onSelect: function(){} });
    "#).unwrap();
    unload_plugin("tm_c", false);   // Vanished
    let out = eval_std("q3", r#" String(globalThis.__s2pkg_topmenu.TopMenu.snapshot().items.length) "#);
    assert_eq!(out, "0");   // the departed plugin's item is gone
    shutdown();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd core && cargo test topmenu`
Expected: FAIL — `__s2pkg_topmenu` is undefined / `dispatch_pending_topmenu_select` not found.

- [ ] **Step 3: Add the registry + natives + post-drain dispatch**

In `core/src/v8host.rs`, add the registry `thread_local`s beside `CONCOMMANDS` (search `static CONCOMMANDS`):

```rust
    /// TopMenu registry (adminmenu framework). Ordered category names (deduped) + items owned by a
    /// plugin. Item `onSelect` is a Global<Function> held like a command handler (NOT marshalled;
    /// invoked in the owner's context on select). Owner-scoped teardown mirrors CONCOMMANDS.
    static TOPMENU_CATEGORIES: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    static TOPMENU_ITEMS: std::cell::RefCell<std::collections::HashMap<String, TopMenuItem>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Slots+ids queued by __s2_topmenu_select (called under the isolate borrow from a menu onSelect);
    /// fanned out post-frame by dispatch_pending_topmenu_select (ffi.rs, HOST free). Same discipline as
    /// COOKIE_CACHED_PENDING — sidesteps the re-entrant double-borrow.
    static TOPMENU_PENDING: std::cell::RefCell<Vec<(String, i32)>> = std::cell::RefCell::new(Vec::new());
```

Add the item struct near the other core structs (e.g. beside `JsHandler`):

```rust
/// A registered TopMenu item. `on_select` is invoked in `owner`'s context (liveness-gated by `generation`).
struct TopMenuItem {
    category: String,
    name: String,
    flags: i64,
    owner: String,
    generation: u64,
    on_select: v8::Global<v8::Function>,
}
```

Add the four natives (mirror `s2_concommand` at v8host.rs:2273 for the store, `s2_commands_list` at :3352 for the snapshot-build). Complete:

```rust
/// `__s2_topmenu_add_category(name)` — append a category if absent (order = insertion; deduped).
fn s2_topmenu_add_category(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        TOPMENU_CATEGORIES.with(|c| { let mut b = c.borrow_mut(); if !b.contains(&name) { b.push(name); } });
    }));
}

/// `__s2_topmenu_add_item(category, id, name, flags, onSelectFn)` — register/replace an item owned by
/// current_plugin. Auto-creates the category (order hint). Mirrors s2_concommand's owner+gen+Global store.
fn s2_topmenu_add_item(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 5 { return; }
        let category = args.get(0).to_rust_string_lossy(scope);
        let id = args.get(1).to_rust_string_lossy(scope);
        let name = args.get(2).to_rust_string_lossy(scope);
        let flags = args.get(3).integer_value(scope).unwrap_or(0);
        let func_local = match v8::Local::<v8::Function>::try_from(args.get(4)) { Ok(f) => f, Err(_) => return };
        let on_select = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        TOPMENU_CATEGORIES.with(|c| { let mut b = c.borrow_mut(); if !b.contains(&category) { b.push(category.clone()); } });
        TOPMENU_ITEMS.with(|m| m.borrow_mut().insert(id, TopMenuItem { category, name, flags, owner, generation, on_select }));
    }));
}

/// `__s2_topmenu_snapshot() -> { categories: string[], items: [{id, category, name, flags}] }` (metadata only).
fn s2_topmenu_snapshot(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cats: Vec<String> = TOPMENU_CATEGORIES.with(|c| c.borrow().clone());
        let items: Vec<serde_json::Value> = TOPMENU_ITEMS.with(|m| m.borrow().iter().map(|(id, it)| {
            serde_json::json!({ "id": id, "category": it.category, "name": it.name, "flags": it.flags })
        }).collect());
        let obj = serde_json::json!({ "categories": cats, "items": items });
        // serialize to a JS value via the JSON string round-trip (the established snapshot pattern).
        if let Some(s) = v8::String::new(scope, &obj.to_string()) {
            if let Some(parsed) = v8::json::parse(scope, s) { rv.set(parsed); }
        }
    }));
}

/// `__s2_topmenu_select(id, slot)` — QUEUE a select for post-drain dispatch (never synchronous — a menu
/// onSelect calls this under the isolate borrow).
fn s2_topmenu_select(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let id = args.get(0).to_rust_string_lossy(scope);
        let slot = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        TOPMENU_PENDING.with(|q| q.borrow_mut().push((id, slot)));
    }));
}

/// Fan out queued TopMenu selects to each item's owner context. Called from ffi.rs AFTER
/// frame_async_drain() (HOST free). Mirrors dispatch_pending_cookie_cached / dispatch_concommand.
pub(crate) fn dispatch_pending_topmenu_select() {
    let pending: Vec<(String, i32)> = TOPMENU_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if pending.is_empty() { return; }
    for (id, slot) in pending {
        // snapshot (owner, gen, Global) — release TOPMENU_ITEMS borrow before entering a context.
        let entry = TOPMENU_ITEMS.with(|m| m.borrow().get(&id).map(|it| (it.owner.clone(), it.generation, it.on_select.clone())));
        let Some((owner, gen, global)) = entry else { continue };   // stale id -> no-op
        if !REGISTRY.with(|r| r.borrow().is_live(&owner, gen)) { continue; }
        let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.context.clone())) else { continue };
        HOST.with(|h| {
            let Ok(mut borrow) = h.try_borrow_mut() else { return };
            let Some(host) = borrow.as_mut() else { return };
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let slot_val: v8::Local<v8::Value> = v8::Number::new(tc, slot as f64).into();
            let func = v8::Local::new(tc, &global);
            if func.call(tc, recv, &[slot_val]).is_none() {
                let msg = tc.exception().map(|e| e.to_rust_string_lossy(&*tc)).unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_pending_topmenu_select('{}'): {}", id, msg));
            }
        });
    }
}
```

Register the natives (beside `__s2_concommand` at v8host.rs:3911):
```rust
    set_native(scope, global_obj, "__s2_topmenu_add_category", s2_topmenu_add_category);
    set_native(scope, global_obj, "__s2_topmenu_add_item", s2_topmenu_add_item);
    set_native(scope, global_obj, "__s2_topmenu_snapshot", s2_topmenu_snapshot);
    set_native(scope, global_obj, "__s2_topmenu_select", s2_topmenu_select);
```

Unload cleanup — in `unload_plugin`, beside the `dropped_cmds` `CONCOMMANDS` retain (v8host.rs:5688), drop the plugin's items:
```rust
    TOPMENU_ITEMS.with(|m| m.borrow_mut().retain(|_, it| it.owner != id));
```
Shutdown clear — beside `CONCOMMANDS.with(|m| m.borrow_mut().clear());` (v8host.rs:5354):
```rust
    TOPMENU_ITEMS.with(|m| m.borrow_mut().clear());
    TOPMENU_CATEGORIES.with(|c| c.borrow_mut().clear());
    TOPMENU_PENDING.with(|q| q.borrow_mut().clear());
```

- [ ] **Step 4: Add the prelude module**

In `INJECTED_STD_PRELUDE`, add beside `globalThis.__s2pkg_menu = ...`:
```javascript
  globalThis.__s2pkg_topmenu = { TopMenu: {
    addCategory: function (name) { __s2_topmenu_add_category(String(name)); },
    addItem: function (category, item) { __s2_topmenu_add_item(String(category), String(item.id), String(item.name), item.flags | 0, item.onSelect); },
    snapshot: function () { return __s2_topmenu_snapshot(); },
    select: function (id, slot) { __s2_topmenu_select(String(id), slot | 0); },
  } };
```

- [ ] **Step 5: Wire the post-drain dispatch in ffi.rs**

In `core/src/ffi.rs`, after `v8host::dispatch_pending_cookie_cached();` (ffi.rs:60), add:
```rust
            v8host::dispatch_pending_topmenu_select(); // Post, HOST free: fan out queued TopMenu.select
```

- [ ] **Step 6: Run the tests**

Run: `cd core && cargo test topmenu`
Expected: PASS (3 tests). Then `cd core && cargo test` (full suite green) and `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`.

- [ ] **Step 7: `.d.ts` + commit**

`packages/topmenu/package.json`:
```json
{ "name": "@s2script/topmenu", "version": "0.1.0", "types": "index.d.ts" }
```
`packages/topmenu/index.d.ts`:
```typescript
/** @s2script/topmenu — the extensible admin/top menu registry. NO runtime code (injected at load). */
export interface TopMenuItem {
  /** Plugin-namespaced unique id, e.g. "playercommands:slap". A duplicate id replaces. */
  id: string;
  /** Shown label. */
  name: string;
  /** Required ADMFLAG bit mask (adminmenu hides items the caller lacks flags for). */
  flags: number;
  /** Runs in THIS plugin's context when an admin selects the item; receives the admin's 0-based slot. */
  onSelect: (adminSlot: number) => void;
}
export interface TopMenuSnapshot {
  categories: string[];
  items: { id: string; category: string; name: string; flags: number }[];
}
export declare const TopMenu: {
  /** Register a category (idempotent; order = first-registration order). */
  addCategory(name: string): void;
  /** Register/replace an item under a category (auto-creates the category). */
  addItem(category: string, item: TopMenuItem): void;
  /** All categories + item metadata (no handlers) — the adminmenu renderer reads this. */
  snapshot(): TopMenuSnapshot;
  /** Fire an item's onSelect (dispatched post-frame to the owner's context). */
  select(id: string, slot: number): void;
};
```

```bash
git add core/src/v8host.rs core/src/ffi.rs packages/topmenu/package.json packages/topmenu/index.d.ts
git commit -m "feat(topmenu): core TopMenu registry + @s2script/topmenu (owner-dispatch, post-drain select)"
```

---

### Task 2: `pickPlayer` helper (`@s2script/cs2`)

A reusable target-picker menu the item handlers use.

**Files:**
- Modify: `games/cs2/js/pawn.js` — add `pickPlayer` + export into `__s2pkg_cs2`.
- Modify: `packages/cs2/index.d.ts` — the `pickPlayer` signature.

**Interfaces:**
- Consumes: `globalThis.__s2pkg_menu.Menu`/`MenuStyle`; `Player.allConnected()`, `Player.fromUserId`.
- Produces: `__s2pkg_cs2.pickPlayer(adminSlot, onPicked)` — shows a Center menu of connected players; on pick, resolves the target and calls `onPicked(targetPlayer)`.

- [ ] **Step 1: Implement `pickPlayer`**

In `pawn.js`, before the `globalThis.__s2pkg_cs2 = { ... }` assembly, add:
```javascript
  // pickPlayer(adminSlot, onPicked): a target-picker Center menu over connected players. The item info
  // is the userid (stable across the pick), re-resolved on select so a player who left -> a graceful skip.
  function pickPlayer(adminSlot, onPicked) {
    var Menu = globalThis.__s2pkg_menu.Menu, MenuStyle = globalThis.__s2pkg_menu.MenuStyle;
    var m = new Menu("Select a player");
    m.style = MenuStyle.Center;
    m.freezePlayer = true;
    var players = Player.allConnected();
    for (var i = 0; i < players.length; i++) {
      var p = players[i];
      m.addItem(String(p.userId), (p.playerName || ("slot " + p.slot)));
    }
    m.onSelect(function (e) {
      var target = Player.fromUserId(parseInt(e.info, 10));
      if (!target) { globalThis.__s2pkg_chat.Chat.toSlot(adminSlot, "Player no longer available"); return; }
      onPicked(target);
    });
    m.display(adminSlot, 30);
  }
```
Add to the `__s2pkg_cs2` export object: `pickPlayer: pickPlayer`.

- [ ] **Step 2: `.d.ts`**

Add to `packages/cs2/index.d.ts` (module exports):
```typescript
/** Show a target-picker menu of connected players to `adminSlot`; calls `onPicked` with the chosen Player. */
export declare function pickPlayer(adminSlot: number, onPicked: (target: Player) => void): void;
```

- [ ] **Step 3: Regenerate + gates + commit**

Run: `bash scripts/package-addon.sh` then `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`.
```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts
git commit -m "feat(cs2): pickPlayer target-picker menu helper"
```

---

### Task 3: The `adminmenu` plugin

**Files:**
- Create: `plugins/adminmenu/package.json`, `plugins/adminmenu/tsconfig.json`, `plugins/adminmenu/src/plugin.ts` (mirror `plugins/basecommands/` structure).

**Interfaces:**
- Consumes: `@s2script/topmenu` (`TopMenu`), `@s2script/menu` (`Menu`, `MenuStyle`), `@s2script/commands` (`Commands`), `@s2script/admin` (`Admin`).
- Produces: `sm_admin`; the standard categories registered in order.

- [ ] **Step 1: Write the plugin**

`plugins/adminmenu/src/plugin.ts`:
```typescript
import { TopMenu } from "@s2script/topmenu";
import { Menu, MenuStyle } from "@s2script/menu";
import { Commands } from "@s2script/commands";
import { Admin } from "@s2script/admin";

// Fix the standard category order (items land in these; a plugin may add more).
TopMenu.addCategory("Player Commands");
TopMenu.addCategory("Server Commands");
TopMenu.addCategory("Voting Commands");

function itemsFor(category: string, flags: number) {
  return TopMenu.snapshot().items.filter(i => i.category === category && ((flags & (1 << 14)) !== 0 || (flags & i.flags) === i.flags));
}

function showCategory(slot: number, category: string, flags: number): void {
  const items = itemsFor(category, flags);
  const m = new Menu(category);
  m.style = MenuStyle.Center;
  for (const it of items) m.addItem(it.id, it.name);
  m.onSelect(e => { TopMenu.select(e.info, slot); });
  m.display(slot, 30);
}

Commands.register("sm_admin", ctx => {
  const slot = ctx.callerSlot;
  if (slot < 0) { ctx.reply("Run sm_admin in-game."); return; }
  const admin = Admin.forSlot(slot);
  if (!admin) { ctx.reply("No access."); return; }
  const snap = TopMenu.snapshot();
  // Only categories with >=1 visible item.
  const cats = snap.categories.filter(c => itemsFor(c, admin.flags).length > 0);
  if (cats.length === 0) { ctx.reply("No admin actions available."); return; }
  const m = new Menu("Admin Menu");
  m.style = MenuStyle.Center;
  for (const c of cats) m.addItem(c, c);
  m.onSelect(e => { showCategory(slot, e.info, admin.flags); });
  m.display(slot, 30);
});

console.log("[adminmenu] onLoad — sm_admin registered");
```
(`admin.flags` is the caller's ADMFLAG mask from `Admin.forSlot`; `1 << 14` is `ADMFLAG.ROOT`. `hasFlags`-equivalent inlined so the filter matches SM: ROOT sees all, else `(admin & item) === item`.)

`plugins/adminmenu/package.json` + `tsconfig.json`: mirror `plugins/basecommands/` (copy the structure, set `name` to `@s2script/adminmenu`, id `adminmenu`).

- [ ] **Step 2: Build (typecheck) + commit**

Run: `node packages/cli/dist/cli.js build plugins/adminmenu` (expect a clean `.s2sp`).
```bash
git add plugins/adminmenu
git commit -m "feat(adminmenu): sm_admin — flag-filtered TopMenu render via @s2script/menu"
```

---

### Task 4: Register the proof item set in the command plugins

Add items with `onSelect` handlers using `pickPlayer`. Each item's `flags` = the same `ADMFLAG` as its text command.

**Files:**
- Modify: `plugins/playercommands/src/plugin.ts` (Slap, Slay), `plugins/basebans/src/plugin.ts` (Kick, Ban), `plugins/basecomm/src/plugin.ts` (Gag), `plugins/basecommands/src/plugin.ts` (Change Map).

**Interfaces:**
- Consumes: `@s2script/topmenu` (`TopMenu`), `@s2script/cs2` (`pickPlayer`, `Player`), `@s2script/menu` (`Menu`, `MenuStyle`), `@s2script/admin` (`ADMFLAG`), plus each plugin's existing primitives (`pawn.health`/`pawn.slay`, `player.kick`, `Bans.add`, the gag path, `Server.command`/`Server.isMapValid`).

- [ ] **Step 1: playercommands — Slap + Slay**

Add to `plugins/playercommands/src/plugin.ts` (after the command registrations), importing `TopMenu` + `pickPlayer`:
```typescript
TopMenu.addItem("Player Commands", { id: "playercommands:slap", name: "Slap", flags: ADMFLAG.SLAY,
  onSelect: adminSlot => pickPlayer(adminSlot, t => { const p = t.pawn; if (p) p.health = Math.max(1, (p.health ?? 1) - 5); }) });
TopMenu.addItem("Player Commands", { id: "playercommands:slay", name: "Slay", flags: ADMFLAG.SLAY,
  onSelect: adminSlot => pickPlayer(adminSlot, t => { const p = t.pawn; if (p) p.slay(); }) });
```

- [ ] **Step 2: basebans — Kick + Ban (→ duration menu)**

Add to `plugins/basebans/src/plugin.ts`, importing `TopMenu`, `pickPlayer`, `Menu`, `MenuStyle` (and the existing `Bans`):
```typescript
TopMenu.addItem("Player Commands", { id: "basebans:kick", name: "Kick", flags: ADMFLAG.KICK,
  onSelect: adminSlot => pickPlayer(adminSlot, t => t.kick("Kicked by admin")) });
TopMenu.addItem("Player Commands", { id: "basebans:ban", name: "Ban", flags: ADMFLAG.BAN,
  onSelect: adminSlot => pickPlayer(adminSlot, t => {
    const steamId = t.steamId, name = t.playerName || "player";
    const dm = new Menu("Ban " + name + " for");
    dm.style = MenuStyle.Center;
    const mins = [0, 5, 30, 60];   // 0 = permanent
    for (const m of mins) dm.addItem(String(m), m === 0 ? "Permanent" : (m + " min"));
    dm.onSelect(e => { const m = parseInt(e.info, 10); Bans.add(steamId, m, "Banned by admin"); t.kick("Banned"); });
    dm.display(adminSlot, 30);
  }) });
```
(If `t.pawn` is needed for kick vs. `t.kick`, use the existing `player.kick` — a `Player` method. Use the plugin's actual `Bans.add` signature.)

- [ ] **Step 3: basecomm — Gag**

Add to `plugins/basecomm/src/plugin.ts`, importing `TopMenu` + `pickPlayer`:
```typescript
TopMenu.addItem("Player Commands", { id: "basecomm:gag", name: "Gag", flags: ADMFLAG.CHAT,
  onSelect: adminSlot => pickPlayer(adminSlot, t => gagPlayer(t)) });
```
where `gagPlayer(t)` calls the plugin's existing gag routine (the SteamID-keyed gag the `sm_gag` handler uses — factor it into a shared function if it isn't one).

- [ ] **Step 4: basecommands — Change Map (→ map picker)**

Add to `plugins/basecommands/src/plugin.ts`, importing `TopMenu`, `Menu`, `MenuStyle`, `Server`:
```typescript
const MAP_CHOICES = ["de_dust2", "de_inferno", "de_mirage", "de_nuke", "de_ancient", "de_anubis"];
TopMenu.addItem("Server Commands", { id: "basecommands:map", name: "Change Map", flags: ADMFLAG.CHANGEMAP,
  onSelect: adminSlot => {
    const m = new Menu("Change Map");
    m.style = MenuStyle.Center;
    for (const map of MAP_CHOICES) if (Server.isMapValid(map)) m.addItem(map, map);
    m.onSelect(e => { Server.command("changelevel " + e.info); });
    m.display(adminSlot, 30);
  } });
```

- [ ] **Step 5: Build all four + commit**

Run: `node packages/cli/dist/cli.js build plugins/playercommands && node packages/cli/dist/cli.js build plugins/basebans && node packages/cli/dist/cli.js build plugins/basecomm && node packages/cli/dist/cli.js build plugins/basecommands` (each a clean `.s2sp`; fix any `.d.ts` mismatch).
```bash
git add plugins/playercommands plugins/basebans plugins/basecomm plugins/basecommands
git commit -m "feat(adminmenu): register Kick/Ban/Slap/Slay/Gag + Change Map items"
```

---

### Task 5: Sniper build + live gate

**Files:** none (build + deploy + gate).

- [ ] **Step 1: Sniper build (Task 1 core)**

Run:
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expect exit 0, no `error:`, GLIBC `libs2script_core` ≤ 2.30 / `s2script.so` ≤ 2.14.

- [ ] **Step 2: Deploy (plugins/ only — NOT examples)**

```bash
mkdir -p dist/addons/s2script/plugins dist/addons/s2script/configs dist/addons/s2script/data
find plugins -path '*/dist/*.s2sp' -exec cp {} dist/addons/s2script/plugins/ \;
docker compose -f docker/docker-compose.yml restart cs2
```

- [ ] **Step 3: Live gate (human client)**

Poll `docker logs s2script-cs2 --since 3m` for `GAMEDATA VALIDATION: 12 ok` + `[adminmenu] onLoad`. Then, with the user joined: `sm_admin` → the category menu shows (flag-filtered); **Player Commands → Slap → pick a bot → its hp drops**; **Kick → pick a bot → it disconnects**; **Ban → pick → duration → the bot is kicked + `bans.json` gains the entry**; **Server Commands → Change Map → pick → the map changes**. Confirm a non-admin is denied and `RestartCount=0`, no crash.

- [ ] **Step 4: Commit any live-gate fixes**

Commit fixes as `fix(adminmenu): <what> (live gate)` with the session trailer.

---

## Self-Review

**Spec coverage:**
- Core TopMenu registry (categories + owner-tracked items, dispatch-to-owner) → Task 1 ✅
- `@s2script/topmenu` module + `.d.ts` → Task 1 ✅
- Post-drain select (the re-entrancy fix) → Task 1 (Steps 3/5) ✅
- `adminmenu` plugin (sm_admin, flag-filter, render) → Task 3 ✅
- `pickPlayer` helper → Task 2 ✅
- Proof item set (Kick/Ban/Slap/Slay/Gag + Change Map) → Task 4 ✅
- Owner-scoped teardown + shutdown → Task 1 (Step 3) ✅
- Live gate (human client) → Task 5 ✅
- Boundary gates + one sniper → Tasks 1–4 run gates, Task 5 snipers ✅
- Deferred (long-tail items, Voting, immunity, item-order) → not built ✅

**Placeholder scan:** The item handlers reference each plugin's existing primitive (`pawn.slay`, `player.kick`, `Bans.add`, the gag routine, `Server.command`); the implementer wires the exact existing call (named per plugin). Task 4 Steps 2/3 note "use the plugin's actual `Bans.add` signature / factor the gag routine" — concrete instructions, not vague TODOs. No "add error handling"/"etc." placeholders.

**Type consistency:** `TopMenu.addItem(category, {id, name, flags, onSelect})` and `snapshot()`/`select(id, slot)` signatures match between Task 1 (native + module + `.d.ts`) and Tasks 3/4 (callers). `pickPlayer(adminSlot, onPicked)` matches between Task 2 (`.d.ts`) and Task 4. `onSelect(adminSlot: number)` arity (one arg) matches the native dispatch (`func.call(tc, recv, &[slot_val])`). Category names are consistent strings (`"Player Commands"`/`"Server Commands"`) across Task 3 (predefined) and Task 4 (item registration).

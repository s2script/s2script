# L1 Lifecycle v2 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the plugin a typed artifact (`export default plugin(factory)`) loaded through a formal phase machine (`Loading → Active → Unloading` + `Failed`) with all registration on a load-scoped `ctx`, one unified teardown walk, dependency-ordered activation — and port the entire base-plugin suite + examples.

**Architecture:** The core (`core/src/v8host.rs` + `loader.rs` + `plugin.rs`) gains an awaited-factory load path, arm-at-Active subscription buffering (pure prelude JS — thunks replayed at Active through the existing subscribe natives, which stay permissive for SDK-internal code), a self-registered `OWNER_SCOPED_STORES` teardown registry replacing the hand-maintained ~16-store cascade, and `Scope` disposal via per-subscription ids. The SDK ships a new `@s2script/sdk/plugin` subpath (`PluginContext`/`plugin()`/`Scope`); old registration verbs are `@deprecated` during the port fan-out and deleted in the final task so every intermediate commit keeps the gate green.

**Tech Stack:** Rust (core, V8 149 via the pinned-scope API idioms already used throughout `v8host.rs`), the injected JS prelude (`INJECTED_STD_PRELUDE`), TypeScript `.d.ts` (packages/sdk), bash gate scripts.

**Authoritative spec:** `docs/superpowers/specs/2026-07-20-L1-lifecycle-v2-design.md` (this plan implements it 1:1; the north star `2026-07-20-safety-by-construction-north-star-design.md` §4/§8 is the parent).

## Global Constraints

- Work in this worktree on branch `rearch/north-star`; one commit per task (`git add` + `git commit`), NO push / NO `gt submit` (held for the human).
- **Gate suite per task** (run what the task touches; the full suite at tasks 4, 29):
  - `cargo test -p s2script-core` — single-threaded via `.cargo/config.toml`; **never pass `--test-threads`**.
  - `make check-boundary` — core must NOT import `games/*`.
  - `./scripts/check-plugins-typecheck.sh` — the 5E.1 gate (every plugin + example).
  - `./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh && ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh && ./scripts/test-boundary-nameleak.sh` (unaffected by L1 — run at 4/29 to prove it).
  - Plugin builds: `cd plugins/<name> && npx s2script build` (or `./scripts/build-base-plugins.sh` for all).
- Core stays engine-generic: no game identifiers in `core/` or `INJECTED_STD_PRELUDE`.
- Plugins are pure ESM (no `require`); naming: PascalCase types, camelCase functions/properties. Command-invocation parameter is named **`cmd`** everywhere (locked).
- `HOST_API_VERSION_MAJOR` becomes **2** (Task 4); every plugin/example manifest becomes `"apiVersion": "2.x"` (ports).
- Cross-context payloads are JSON — `BigInt` throws and drops the payload (unchanged; carry 64-bit as decimal strings).
- Do NOT touch: EntityRef shape (E1's job), `s2s build` bundling/manifest derivation (B1), ESLint (B2), the 13 pre-existing CLI test failures (schema-runtime + player-identity — known, unrelated).
- Docs: append nothing to CLAUDE.md's Current state; PROGRESS.md entry only after the (held) live gate.

## Parallelization map

```
SPINE (sequential — same core files):
  T1 owner-stores registry ─► T2 phase machine + typed artifact ─► T3 scopes + sub-ids + onDamage
     ─► T4 loader v2 (topo/waiting/timeout/apiVersion-2)

SDK (parallel with T3/T4 — different directory):
  T5 plugin.d.ts + deprecations (after T2)     T6 tsconfig-shared + s2s create template (after T5)

CONTRACT FREEZE = T1–T6 all merged. Then:

FAN-OUT (all 22 tasks fully parallel; each consumes ONLY the frozen contract):
  T7 basecommands   T8 basechat        T9 playercommands  T10 antiflood
  T11 adminhelp     T12 basecomm       T13 basebans       T14 reservedslots
  T15 basetriggers  T16 funcommands    T17 clientprefs    T18 adminmenu
  T19 basevotes     T20 zones          T21 nominations    T22 rockthevote
  T23 funvotes      T24 nextmap
  T25 examples batch A   T26 examples batch B   T27 examples batch C   T28 examples batch D

FINAL (sequential, after ALL fan-out tasks):
  T29 legacy-surface removal + fixtures + full gate ─► T30 docs + live-gate checklist (gate itself HELD for human)
```

Every port task is independent of every other port task (different directories). When dispatching a fan-out task to a worker, include §"Port Kit" below verbatim in its prompt in addition to the task body.

---

### Task 1: `OWNER_SCOPED_STORES` — self-registered teardown + `EventMux` subscription ids

**Files:**
- Create: `core/src/owner_stores.rs`
- Modify: `core/src/lib.rs` (module decl), `core/src/event_mux.rs`, `core/src/v8host.rs` (`unload_plugin` `:9855-9958`, `init` `:9199`, `NEXT_SUB_ID` `:588`)
- Test: inline `#[cfg(test)]` in `owner_stores.rs` + existing `cargo test -p s2script-core` suite (must stay green — this task is behavior-preserving)

**Interfaces:**
- Consumes: the existing per-store teardown block at `v8host.rs:9860-9958` (each store's `remove_by_owner` + its follow-up engine-op), `EventMux` (`core/src/event_mux.rs`), `NEXT_SUB_ID` thread-local (`v8host.rs:588`).
- Produces (relied on by T2/T3):
  - `owner_stores::register(name: &'static str, remove_by_owner: Box<dyn Fn(&str)>, remove_by_ids: Box<dyn Fn(&[u64])>)`
  - `owner_stores::sweep_owner(owner: &str)` — runs every store's `remove_by_owner` in registration order
  - `owner_stores::sweep_ids(ids: &[u64])` — runs every store's `remove_by_ids`
  - `EventMux::subscribe(...) -> (bool /*first*/, u64 /*sub id*/)`; `EventMux::remove_by_ids(&[u64]) -> Vec<String> /*emptied names*/`
  - `v8host::next_sub_id() -> u64` (public-in-crate wrapper over `NEXT_SUB_ID`)

- [ ] **Step 1: Write `core/src/owner_stores.rs` with tests first**

```rust
//! Self-registration list of every owner-scoped subscription store (design spec §6).
//! unload_plugin sweeps THIS registry instead of a hand-maintained cascade; a new
//! capability slice registers its store next to the store's definition.
use std::cell::RefCell;

pub struct OwnerScopedStore {
    pub name: &'static str,
    pub remove_by_owner: Box<dyn Fn(&str)>,
    pub remove_by_ids: Box<dyn Fn(&[u64])>,
}

thread_local! {
    static STORES: RefCell<Vec<OwnerScopedStore>> = RefCell::new(Vec::new());
}

pub fn register(name: &'static str, remove_by_owner: Box<dyn Fn(&str)>, remove_by_ids: Box<dyn Fn(&[u64])>) {
    STORES.with(|s| s.borrow_mut().push(OwnerScopedStore { name, remove_by_owner, remove_by_ids }));
}

/// Idempotent re-registration guard for re-init paths (Metamod reload): clears the list.
pub fn reset() { STORES.with(|s| s.borrow_mut().clear()); }

pub fn sweep_owner(owner: &str) {
    // Collect closures' indices first? No: closures may re-enter register? They must not — they only
    // touch their own store thread-locals. A plain indexed loop over a snapshot of len avoids holding
    // the borrow across a closure that could (defensively) re-enter.
    let len = STORES.with(|s| s.borrow().len());
    for i in 0..len {
        STORES.with(|s| { if let Some(st) = s.borrow().get(i) { (st.remove_by_owner)(owner); } });
    }
}

pub fn sweep_ids(ids: &[u64]) {
    if ids.is_empty() { return; }
    let len = STORES.with(|s| s.borrow().len());
    for i in 0..len {
        STORES.with(|s| { if let Some(st) = s.borrow().get(i) { (st.remove_by_ids)(ids); } });
    }
}
```

Wait — `sweep_owner` calls the closure while `STORES` is borrowed (the `if let` holds the borrow). Fix: clone nothing; take the closure out temporarily is impossible (`Box<dyn Fn>` not Clone). Correct pattern — run WITHOUT holding the borrow by using raw pointer indexing is unsafe; instead document + enforce the invariant that store closures never call `register`/`sweep_*` (true for all builtin stores: they only touch their own mux thread-locals), and keep the borrow. Use this final body:

```rust
pub fn sweep_owner(owner: &str) {
    STORES.with(|s| {
        let stores = s.borrow();
        for st in stores.iter() { (st.remove_by_owner)(owner); }
    });
}
pub fn sweep_ids(ids: &[u64]) {
    if ids.is_empty() { return; }
    STORES.with(|s| {
        let stores = s.borrow();
        for st in stores.iter() { (st.remove_by_ids)(ids); }
    });
}
```

Add tests in the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    thread_local! { static HITS: RefCell<Vec<String>> = RefCell::new(Vec::new()); }

    #[test]
    fn sweep_owner_runs_every_store_in_registration_order() {
        reset();
        register("a", Box::new(|o| HITS.with(|h| h.borrow_mut().push(format!("a:{o}")))), Box::new(|_| {}));
        register("b", Box::new(|o| HITS.with(|h| h.borrow_mut().push(format!("b:{o}")))), Box::new(|_| {}));
        sweep_owner("p1");
        HITS.with(|h| assert_eq!(*h.borrow(), vec!["a:p1".to_string(), "b:p1".to_string()]));
    }

    #[test]
    fn sweep_ids_skips_empty_and_hits_all_stores() {
        reset();
        HITS.with(|h| h.borrow_mut().clear());
        register("a", Box::new(|_| {}), Box::new(|ids| HITS.with(|h| h.borrow_mut().push(format!("a:{}", ids.len())))));
        sweep_ids(&[]);
        HITS.with(|h| assert!(h.borrow().is_empty(), "empty ids = no-op"));
        sweep_ids(&[1, 2]);
        HITS.with(|h| assert_eq!(*h.borrow(), vec!["a:2".to_string()]));
    }
}
```

- [ ] **Step 2: Run the new tests — expect FAIL (module not declared), then declare `pub mod owner_stores;` in `core/src/lib.rs` (alongside `pub mod plugin;` etc.) and re-run**

Run: `cargo test -p s2script-core owner_stores`
Expected: 2 passed.

- [ ] **Step 3: Give `EventMux` per-row ids + `remove_by_ids`**

In `core/src/event_mux.rs` (signatures below are the contract — keep the file's existing style/tests):

```rust
pub struct EventSub<H> { pub id: u64, pub owner: String, pub generation: u64, pub handler: H }

impl<H: Clone> EventMux<H> {
    /// Returns (first_for_name, sub_id). `id` comes from the caller (v8host::next_sub_id()).
    pub fn subscribe_with_id(&mut self, name: &str, id: u64, owner: String, generation: u64, handler: H) -> bool {
        let list = self.by_name.entry(name.to_string()).or_default();
        let first = list.is_empty();
        list.push(EventSub { id, owner, generation, handler });
        first
    }
    /// Remove rows whose id is in `ids`. Returns names that became empty (same contract as remove_by_owner).
    pub fn remove_by_ids(&mut self, ids: &[u64]) -> Vec<String> {
        let mut emptied = Vec::new();
        for (name, list) in self.by_name.iter_mut() {
            let before = list.len();
            list.retain(|s| !ids.contains(&s.id));
            if before > 0 && list.is_empty() { emptied.push(name.clone()); }
        }
        emptied
    }
}
```

Keep the existing `subscribe` as a thin wrapper `subscribe(&mut self, name, owner, generation, handler) -> bool { self.subscribe_with_id(name, 0, ...) }`? **No** — instead change `subscribe`'s signature to take `id: u64` directly and update every caller in `v8host.rs` to pass `next_sub_id()` (grep `\.subscribe(` on `EVENT_MUX|EVENT_MUX_PRE|DAMAGE_MUX|CHAT_MSG_SUBS|CLIENT_MUX|MAP_MUX|PRECACHE_MUX|COOKIE_CACHED_MUX|WS_EVENT_MUX|NET_EVENT_MUX|OUTPUT_MUX|ENTITY_MUX|USERCMD_MUX|USERMSG_MUX` — mechanical). `snapshot` keeps its `(owner, generation, handler)` tuple shape so NO dispatch site changes. Add `next_sub_id()` in `v8host.rs`:

```rust
pub(crate) fn next_sub_id() -> u64 {
    NEXT_SUB_ID.with(|c| { let v = c.get(); c.set(v + 1); v })
}
```

(If `NEXT_SUB_ID` already has an allocator helper, reuse it — do not create a second counter.)

Add an `event_mux.rs` test:

```rust
#[test]
fn remove_by_ids_removes_only_matching_rows_and_reports_emptied() {
    let mut m: EventMux<&'static str> = EventMux::new();
    m.subscribe_with_id("e1", 10, "p".into(), 1, "h1");
    m.subscribe_with_id("e1", 11, "p".into(), 1, "h2");
    m.subscribe_with_id("e2", 12, "p".into(), 1, "h3");
    assert!(m.remove_by_ids(&[10]).is_empty());
    assert_eq!(m.snapshot("e1").len(), 1);
    let emptied = m.remove_by_ids(&[11, 12]);
    assert!(emptied.contains(&"e1".to_string()) && emptied.contains(&"e2".to_string()));
}
```

- [ ] **Step 4: Register the builtin stores and collapse `unload_plugin`'s cascade**

Add to `v8host.rs` a `pub(crate) fn register_builtin_stores()` called at the END of `init()` (`:9253`, after `HOST` is set; also call `owner_stores::reset()` first — re-init safe). Move each block of `unload_plugin:9860-9958` verbatim into a store closure, e.g.:

```rust
pub(crate) fn register_builtin_stores() {
    crate::owner_stores::reset();
    // FRAME (OnGameFrame) — remove_by_owner + detour reconcile; ids handled via Descriptor::unsubscribe.
    crate::owner_stores::register("FRAME",
        Box::new(|owner| { let _ = FRAME.with(|f| f.borrow_mut().remove_by_owner(owner)); refresh_detour(); }),
        Box::new(|_ids| { /* frame subs dispose via their {dispose} closure — see Scope (T3) */ }));
    // EVENT_MUX — emptied names → engine-op event_unsubscribe.
    crate::owner_stores::register("EVENT_MUX",
        Box::new(|owner| {
            let emptied = EVENT_MUX.with(|m| m.borrow_mut().remove_by_owner(owner));
            unsubscribe_emptied_events(&emptied);
        }),
        Box::new(|ids| {
            let emptied = EVENT_MUX.with(|m| m.borrow_mut().remove_by_ids(ids));
            unsubscribe_emptied_events(&emptied);
        }));
    // EVENT_MUX_PRE — whole-mux-empty → remove the global GameEvent hook.
    crate::owner_stores::register("EVENT_MUX_PRE",
        Box::new(|owner| { EVENT_MUX_PRE.with(|m| m.borrow_mut().remove_by_owner(owner)); reconcile_pre_hook(); }),
        Box::new(|ids| { EVENT_MUX_PRE.with(|m| m.borrow_mut().remove_by_ids(ids)); reconcile_pre_hook(); }));
    // ... one register() per remaining store, each carrying its existing follow-up verbatim:
    // DAMAGE_MUX, CHAT_MSG_SUBS, CLIENT_MUX, MAP_MUX, PRECACHE_MUX, COOKIE_CACHED_MUX,
    // WS_EVENT_MUX, NET_EVENT_MUX, OUTPUT_MUX, ENTITY_MUX, USERCMD_MUX,
    // USERMSG_MUX (emptied → USERMSG_IDS.remove + ops.usermsg_hook_unsub),
    // transmit (transmit_remove_owner(owner); ids: no-op),
    // CONFIG_SUBS (+ loader::unwatch_config_for(owner); ids: CONFIG_SUBS remove_by_ids only),
    // CONCOMMANDS+COMMAND_META (the retain + meta-drop block; ids: no-op — commands are not scope-able),
    // TOPMENU_ITEMS (retain owner != id; ids: no-op).
}

fn unsubscribe_emptied_events(emptied: &[String]) {
    for evname in emptied {
        if let Some(ops) = ENGINE_OPS.with(|o| o.get()) {
            if let Some(func) = ops.event_unsubscribe {
                if let Ok(cn) = CString::new(evname.as_str()) { func(cn.as_ptr()); }
            }
        }
    }
}
fn reconcile_pre_hook() {
    if EVENT_MUX_PRE.with(|m| m.borrow().is_empty()) {
        if let Some(req) = HOOK_REQUEST.with(|r| r.get()) {
            if let Ok(d) = CString::new("GameEvent") { req(d.as_ptr(), 0); }
        }
    }
}
```

Then replace `unload_plugin`'s `(a)`–`(a2e)` block (`:9857-9958`) with exactly:

```rust
    crate::owner_stores::sweep_owner(id);
    refresh_detour();
```

**Important:** the in-isolate tests construct hosts via `init()` in tests — verify `register_builtin_stores()` is reachable in tests (call it from `init()` itself, last line before `Ok(())`, so every path gets it).

- [ ] **Step 5: Full core test run + boundary**

Run: `cargo test -p s2script-core` → all pass (this task changes no observable behavior).
Run: `make check-boundary` → PASS.

- [ ] **Step 6: Commit**

```bash
git add core/src/owner_stores.rs core/src/lib.rs core/src/event_mux.rs core/src/v8host.rs
git commit -m "core: OWNER_SCOPED_STORES self-registration replaces the hand-maintained unload cascade; EventMux rows get sub ids"
```

---

### Task 2: Phase machine + the typed artifact load path (awaited factory, arm-at-Active, `Failed`, handoff)

**Files:**
- Modify: `core/src/plugin.rs` (add `Phase`), `core/src/v8host.rs` (`PluginInstance` `:457`, `INJECTED_STD_PRELUDE` `:927`, `install_natives` `:7431`, `load_plugin_js` `:9079` → `start_plugin_load`, `unload_plugin` `:9855`, `frame_async_drain` tail `:9827`, in-isolate tests), `core/src/loader.rs` (`load_and_reconcile` `:84`)
- Test: in-isolate tests in `v8host.rs` `#[cfg(test)]` (rewrites + new)

**Interfaces:**
- Consumes: T1's `owner_stores::sweep_owner`; existing `PENDING_HANDOFF` (`v8host.rs:732`), `iface_to_json`/`iface_from_json`, `reconcile_publishes` (`v8host.rs:4471`), `eval_in_context` (`:7880`), `current_plugin` (`:7729`), `create_plugin_context` (`:7800`).
- Produces (relied on by T3–T6 and every port):
  - Artifact contract: bundle default export `{ __s2plugin: 1, factory: (ctx) => void|hooks|Promise }`; hooks = `{ onUnload?: () => void, state?: () => unknown }`.
  - `Phase { Loading, Active, Unloading, Failed }` in `core/src/plugin.rs`; `PluginInstance.phase`.
  - Natives: `__s2_load_settled(hooks?)`, `__s2_load_failed(message)`, `__s2_handoff_take() -> unknown`.
  - Prelude: `__s2_make_ctx()`, `globalThis.__s2_run_factory(def)`, per-context `__s2_ctx_arm()` / `__s2_ctx_seal()`; module global `__s2pkg_plugin = { plugin: (f) => ({ __s2plugin: 1, factory: f }) }` (resolves as `@s2script/sdk/plugin` via the existing strip at `v8host.rs:4332`).
  - `v8host::start_plugin_load(id, js, cfg)` (replaces `load_plugin_js`'s external role; keep `load_plugin_js` name as the public entry to minimize loader churn — it now STARTS a load), `v8host::finalize_loading_plugins()` (called at the tail of `frame_async_drain` and inline for the sync fast-path), `v8host::plugin_phase(id) -> Option<Phase>`, `LOAD_TIMEOUT_FRAMES: u64 = 1920`.
  - Test helper: `fn def_js(factory_body: &str) -> String` producing a new-shape bundle.

- [ ] **Step 1: `Phase` in `core/src/plugin.rs` + failing unit test**

```rust
/// The plugin lifecycle phase (design spec §5). Stored on the v8host PluginInstance;
/// pure data here (no V8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase { Loading, Active, Unloading, Failed }
```

Test (plugin.rs): `#[test] fn phase_is_copy_eq() { assert_eq!(Phase::Loading, Phase::Loading); assert_ne!(Phase::Active, Phase::Failed); }` (trivial — the real behavior tests are in-isolate below).

- [ ] **Step 2: Extend `PluginInstance` and add host load-state**

In `v8host.rs`:

```rust
struct PluginInstance {
    exports: Option<v8::Global<v8::Object>>,   // now stores the settled PluginHooks object (renamed usage; keep field name `exports` to minimize churn, update doc comment)
    context: v8::Global<v8::Context>,
    generation: u64,
    config_decls: std::collections::HashMap<String, crate::config::ConfigEntry>,
    phase: crate::plugin::Phase,               // NEW — starts Loading in create_plugin_context
}
```

Thread-locals (next to `PENDING_HANDOFF` `:732`):

```rust
/// In-flight factory loads: id → (frame the load started, settle state, queued-reload flag).
static LOADING: std::cell::RefCell<std::collections::HashMap<String, LoadingEntry>> = ...;
/// Failed plugins (context already torn down) — reason kept for `sm plugins list` (spec §5/§8).
static FAILED_PLUGINS: std::cell::RefCell<std::collections::HashMap<String, String>> = ...;
```

```rust
enum SettleState { InFlight, Settled, Failed(String) }
struct LoadingEntry { started_frame: u64, state: SettleState, pending_reload: bool }
pub(crate) const LOAD_TIMEOUT_FRAMES: u64 = 1920; // ~30s at 64Hz (spec §5.2, open question #1)
```

`create_plugin_context` sets `phase: Phase::Loading` and clears any `FAILED_PLUGINS` entry for the id.

- [ ] **Step 3: The ctx prelude.** Append to `INJECTED_STD_PRELUDE` (engine-generic; no game identifiers). Real code — this is the heart of arm-at-Active:

```js
  // --- L1 lifecycle v2: the plugin() artifact + load-scoped ctx (design spec §1/§3/§4) ---
  globalThis.__s2pkg_plugin = { plugin: function (factory) { return { __s2plugin: 1, factory: factory }; } };

  function __s2_make_ctx() {
    var pending = [];      // registration thunks, replayed at arm (Active)
    var armed = false;
    var sealed = false;
    var scopes = [];
    function reg(thunk) {
      if (sealed) throw new Error("s2script: registration outside the load window - use a Scope from ctx.createScope()");
      if (armed) { thunk(); } else { pending.push(thunk); }   // armed=true only ever inside scopes (see makeSubjects)
    }
    // Build one subjects bundle; `track` is null for ctx (plugin-lifetime) or the scope's tracker.
    function makeSubjects(track) {
      var t = track || { ids: function () {}, disposer: function (d) {} };
      function viaId(call) { return function () { var id = call.apply(null, arguments); if (typeof id === "number") t.ids(id); }; }
      var Ev = __s2pkg_events.Events, Cl = __s2pkg_clients.Clients, En = __s2pkg_entity.Entity;
      var Sv = __s2pkg_server.Server, Fr = __s2pkg_frame.OnGameFrame, Ck = __s2pkg_cookies.Cookies;
      var Uc = __s2pkg_usercmd.UserCmd, Dm = __s2pkg_damage.Damage, Ch = __s2pkg_chat.Chat, Sn = __s2pkg_sound.Sound;
      return {
        events: {
          on:    function (n, h) { reg(viaId(function () { return Ev.on(n, h); })); },
          onPre: function (n, h) { reg(viaId(function () { return Ev.onPre(n, h); })); },
        },
        clients: {
          onConnect:         function (h) { reg(viaId(function () { return Cl.onConnect(h); })); },
          onPutInServer:     function (h) { reg(viaId(function () { return Cl.onPutInServer(h); })); },
          onActive:          function (h) { reg(viaId(function () { return Cl.onActive(h); })); },
          onFullyConnect:    function (h) { reg(viaId(function () { return Cl.onFullyConnect(h); })); },
          onDisconnect:      function (h) { reg(viaId(function () { return Cl.onDisconnect(h); })); },
          onSettingsChanged: function (h) { reg(viaId(function () { return Cl.onSettingsChanged(h); })); },
          onVoice:           function (h) { reg(viaId(function () { return Cl.onVoice(h); })); },
          onCookiesCached:   function (h) { reg(viaId(function () { return Ck.onCached(h); })); },
          onSay:             function (h) { reg(viaId(function () { return Ch.onMessage(h); })); },
          onRunCmd:          function (h) { reg(viaId(function () { return Uc.onRun(h); })); },
        },
        entities: {
          onCreate: function (c, h) { reg(viaId(function () { return En.onCreate(c, h); })); },
          onSpawn:  function (c, h) { reg(viaId(function () { return En.onSpawn(c, h); })); },
          onDelete: function (c, h) { reg(viaId(function () { return En.onDelete(c, h); })); },
          onOutput: function (c, o, h) { reg(viaId(function () { return En.onOutput(c, o, h); })); },
          onDamage: function (h) { reg(viaId(function () { return Dm.onPre(h); })); },
        },
        server: {
          onGameFrame: function (fn, opts) { reg(function () { var d = Fr.subscribe(fn, opts || {}); if (d && d.dispose) t.disposer(d.dispose); }); },
          onMapStart:  function (h) { reg(viaId(function () { return Sv.onMapStart(h); })); },
          onPrecache:  function (h) { reg(viaId(function () { return Sn.onPrecache(h); })); },
        },
      };
    }
    var ctx = makeSubjects(null);
    ctx.id = __s2_current_plugin();
    ctx.previous = __s2_handoff_take();
    ctx.commands = {
      register:       function (n, h)    { reg(function () { __s2pkg_commands.Commands.register(n, h); }); },
      registerServer: function (n, h)    { reg(function () { __s2pkg_commands.Commands.registerServer(n, h); }); },
      registerAdmin:  function (n, f, h) { reg(function () { __s2pkg_commands.Commands.registerAdmin(n, f, h); }); },
    };
    ctx.config  = { onChange: function (h) { reg(function () { __s2pkg_config.config.onChange(h); }); } };
    ctx.topmenu = {
      addCategory: function (n)    { reg(function () { __s2pkg_topmenu.TopMenu.addCategory(n); }); },
      addItem:     function (c, i) { reg(function () { __s2pkg_topmenu.TopMenu.addItem(c, i); }); },
    };
    ctx.publish = function (name, impl) {
      reg(function () { __s2_iface_publish(name, impl); });
      return { emit: function (ev, payload) { return __s2_iface_emit(name, ev, payload); } };
    };
    function handleFor(name) {
      return new Proxy({}, { get: function (_t, prop) {
        if (prop === "on") return function (ev, h) { reg(function () { __s2_iface_on(name, ev, h); }); };
        if (typeof prop !== "string") return undefined;
        return function () { return __s2_iface_call(name, prop, Array.prototype.slice.call(arguments)); };
      }});
    }
    ctx.use = function (name) {
      if (sealed) throw new Error("s2script: ctx.use outside the load window");
      var kind = __s2_iface_dep_kind(name);
      if (kind !== "hard") throw new Error("s2script: ctx.use('" + name + "') requires a pluginDependencies entry (declared: " + kind + ")");
      return handleFor(name);
    };
    ctx.tryUse = function (name) {
      if (sealed) throw new Error("s2script: ctx.tryUse outside the load window");
      var kind = __s2_iface_dep_kind(name);
      if (kind !== "optional") throw new Error("s2script: ctx.tryUse('" + name + "') requires an optionalPluginDependencies entry (declared: " + kind + ")");
      return __s2_iface_is_published(name) ? handleFor(name) : null;
    };
    ctx.createScope = function () {
      if (sealed) throw new Error("s2script: createScope outside the load window");
      var ids = [], disposers = [], disposed = false;
      var tracker = { ids: function (i) { ids.push(i); }, disposer: function (d) { disposers.push(d); } };
      var scope = makeSubjects(tracker);
      // Scope regs are legal ANY time until disposed: rebind reg for the scope's subjects.
      // makeSubjects closes over the OUTER reg; scopes need their own — so shadow it:
      // (implementation note: pass reg as a parameter into makeSubjects instead of closing over —
      //  makeSubjects(track, regFn); ctx uses the buffering reg, scopes use a direct-or-buffered reg:)
      scope.clear = function () { __s2_scope_dispose(ids.slice()); ids.length = 0; var ds = disposers.slice(); disposers.length = 0; for (var i = 0; i < ds.length; i++) { try { ds[i](); } catch (e) {} } };
      scope.dispose = function () { if (disposed) return; scope.clear(); disposed = true; };
      Object.defineProperty(scope, "disposed", { get: function () { return disposed; } });
      scopes.push(scope);
      return scope;
    };
    ctx.__scopeReg = function (thunk) {  // the scope reg: buffer while Loading, immediate after
      if (!armed) { pending.push(thunk); } else { thunk(); }
    };
    globalThis.__s2_ctx_arm = function () {
      var p = pending; pending = []; armed = true;
      for (var i = 0; i < p.length; i++) { p[i](); }   // a throw here aborts the arm → Failed (host TryCatch)
      sealed = true;
    };
    globalThis.__s2_ctx_seal = function () { sealed = true; pending = []; };
    return ctx;
  }

  globalThis.__s2_run_factory = function (def) {
    var ctx = __s2_make_ctx();
    var out;
    try { out = def.factory(ctx); }
    catch (e) { __s2_load_failed(String((e && e.stack) || e)); return; }
    if (out && typeof out.then === "function") {
      out.then(function (hooks) { __s2_load_settled(hooks); },
               function (e) { __s2_load_failed(String((e && e.stack) || e)); });
    } else {
      __s2_load_settled(out);
    }
  };
```

**Implementation note (resolve during coding, keep the contract):** `makeSubjects` must take the reg function as a parameter (`makeSubjects(track, regFn)`) — ctx passes the load-window `reg`, `createScope` passes a scope-reg that buffers while `!armed` and registers immediately after. `__s2_scope_dispose` arrives in T3; until then stub it in the prelude as `function __s2_scope_dispose() {}` IF T3 hasn't landed — NO: T3 is sequenced after T2 in the same stack; instead have `scope.clear` guard `typeof __s2_scope_dispose === "function"` so T2 is green standalone. Keep `viaId` tolerant of natives that return `undefined` (they all do until T3 makes them return ids).

- [ ] **Step 4: The three natives.** Install in `install_natives` (`:7431`, alongside the other `__s2_*`):

```rust
/// __s2_load_settled(hooks?) — the factory settled OK. Stores the hooks object (if any) on the
/// PluginInstance and marks LOADING Settled. Idempotent-hostile: a second call for the same id WARNs.
fn s2_load_settled(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        if args.length() >= 1 {
            if let Ok(obj) = v8::Local::<v8::Object>::try_from(args.get(0)) {
                let g = v8::Global::new(scope.as_ref(), obj);
                PLUGINS.with(|p| { if let Some(pi) = p.borrow_mut().get_mut(&id) { pi.exports = Some(g); } });
            }
        }
        LOADING.with(|l| { if let Some(e) = l.borrow_mut().get_mut(&id) {
            if matches!(e.state, SettleState::InFlight) { e.state = SettleState::Settled; }
        }});
    }));
}
/// __s2_load_failed(message)
fn s2_load_failed(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        let msg = if args.length() >= 1 { args.get(0).to_rust_string_lossy(scope) } else { "factory failed".into() };
        LOADING.with(|l| { if let Some(e) = l.borrow_mut().get_mut(&id) {
            if matches!(e.state, SettleState::InFlight) { e.state = SettleState::Failed(msg); }
        }});
    }));
}
/// __s2_handoff_take() -> revived previous state (or undefined). Consume-once (5E.3 mechanics moved
/// from onLoad(prev) to ctx.previous).
fn s2_handoff_take(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        let Some(blob) = PENDING_HANDOFF.with(|h| h.borrow_mut().remove(&id)) else { return };
        if let Some(v) = iface_from_json(scope, &blob) { rv.set(v); }
    }));
}
```

(`iface_from_json` is currently called with a TryCatch pin at `:9167` — match its actual signature; if it needs the TryCatch-style scope, wrap accordingly, exactly as the old `onLoad(prev)` call site did.)

- [ ] **Step 5: Rewrite `load_plugin_js` (`:9079`) as the artifact loader.** Keep the fn name + signature (`loader.rs` keeps calling it). Changes inside, after the wrapper eval captures `exports` (`:9151`):

```rust
// The artifact: module.exports.default must be a plugin() definition (spec §1.1).
let def: Option<v8::Local<v8::Object>> = (|| {
    let k = v8::String::new(tc, "default")?;
    let v = exports.get(tc, k.into())?;
    let o = v8::Local::<v8::Object>::try_from(v).ok()?;
    let tag_k = v8::String::new(tc, "__s2plugin")?;
    let tag = o.get(tc, tag_k.into())?;
    if tag.int32_value(tc)? != 1 { return None; }
    let f_k = v8::String::new(tc, "factory")?;
    let f = o.get(tc, f_k.into())?;
    if !f.is_function() { return None; }
    Some(o)
})();
let Some(def) = def else {
    // Fail loud (locked decision #5): legacy onLoad shape or a malformed default export.
    let has_legacy = /* exports.get("onLoad").is_some_and(|v| v.is_function()) */;
    let reason = if has_legacy {
        "legacy plugin shape (export onLoad) - rebuild with @s2script/sdk >= 0.2: export default plugin(factory)"
    } else {
        "default export is not a plugin() definition"
    };
    log_warn(&format!("WARN: load('{}'): {}", id, reason));
    crate::crash::report_js_error(id, "load", reason, "");
    FAILED_PLUGINS.with(|f| f.borrow_mut().insert(id.to_string(), reason.to_string()));
    // teardown the fresh context (partial-ledger walk is a no-op here)
    break 'blk None;   // then, after the HOST borrow drops, call unload_partial(id) — see Step 7
};
// Register the in-flight load BEFORE running the factory (a sync settle mutates this entry).
LOADING.with(|l| l.borrow_mut().insert(id.to_string(), LoadingEntry {
    started_frame: FRAME_COUNTER.with(|c| c.get()), state: SettleState::InFlight, pending_reload: false }));
// Run the driver: globalThis.__s2_run_factory(def)
let global = /* context global */;
let run_k = v8::String::new(tc, "__s2_run_factory").unwrap();
let run_f = v8::Local::<v8::Function>::try_from(global.get(tc, run_k.into()).unwrap()).unwrap();
let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
if run_f.call(tc, recv, &[def.into()]).is_none() {
    let msg = tc.exception().map(|e| e.to_rust_string_lossy(&*tc)).unwrap_or_else(|| "factory driver threw".into());
    LOADING.with(|l| { if let Some(e) = l.borrow_mut().get_mut(id) { e.state = SettleState::Failed(msg); } });
}
```

Do NOT store `exports` as before (`pi.exports` now carries hooks, set by `__s2_load_settled`). After the `HOST.with` block returns, run the **sync fast-path**: `finalize_loading_plugins()` (Step 6) — a sync factory is already `Settled`, so it arms + reconciles + goes Active within this same call, preserving today's synchronous-load semantics for the whole base suite.

- [ ] **Step 6: `finalize_loading_plugins()` + the `Failed` teardown**

```rust
/// Drive every in-flight load to its transition (spec §5). Called (1) at the tail of
/// frame_async_drain — after the microtask checkpoint that runs factory continuations — and
/// (2) inline at the end of load_plugin_js for the sync fast-path.
pub(crate) fn finalize_loading_plugins() {
    let frame = FRAME_COUNTER.with(|c| c.get());
    let snapshot: Vec<(String, SettleStateSnapshot, bool, u64)> = LOADING.with(|l| { /* clone id+state+pending_reload+started */ });
    for (id, state, pending_reload, started) in snapshot {
        match state {
            Snapshot::Settled => {
                // (1) arm: replay buffered registrations + seal the ctx.
                let arm_ok = eval_in_context(&id, "globalThis.__s2_ctx_arm && globalThis.__s2_ctx_arm();").is_ok();
                if !arm_ok { fail_load(&id, "a registration failed while arming at Active"); continue_or_reload(&id, pending_reload); continue; }
                // (2) publishes reconciliation MOVES here from loader::load_and_reconcile (spec §4).
                if let Err(e) = reconcile_publishes(&id) { fail_load(&id, &format!("publishes: {}", e)); continue_or_reload(&id, pending_reload); continue; }
                // (3) Active.
                PLUGINS.with(|p| { if let Some(pi) = p.borrow_mut().get_mut(&id) { pi.phase = crate::plugin::Phase::Active; } });
                LOADING.with(|l| { l.borrow_mut().remove(&id); });
                if let Some((_, version)) = plugin_manifest_version(&id) { crate::crash::breadcrumb::plugin_loaded(&id, &version); }
                log_warn(&format!("[plugins] '{}' Active", id));
                if pending_reload { crate::loader::request_reload(&id); }
            }
            Snapshot::Failed(msg) => { fail_load(&id, &msg); continue_or_reload(&id, pending_reload); }
            Snapshot::InFlight if frame.saturating_sub(started) > LOAD_TIMEOUT_FRAMES => {
                let _ = eval_in_context(&id, "globalThis.__s2_ctx_seal && globalThis.__s2_ctx_seal();");
                fail_load(&id, "factory did not settle within ~30s (LOAD_TIMEOUT_FRAMES)");
                continue_or_reload(&id, pending_reload);
            }
            _ => {}
        }
    }
    crate::loader::start_unblocked_waiters();   // T4 provides; until then a no-op stub in loader.rs
}

fn fail_load(id: &str, reason: &str) {
    log_warn(&format!("WARN: load('{}') FAILED: {} - tearing down (the plugin is NOT running)", id, reason));
    crate::crash::report_js_error(id, "factory", reason, "");
    FAILED_PLUGINS.with(|f| f.borrow_mut().insert(id.to_string(), reason.to_string()));
    LOADING.with(|l| { l.borrow_mut().remove(id); });
    unload_partial(id);
}

/// Teardown for a never-Active plugin: seal (done by caller where needed), sweep stores (no-op for
/// buffered subs), walk the PARTIAL ledger (DB conns/timers/imports acquired before the failure),
/// dispose context. Skips onUnload/state() — the plugin was never Active (spec §5.4).
fn unload_partial(id: &str) { /* = unload_plugin minus the hooks/state block */ }
```

`unload_plugin` itself becomes phase-aware at the top:

```rust
pub(crate) fn unload_plugin(id: &str) {
    crate::crash::breadcrumb::plugin_unloaded(id);
    let phase = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.phase));
    if matches!(phase, Some(crate::plugin::Phase::Loading)) {
        let _ = eval_in_context(id, "globalThis.__s2_ctx_seal && globalThis.__s2_ctx_seal();");
        LOADING.with(|l| { l.borrow_mut().remove(id); });
        unload_partial(id);
        return;
    }
    PLUGINS.with(|p| { if let Some(pi) = p.borrow_mut().get_mut(id) { pi.phase = crate::plugin::Phase::Unloading; } });
    crate::owner_stores::sweep_owner(id);      // T1
    refresh_detour();
    capture_state_and_run_onunload(id);        // Step 7
    /* ledger reverse-walk + iface cleanup + exports drop + dispose — UNCHANGED from :10018-10108 */
}
```

- [ ] **Step 7: `capture_state_and_run_onunload`** — replaces the `(b)` block (`:9960-10016`). Same scope/TryCatch construction; reads off the stored hooks object (`pi.exports`): call `state()` FIRST (serialize via `iface_to_json` → `PENDING_HANDOFF`, WARN on non-serializable — reuse the existing wording), then `onUnload()` (return ignored; if it returns non-undefined, WARN once: `"onUnload return is ignored - use state() for the reload handoff"`).

- [ ] **Step 8: Wire the drain + loader.** Add `finalize_loading_plugins();` at the tail of `frame_async_drain` (`:9827`, after `periodic_sweep()`). In `loader.rs`, `load_and_reconcile` (`:84`) shrinks to (reconcile moved into finalize):

```rust
fn start_load(manifest: &Manifest, js: &str, cfg: &str) {
    crate::v8host::load_plugin_js(&manifest.id, js, cfg);
    // Transition + breadcrumb now happen in v8host::finalize_loading_plugins (sync fast-path or a later drain).
}
```

`v8host` needs the version for the breadcrumb at Active: add `static MANIFEST_VERSIONS: RefCell<HashMap<String,String>>` populated from the loader (`store_config_decls` call site) — or simpler: loader passes version into `load_plugin_js` — pick ONE and keep `plugin_manifest_version` above consistent (recommended: add a `set_plugin_version(id, version)` setter next to `set_plugin_publishes`, called from both loader paths at `:262/:376/:403`).

- [ ] **Step 9: Rewrite the in-isolate tests.** Add the helper near the test mod top:

```rust
/// New-shape bundle: a plugin() definition whose factory body is `body` (sync). `ctx` is in scope.
fn def_js(body: &str) -> String {
    format!("module.exports.default = {{ __s2plugin: 1, factory: function (ctx) {{ {} }} }};", body)
}
```

Mechanically update every test that loads `module.exports.onLoad=...` JS (grep `onLoad` in the `#[cfg(test)]` mod — including the handoff tests at `:10676-10771` and the subscribe-from-handler test at `:10348`) to the `def_js` shape; handoff tests change from `onLoad(prev)` assertions to `ctx.previous` assertions and from onUnload-return capture to `state()` capture, e.g.:

```rust
// old: module.exports.onUnload = () => ({n: 7});
// new:
let js = def_js("globalThis.KEEP = ctx.previous; return { state: function () { return {n: 7}; } };");
```

New tests (each an in-isolate test following the file's existing pattern of `init` + `load_plugin_js` + `eval_in_context` assertions):

1. `sync_factory_reaches_active_in_one_call` — load `def_js("ctx.events.on('round_start', function(){});")`; assert `plugin_phase(id) == Some(Active)` immediately and `EVENT_MUX` has 1 sub for `round_start`.
2. `buffered_registration_does_not_arm_before_active` — factory = async (returns a promise resolved via a stashed resolver); after `load_plugin_js` assert phase Loading AND `EVENT_MUX.snapshot("round_start")` empty; resolve + `frame_async_drain()`; assert Active + 1 sub.
3. `throwing_factory_fails_loud_no_zombie` — factory throws after `ctx.events.on(...)`; assert phase reported via `FAILED_PLUGINS`, `PLUGINS` has no entry (context disposed), `EVENT_MUX` empty.
4. `async_rejection_fails` — factory returns `Promise.reject(new Error("nope"))`; drain; assert Failed reason contains "nope".
5. `legacy_shape_refused` — load `"module.exports.onLoad=()=>{};"`; assert FAILED_PLUGINS reason contains "legacy plugin shape".
6. `sealed_ctx_throws` — factory stashes ctx on `globalThis.LEAK = ctx`; after Active, `eval_in_context(id, "try { LEAK.events.on('x', function(){}); globalThis.T='no' } catch(e) { globalThis.T='threw' }")`; assert `T === "threw"`.
7. `load_timeout_fails` — never-settling factory (`return new Promise(function(){})`); advance `FRAME_COUNTER` past `LOAD_TIMEOUT_FRAMES` (set the Cell directly in-test) + `finalize_loading_plugins()`; assert Failed with the timeout reason.
8. `state_and_previous_roundtrip` — load A with `state()` returning `{n:7}`; `unload_plugin`; reload with factory asserting `ctx.previous.n === 7` into a global; check.
9. `unload_while_loading_seals_and_walks_partial_ledger` — async factory that first `__s2_delay(10)`s (a ledgered timer); `unload_plugin` mid-flight; assert TIMERS empty + context gone + no `state()` capture.

- [ ] **Step 10: Run + commit**

Run: `cargo test -p s2script-core` → all pass. `make check-boundary` → PASS.

```bash
git add core/src/plugin.rs core/src/v8host.rs core/src/loader.rs
git commit -m "core: lifecycle v2 - plugin() artifact, awaited factory, Loading/Active/Failed phase machine, arm-at-Active, state()/ctx.previous handoff"
```

---

### Task 3: `ctx.createScope()` disposal + subscribe natives return ids + `onDamage` return collapse

**Files:**
- Modify: `core/src/v8host.rs` (subscribe natives — `s2_event_subscribe:4976`, `s2_damage_subscribe:5068`, usercmd/chat/client/map/precache/cookie/output/entity-lifecycle subscribe natives nearby; `dispatch_damage:6983`; `install_natives:7431`; `INJECTED_STD_PRELUDE` scope stub), tests in the same file
- Test: in-isolate tests

**Interfaces:**
- Consumes: T1 `EventMux::subscribe_with_id`/`remove_by_ids`, `owner_stores::sweep_ids`, `v8host::next_sub_id()`; T2 ctx prelude (`viaId`, `t.ids`, `scope.clear`).
- Produces (relied on by ports, esp. zones):
  - Every EventMux-family subscribe native returns the `u64` sub id as a JS number (`rv.set(v8::Number::new(scope, id as f64).into())`).
  - Native `__s2_scope_dispose(ids: number[])` → `owner_stores::sweep_ids(&ids)`.
  - `ctx.entities.onDamage` handlers returning `>= HookResult.Handled (2)` zero the live damage and stop the chain.

- [ ] **Step 1 (test first):** in-isolate test `scope_clear_removes_only_scope_subs`:

```rust
let js = def_js(r#"
    ctx.events.on('round_start', function () { globalThis.PLUGIN_HITS = (globalThis.PLUGIN_HITS|0) + 1; });
    var s = ctx.createScope();
    s.events.on('round_start', function () { globalThis.SCOPE_HITS = (globalThis.SCOPE_HITS|0) + 1; });
    globalThis.S = s;
"#);
// load → Active; dispatch round_start once → both hit; eval "S.clear()"; dispatch again →
// PLUGIN_HITS == 2, SCOPE_HITS == 1; EVENT_MUX still has exactly 1 sub for round_start.
```

Run: `cargo test -p s2script-core scope_clear` → FAIL (ids not returned yet).

- [ ] **Step 2:** make each mux subscribe native allocate + return the id. Pattern (apply to every native that feeds an `EventMux` store — events on/onPre, damage, chat-message, the 7 client-lifecycle subs, map-start, precache, cookies-cached, usercmd, entity-lifecycle (onCreate/onSpawn/onDelete), entity-output; leave ws/net conn muxes alone — not scope surfaces):

```rust
let sub_id = next_sub_id();
let first = EVENT_MUX.with(|m| m.borrow_mut().subscribe_with_id(&name, sub_id, owner, generation, handler_g));
/* existing first-subscriber engine-op block unchanged */
rv.set(v8::Number::new(scope, sub_id as f64).into());
```

(The natives currently take `_rv` — rename to `mut rv`.)

- [ ] **Step 3:** add the native `__s2_scope_dispose(ids)`:

```rust
fn s2_scope_dispose(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(0)) else { return };
        let mut ids = Vec::with_capacity(arr.length() as usize);
        for i in 0..arr.length() {
            if let Some(v) = arr.get_index(scope, i) { if let Some(n) = v.number_value(scope) { ids.push(n as u64); } }
        }
        crate::owner_stores::sweep_ids(&ids);
    }));
}
```

Install it; remove the T2 prelude guard (`typeof __s2_scope_dispose === "function"` check) if one was added.

- [ ] **Step 4: onDamage return collapse.** In `dispatch_damage` (`:6983`), capture the handler's return:

```rust
match func.call(tc, recv, &[info_val]) {
    None => { /* existing WARN */ }
    Some(ret) => {
        // Return-type carries the block power (locked #8): >= Handled zeroes the live damage
        // and stops the chain. Reuse the exact write path the JS `info.damage = 0` setter uses —
        // locate the DamageInfo setter in the prelude (it calls a __s2_damage_set_* native /
        // engine op); call that op with 0.0 here.
        if let Some(n) = ret.int32_value(tc) {
            if n >= 2 { zero_current_damage(); break; }
        }
    }
}
```

The worker MUST find the actual setter native the prelude's `DamageInfo.damage` setter calls (grep `__s2pkg_damage` / `damage_set` in `v8host.rs`) and factor its op call into `zero_current_damage()`. Add an in-isolate test mirroring the existing damage-dispatch test (grep `dispatch_damage` in the test mod): a handler returning `HookResult.Handled` → the damage-set op records 0 (assert via whatever seam the existing damage tests use; if they use a mock ops table, assert the recorded set).

- [ ] **Step 5:** run `cargo test -p s2script-core` (new + old green), `make check-boundary`.

- [ ] **Step 6: Commit**

```bash
git add core/src/v8host.rs core/src/event_mux.rs
git commit -m "core: Scope disposal via per-sub ids (__s2_scope_dispose); subscribe natives return ids; onDamage return >= Handled blocks"
```

---

### Task 4: Loader v2 — topological activation, WAITING, reload-queue, timeout wiring, apiVersion 2

**Files:**
- Modify: `core/src/loader.rs` (whole poll/action pipeline `:335-543`, `HOST_API_VERSION_MAJOR:56`, `drain_pending_ops:238`, `plugin_list:204`, tests), `core/src/v8host.rs` (`plugin_phase`, `is_loading(id)`, `LOADING.pending_reload` setter), `packages/sdk/plugins.d.ts` (additive `state`)
- Test: `loader.rs` unit tests + in-isolate tests

**Interfaces:**
- Consumes: T2 `finalize_loading_plugins`, `LOADING`, `FAILED_PLUGINS`, `plugin_phase`; existing `__s2_iface_is_published` backing fn (grep `iface_is_published` for the core-side `pub(crate)` fn; if only a native exists, extract `pub(crate) fn iface_published(name: &str) -> bool`).
- Produces:
  - `HOST_API_VERSION_MAJOR = 2`.
  - `loader::start_unblocked_waiters()` (real body; T2 stubbed it).
  - Topo-ordered Load batches; `WAITING` with `LOAD_TIMEOUT_FRAMES` bound; reload-while-Loading queue; `plugin_list() -> Vec<(String, String /*state: running|loading|waiting|failed|unloaded*/)>`.
  - `plugins.d.ts`: `PluginInfo` gains `readonly state: "running" | "loading" | "waiting" | "failed" | "unloaded";` (additive; `loaded` kept, `loaded === (state === "running")`).

- [ ] **Step 1 (tests first, pure logic):** add to `loader.rs` tests a pure topo-sort unit:

```rust
#[test]
fn load_batch_orders_producers_before_consumers() {
    // c depends (hard) on iface "@x/if" which p publishes; order must put p first regardless of name order.
    let batch = vec![
        ("c".to_string(), vec!["@x/if".to_string()], vec![]),                      // (id, hard dep ifaces, publishes)
        ("p".to_string(), vec![], vec!["@x/if".to_string()]),
    ];
    let order = topo_order(&batch);
    assert_eq!(order, vec!["p".to_string(), "c".to_string()]);
}
#[test]
fn topo_cycle_falls_back_to_name_order() {
    let batch = vec![
        ("a".into(), vec!["@b/if".into()], vec!["@a/if".into()]),
        ("b".into(), vec!["@a/if".into()], vec!["@b/if".into()]),
    ];
    assert_eq!(topo_order(&batch), vec!["a".to_string(), "b".to_string()]);  // + a WARN
}
```

Implement `fn topo_order(batch: &[(String, Vec<String>, Vec<String>)]) -> Vec<String>` (Kahn's over iface-name edges; stable tie-break by id; on cycle WARN + lexicographic).

- [ ] **Step 2: batch the scan.** In `poll_plugins`, split action execution: parse ALL `Load`/`Reload` files first (collect `(path, mtime, manifest, js)`), then order via `topo_order` (deps = `plugin_dependencies` keys; publishes = `manifest.publishes` keys), then for each in order:
  - hard-dep gate: every `pluginDependencies` name satisfies `v8host::iface_published(name)` **or** has its producer earlier in this same batch **and** that producer went Active synchronously (check `plugin_phase(producer) == Some(Active)` at the consumer's turn — an async producer parks the consumer);
  - satisfied → `set_plugin_imports` + `set_plugin_publishes` + `set_plugin_version` + `materialize_for_load` + `start_load` + WATCH_STATE insert (existing code, unchanged);
  - not satisfied → insert into `WAITING: RefCell<HashMap<String, WaitingLoad>>` (`struct WaitingLoad { path: PathBuf, mtime: SystemTime, manifest: Manifest, js: String, since_frame: u64 }`) + WATCH_STATE insert (so mtime edits still retrigger).
- [ ] **Step 3: `start_unblocked_waiters()`** (replaces the T2 stub):

```rust
pub(crate) fn start_unblocked_waiters() {
    let frame = crate::v8host::current_frame();
    let ready: Vec<String> = WAITING.with(|w| w.borrow().iter().filter_map(|(id, wl)| {
        let unblocked = wl.manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n));
        let expired = frame.saturating_sub(wl.since_frame) > crate::v8host::LOAD_TIMEOUT_FRAMES;
        (unblocked || expired).then(|| id.clone())
    }).collect());
    for id in ready {
        let Some(wl) = WAITING.with(|w| w.borrow_mut().remove(&id)) else { continue };
        let unblocked = wl.manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n));
        if !unblocked {
            crate::v8host::log_warn(&format!(
                "WARN: '{}': hard dependency producer not Active after ~30s - loading anyway (calls will throw InterfaceUnavailable until it appears)", id));
        }
        /* set imports/publishes/version + materialize + start_load (same block as Step 2) */
    }
}
```

(`v8host::current_frame()` = tiny getter over `FRAME_COUNTER`.)

- [ ] **Step 4: reload/unload interplay.**
  - `compute_actions` `Reload` on an id that `v8host::is_loading(id)` → call `v8host::queue_pending_reload(id)` (sets `LOADING[id].pending_reload = true`) instead of unload+load; also update the WATCH_STATE mtime so the poll doesn't re-fire.
  - `Action::Unload` / `drain_pending_ops` Unload on a WAITING id → just remove from WAITING (+ `clear_pending_handoff`).
  - `drain_pending_ops` Reload path: same is_loading queue check.
- [ ] **Step 5: apiVersion 2 + states.** `HOST_API_VERSION_MAJOR: u32 = 2` (`:56`); fix the two loader tests (`api_version_compatible_accepts_matching_major` → `"2.x"`, `"2.0.0"`, `"^2.1.0"`; rejects `"1.x"`). Update every `make_test_s2sp` manifest in loader tests to `"apiVersion":"2.x"` and every inline `plugin.js` to the T2 `def_js` shape (write it literally: `"module.exports.default={__s2plugin:1,factory:function(ctx){}};"`). Extend `plugin_list()`:

```rust
pub(crate) fn plugin_list() -> Vec<(String, String)> {
    // running (WATCH_STATE ∧ phase Active) | loading (LOADING) | waiting (WAITING) |
    // failed (FAILED_PLUGINS) | unloaded (SUPPRESSED)
}
```

Update the `Plugins.list()` native + prelude to emit `{id, loaded, state}`; `packages/sdk/plugins.d.ts` `PluginInfo` gains `readonly state: string` with the doc comment listing the five values (additive — do NOT remove `loaded`).

- [ ] **Step 6:** `cargo test -p s2script-core` green; `make check-boundary`; `./scripts/check-plugins-typecheck.sh` (still green — plugins.d.ts change is additive); full generated-file gates (`check-schema/nav/events/csitem`, `test-boundary-nameleak`) → all PASS.

- [ ] **Step 7: Commit**

```bash
git add core/src/loader.rs core/src/v8host.rs packages/sdk/plugins.d.ts
git commit -m "loader: topological activation + WAITING + reload-while-Loading queue; apiVersion major 2; plugin states in sm plugins list"
```

---

### Task 5: SDK surface — `@s2script/sdk/plugin` types + deprecation pass

**Files:**
- Create: `packages/sdk/plugin.d.ts`
- Modify: `packages/sdk/commands.d.ts`, `packages/sdk/usercmd.d.ts`, and `@deprecated` tags in: `events.d.ts`, `frame.d.ts`, `server.d.ts`, `sound.d.ts`, `chat.d.ts`, `clients.d.ts`, `cookies.d.ts`, `damage.d.ts`, `entity.d.ts`, `config.d.ts`, `interfaces.d.ts`, `topmenu.d.ts`
- Create: `.changeset/l1-lifecycle-v2.md`
- Test: `./scripts/check-plugins-typecheck.sh`

**Interfaces:**
- Consumes: the frozen contract from the design spec §1.2 (T2's runtime already implements it).
- Produces (THE port contract — every fan-out task consumes this):
  - `packages/sdk/plugin.d.ts` EXACTLY as written in `docs/superpowers/specs/2026-07-20-L1-lifecycle-v2-design.md` §1.2 — copy it verbatim (it is complete, including imports and doc comments to add). Key signatures restated: `plugin(factory: PluginFactory): PluginDefinition`; `PluginFactory = (ctx: PluginContext) => void | PluginHooks | Promise<void | PluginHooks>`; `PluginHooks = { onUnload?(): void; state?(): unknown }`; `PluginContext = { id, previous, events, clients, entities, server, commands, config, topmenu, publish, use, tryUse, createScope }`; `Scope = { events, clients, entities, server, clear(), dispose(), disposed }`.
  - `commands.d.ts`: `CommandContext` renamed `CommandInvocation` (members untouched) + a deprecated alias `/** @deprecated renamed CommandInvocation (L1); removed in the cleanup task */ export type CommandContext = CommandInvocation;`
  - `usercmd.d.ts`: `Cmd` renamed `UserCmdView` + deprecated alias `export type Cmd = UserCmdView;`
- NOT in this task: removing any old verb (T29). Add `/** @deprecated moved to ctx.<subject>.<verb> (L1 lifecycle v2) — removed after the port fan-out */` on every member listed in the spec §2 table's left column.

- [ ] **Step 1:** write `packages/sdk/plugin.d.ts` (verbatim from spec §1.2). Doc-comment `CtxClients` with the already-connected-clients idiom (spec §2): *"Handlers fire for clients that connect AFTER Active. To cover already-connected clients, seed explicitly in the factory: `for (const c of Clients.all()) { … }`."*
- [ ] **Step 2:** apply the renames + aliases + `@deprecated` tags. `frame.d.ts`: tag `OnGameFrame.subscribe` `@deprecated moved to ctx.server.onGameFrame; this module is deleted in the cleanup task`.
- [ ] **Step 3:** changeset:

```md
---
"@s2script/sdk": minor
---

L1 lifecycle v2: the plugin is a typed artifact. New `@s2script/sdk/plugin` subpath
(`plugin()`, `PluginContext`, `Scope`, `PluginHooks`); every registration verb moves to `ctx`;
`CommandContext`→`CommandInvocation` (param naming: `cmd`); usercmd `Cmd`→`UserCmdView`;
apiVersion major is now 2.x. Old ambient registration verbs are deprecated and removed in-series.
```

- [ ] **Step 4:** `./scripts/check-plugins-typecheck.sh` → PASS (additive + aliases keep everything green). `node --experimental-strip-types --no-warnings -e "import('./packages/sdk/src/typecheck/typecheck.ts').then(async m => { const r = m.typecheckPlugin('plugins/basecommands', {packagesDir: 'packages'}); console.log(r.ok ? 'OK' : m.formatDiagnostics(r.diagnostics)); })"` → `OK`.
- [ ] **Step 5: Commit**

```bash
git add packages/sdk/plugin.d.ts packages/sdk/*.d.ts .changeset/l1-lifecycle-v2.md
git commit -m "sdk: @s2script/sdk/plugin surface (PluginContext/plugin()/Scope); CommandInvocation + UserCmdView renames; deprecate ambient registration verbs"
```

---

### Task 6: tsconfig single source of truth + `s2s create` template

**Files:**
- Create: `packages/sdk/src/tsconfig-shared.ts`
- Modify: `packages/sdk/src/typecheck/typecheck.ts` (`:81-102`), `packages/sdk/src/create/create.ts` (`tsconfigJson:208`, `pluginSource:174`, `packageJsonContent:249`)
- Test: `packages/sdk` node tests + the gate

**Interfaces:**
- Consumes: T5 `plugin.d.ts`.
- Produces:
  - `tsconfig-shared.ts`: `export const sharedCompilerOptionsJson = { strict: true, noEmit: true, moduleResolution: "bundler", module: "ESNext", target: "ES2020", lib: ["ES2020"], types: [], skipLibCheck: true, allowImportingTsExtensions: true } as const;` + `export function sharedProgramOptions(ts: typeof import("typescript")): import("typescript").CompilerOptions` mapping the same values to enum form (the exact enum mapping currently inlined at `typecheck.ts:81-102`).
  - `typecheck.ts` builds `options = { ...sharedProgramOptions(ts), baseUrl: packagesDir, paths: {…unchanged…} }`.
  - `create.ts` `tsconfigJson()` emits `{ compilerOptions: sharedCompilerOptionsJson, include: ["src", "node_modules/@s2script/sdk/globals.d.ts"] }`.
  - New scaffold `src/plugin.ts` (cs2 flavor):

```ts
import { plugin } from "@s2script/sdk/plugin";
import { Chat } from "@s2script/sdk/chat";

export default plugin((ctx) => {
  ctx.commands.register("hello", (cmd) => {
    cmd.reply("hello from s2script");
    if (cmd.callerSlot >= 0) {
      Chat.toSlot(cmd.callerSlot, "hello from s2script");
    }
  });
});
```

  - `none` flavor:

```ts
import { plugin } from "@s2script/sdk/plugin";
import { delay } from "@s2script/sdk/timers";

export default plugin((ctx) => {
  let n = 0;
  ctx.server.onGameFrame(() => { n += 1; });
  void delay(1000).then(() => console.log("s2script plugin alive; frames so far:", n));
});
```

  - `packageJsonContent`: `s2script.apiVersion` → `"2.x"`.

- [ ] **Step 1:** write `tsconfig-shared.ts`; refactor both consumers; scaffold sources + apiVersion.
- [ ] **Step 2:** `cd packages/sdk && npm test` — expected: only the 13 pre-existing failures (schema-runtime + player-identity); any NEW failure is yours (create/typecheck tests may snapshot the template — update them to the new template text).
- [ ] **Step 3:** `./scripts/check-plugins-typecheck.sh` → PASS.
- [ ] **Step 4: Commit**: `git add packages/sdk/src && git commit -m "sdk: one tsconfig source of truth (tsconfig-shared) + plugin(ctx) create template, apiVersion 2.x"`

---

## Port Kit (include verbatim in every fan-out task prompt, T7–T28)

**The contract you consume** (produced by T2 runtime + T5 types; import from `@s2script/sdk/plugin`):

```ts
plugin(factory: (ctx: PluginContext) => void | PluginHooks | Promise<void | PluginHooks>): PluginDefinition
PluginHooks = { onUnload?(): void; state?(): unknown }
ctx.events.on(name: string, h: (ev: GameEvent) => void): void
ctx.events.onPre(name: string, h: (ev: GameEvent) => HookResultValue | void): void
ctx.clients.onConnect/onPutInServer/onActive/onFullyConnect(h: (client: Client) => void | Promise<void>): void
ctx.clients.onDisconnect/onSettingsChanged/onVoice/onCookiesCached(h: (client: Client) => void): void
ctx.clients.onSay(h: (slot: number, text: string, teamonly: boolean) => HookResultValue | void): void
ctx.clients.onRunCmd(h: (cmd: UserCmdView, info: { slot: number }) => HookResultValue | void): void
ctx.entities.onCreate/onSpawn/onDelete(className: string, h: (entity: EntityRef | null, className: string) => void): void
ctx.entities.onOutput(classname: string, output: string, h: (ev: OutputEvent) => HookResultValue | void): void
ctx.entities.onDamage(h: (info: DamageInfo) => HookResultValue | void): void
ctx.server.onGameFrame(fn: () => void, opts?: { priority?: "high"|"normal"|"low"|"monitor" }): void
ctx.server.onMapStart(h: (mapName: string) => void): void
ctx.server.onPrecache(h: (pc: PrecacheContext) => void): void
ctx.commands.register(name: string, h: (cmd: CommandInvocation) => void): void
ctx.commands.registerServer(name: string, h: (cmd: CommandInvocation) => void): void
ctx.commands.registerAdmin(name: string, flags: number, h: (cmd: CommandInvocation) => void): void
ctx.config.onChange(h: (cfg: Config) => void): void
ctx.topmenu.addCategory(name: string): void
ctx.topmenu.addItem(category: string, item: TopMenuItem): void
ctx.publish<T extends object>(name: string, impl: T): PublishHandle
ctx.use<T extends object>(name: string): T & { on(event: string, h: (payload: any) => void): void }
ctx.tryUse<T extends object>(name: string): (T & { on(...): void }) | null
ctx.createScope(): Scope   // Scope = { events, clients, entities, server, clear(), dispose(), disposed } — load-window allocation, drive any time
ctx.previous: unknown      // revived state() of the previous instance
ctx.id: string
```

**The mechanical recipe** (design spec §2 disposition table is authoritative):

1. `import { plugin } from "@s2script/sdk/plugin";` — first import.
2. `export function onLoad(...)` → `export default plugin((ctx) => { ... });` (async factory iff the body awaits). Module-level helpers stay module-level; only registrations need `ctx`, so helpers that register move inside or take `ctx`.
3. Old verb → ctx verb per the table: `Commands.register*` → `ctx.commands.*`; `Chat.onMessage` → `ctx.clients.onSay`; `Clients.on*` → `ctx.clients.*`; `Cookies.onCached` → `ctx.clients.onCookiesCached`; `UserCmd.onRun` → `ctx.clients.onRunCmd`; `Damage.onPre` → `ctx.entities.onDamage`; `Entity.onCreate/onSpawn/onDelete/onOutput` → `ctx.entities.*`; `OnGameFrame.subscribe` → `ctx.server.onGameFrame`; `Server.onMapStart` → `ctx.server.onMapStart`; `Sound.onPrecache` → `ctx.server.onPrecache`; `config.onChange` → `ctx.config.onChange`; `publishInterface` → `ctx.publish`; `TopMenu.addCategory/addItem` → `ctx.topmenu.*`; ESM interface import → `ctx.use`/`ctx.tryUse`.
4. Drop now-unused imports; KEEP ambient ones still used (`Chat.toSlot`, `Clients.fromSlot`, `Admin`, `ADMFLAG`, `Server.*` reads, `config.get*`, `Player`, timers, `Vote`, `Menu`, `TopMenu.snapshot/select`, `Commands.list/dispatch`, `HookResult`).
5. **Every command-handler parameter named `ctx` is renamed `cmd`** (body references too). Type annotations `CommandContext` → `CommandInvocation`.
6. `export function onUnload()` → factory returns `{ onUnload() { ... } }` (omit entirely if the body was just a console.log — logging unload is now the host's job; DELETE pure-log onUnloads).
7. `package.json` → `"apiVersion": "2.x"` (in the `s2script` block, line ~7).
8. Async init: `await` it in the factory and delete `X | null` guards + hand-rolled ready promises — a failure now correctly fails the load (fail-loud is the DESIGN, not a regression).
9. NO other behavior changes (no polling→event rewrites, no logic edits) unless the task body says so.

**Verification for every port** (from the repo root; `<dir>` = the plugin dir):

```bash
node --experimental-strip-types --no-warnings -e "import('./packages/sdk/src/typecheck/typecheck.ts').then(m => { const r = m.typecheckPlugin('<dir>', {packagesDir: 'packages'}); if (!r.ok) { console.error(m.formatDiagnostics(r.diagnostics)); process.exit(1);} console.log('OK'); })"
cd <dir> && npx s2script build     # expect: dist/<id>.s2sp written
```

Then commit: `git add <dir> && git commit -m "port(<name>): plugin(ctx) lifecycle v2"`.

---

### Task 7: Port `plugins/basecommands`

**Files:** Modify `plugins/basecommands/src/plugin.ts` (167 lines), `plugins/basecommands/package.json`.
**Interfaces:** Consumes the Port Kit; specifically `ctx.commands.register`/`registerAdmin`, `ctx.entities.onDamage`, `ctx.topmenu.addItem`, ambient `Admin/ADMFLAG/Player/Server/Plugins/Menu/MenuStyle` (unchanged imports; DROP `Commands`, `Damage`, `TopMenu` imports — TopMenu only used for addItem here). Produces: the ported plugin.

- [ ] **Step 1:** rewrite. Before→after of the load-bearing hunks (the command bodies are UNCHANGED except `ctx` → `cmd`):

```ts
// BEFORE (imports + shape)
import { Commands } from "@s2script/sdk/commands";
import { Damage } from "@s2script/sdk/damage";
import { TopMenu } from "@s2script/sdk/topmenu";
export function onLoad(): void {
  Commands.registerAdmin("sm_kick", ADMFLAG.KICK, (ctx) => {
    const targetStr = ctx.arg(0);
    if (!targetStr) { ctx.reply("Usage: sm_kick <target> [reason]"); return; }
    const reason = ctx.argsFrom(1) || "Kicked by admin";
    const targets = Player.target(targetStr, ctx.callerSlot, true);
    ...
  Damage.onPre((info) => { ... });
  TopMenu.addItem("Server Commands", { id: "basecommands:map", ... });
}
export function onUnload(): void { console.log("[basecommands] onUnload"); }

// AFTER
import { plugin } from "@s2script/sdk/plugin";
import { Admin, ADMFLAG } from "@s2script/sdk/admin";
import { Player } from "@s2script/cs2";
import { Server } from "@s2script/sdk/server";
import { Plugins } from "@s2script/sdk/plugins";
import { Menu, MenuStyle } from "@s2script/sdk/menu";

const MAP_CHOICES = ["de_dust2", "de_inferno", "de_mirage", "de_nuke", "de_ancient", "de_anubis"];

export default plugin((ctx) => {
  ctx.commands.registerAdmin("sm_kick", ADMFLAG.KICK, (cmd) => {
    const targetStr = cmd.arg(0);
    if (!targetStr) { cmd.reply("Usage: sm_kick <target> [reason]"); return; }
    const reason = cmd.argsFrom(1) || "Kicked by admin";
    const targets = Player.target(targetStr, cmd.callerSlot, true);
    ...                                  // body otherwise verbatim, ctx.→cmd. throughout
  });
  // sm_map / sm_who / sm_rcon / sm_exec / sm_cvar / sm: same treatment (ctx.commands.registerAdmin
  // or ctx.commands.register + handler param cmd).
  ctx.entities.onDamage((info) => {
    ...                                  // body verbatim (in-place halve; no return)
  });
  Admin.add("76561199000000009", ADMFLAG.KICK | ADMFLAG.CHAT);   // ambient action — unchanged
  ...
  ctx.topmenu.addItem("Server Commands", { id: "basecommands:map", name: "Change Map", flags: ADMFLAG.CHANGEMAP,
    onSelect: adminSlot => { ... } });   // item body verbatim
  console.log("[basecommands] onLoad — kick/map/who/rcon/exec/cvar/sm registered");
});
```

(Delete the pure-log `onUnload`.) Every `(ctx)` command handler → `(cmd)`; there are 7 command registrations — verify with `grep -c "ctx\." src/plugin.ts` → 0 after.
- [ ] **Step 2:** `package.json` apiVersion → `"2.x"`.
- [ ] **Step 3:** Port Kit verification (typecheck one-liner OK; `npx s2script build` → `dist/…s2sp`).
- [ ] **Step 4:** Commit: `git add plugins/basecommands && git commit -m "port(basecommands): plugin(ctx) lifecycle v2"`.

---

### Task 8: Port `plugins/basechat`

**Files:** `plugins/basechat/src/plugin.ts` (90), `package.json`.
**Interfaces:** Consumes: `ctx.commands.registerAdmin(name, flags, (cmd: CommandInvocation) => void)`, `ctx.clients.onSay((slot, text, teamonly) => HookResultValue | void)`; ambient `Chat.toSlot`, `Admin/ADMFLAG`, `Player`, `ChatColors/Activity`, `HookResult`.

- [ ] **Step 1:** rewrite. Helpers (`actorName/doSay/doAdminChat/doPsay/resolveOne`) stay module-level verbatim. Shape:

```ts
import { plugin } from "@s2script/sdk/plugin";
// keep: Chat (toSlot), Admin/ADMFLAG, Player/ChatColors/Activity, HookResult; drop: Commands
export default plugin((ctx) => {
  ctx.commands.registerAdmin("sm_say", ADMFLAG.CHAT, (cmd) => {
    const msg = cmd.argString.trim();
    if (!msg) { cmd.reply("Usage: sm_say <message>"); return; }
    doSay(cmd.callerSlot, msg);
  });
  // sm_chat, sm_psay: same param rename
  ctx.clients.onSay((slot, text, teamonly) => {
    // body verbatim from the old Chat.onMessage handler (the @ / @@ trigger logic)
  });
});
```

(No onUnload existed.)
- [ ] **Step 2:** apiVersion `2.x`; Port Kit verification; commit `port(basechat)`.

---

### Task 9: Port `plugins/playercommands`

**Files:** `plugins/playercommands/src/plugin.ts` (94), `package.json`.
**Interfaces:** Consumes: `ctx.commands.registerAdmin`, `ctx.topmenu.addItem(category, {id,name,flags,onSelect})`; ambient `Player/Events/pickPlayer` (from `@s2script/cs2` — `Events.fire` is an action, stays), `ADMFLAG`.

- [ ] **Step 1:** `slapPlayer/slayPlayer/pickLoop` stay module-level. `onLoad` → factory; 3× `registerAdmin` → `ctx.commands.registerAdmin` with `(cmd)`; 2× `TopMenu.addItem` → `ctx.topmenu.addItem`; drop `Commands`/`TopMenu` imports; delete log-only onUnload. Example hunk:

```ts
  ctx.commands.registerAdmin("sm_rename", ADMFLAG.SLAY, (cmd) => {
    const targetStr = cmd.arg(0);
    const rawName = cmd.argsFrom(1).trim();
    ...
    Events.fire("player_changename", { userid: p.userId, oldname, newname });  // ambient — unchanged
```

- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(playercommands)`.

---

### Task 10: Port `plugins/antiflood`

**Files:** `plugins/antiflood/src/plugin.ts` (50), `package.json`.
**Interfaces:** Consumes: `ctx.config.onChange((cfg: Config) => void)`, `ctx.clients.onSay`; ambient `Chat.toSlot`, `config.getFloat/getInt`, `ChatColors`, `HookResult`, local `./flood`.

- [ ] **Step 1:** full new file (short enough):

```ts
import { plugin } from "@s2script/sdk/plugin";
import { Chat } from "@s2script/sdk/chat";
import { config } from "@s2script/sdk/config";
import { HookResult } from "@s2script/sdk/events";
import { ChatColors } from "@s2script/cs2";
import { floodStep } from "./flood";

interface SlotState { tokens: number; lastTime: number; lastNotify: number; }
const state = new Map<number, SlotState>();
const NOTIFY_INTERVAL = 2.0;

export default plugin((ctx) => {
  ctx.config.onChange(() => {
    console.log("[antiflood] config changed — flood_time=" + config.getFloat("flood_time") + " max_tokens=" + config.getInt("max_tokens"));
  });

  ctx.clients.onSay((slot, _text, _teamonly) => {
    const floodTime = config.getFloat("flood_time");
    if (floodTime <= 0) return HookResult.Continue;
    const maxTokens = config.getInt("max_tokens");
    const now = Date.now() / 1000;
    const prev = state.get(slot) ?? { tokens: 0, lastTime: 0, lastNotify: 0 };
    const r = floodStep({ tokens: prev.tokens, lastTime: prev.lastTime }, now, floodTime, maxTokens);
    let lastNotify = prev.lastNotify;
    if (r.block && now - lastNotify >= NOTIFY_INTERVAL) {
      Chat.toSlot(slot, " " + ChatColors.Red + "[antiflood] You are sending messages too fast. Please slow down.");
      lastNotify = now;
    }
    state.set(slot, { tokens: r.tokens, lastTime: r.lastTime, lastNotify });
    return r.block ? HookResult.Handled : HookResult.Continue;
  });

  console.log("[antiflood] active (flood_time=" + config.getFloat("flood_time") + " max_tokens=" + config.getInt("max_tokens") + ")");
});
```

- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(antiflood)`.

---

### Task 11: Port `plugins/adminhelp`

**Files:** `plugins/adminhelp/src/plugin.ts` (41), `package.json`.
**Interfaces:** Consumes: `ctx.commands.registerAdmin`; ambient `Commands.list()` (KEEP the `Commands` import — `list` stays ambient), `ADMFLAG`.

- [ ] **Step 1:** `flagsToLabel` stays; `onLoad` → factory; the one `registerAdmin` → `ctx.commands.registerAdmin("sm_help", ADMFLAG.GENERIC, (cmd) => { ... cmd.argInt(0, 1) ... cmd.reply(...) })`; body's `Commands.list()` call unchanged; delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(adminhelp)`.

---

### Task 12: Port `plugins/basecomm`

**Files:** `plugins/basecomm/src/plugin.ts` (83), `package.json`.
**Interfaces:** Consumes: `ctx.clients.onSay`, `ctx.clients.onPutInServer((client: Client) => void | Promise<void>)`, `ctx.commands.registerAdmin` (×6), `ctx.topmenu.addItem`; ambient `Clients.fromSlot` (KEEP `Clients` import), `Player/pickPlayer`, `HookResult`, `ADMFLAG`.

- [ ] **Step 1:** `gagged/muted/forTargets/setGag/setMute` stay module-level. Factory hunk:

```ts
export default plugin((ctx) => {
  ctx.clients.onSay((slot, _text, _teamonly) => {
    if (gagged.size === 0) return HookResult.Continue;
    const p = Player.fromSlot(slot);
    const sid = p ? p.steamId : null;
    return sid && gagged.has(sid) ? HookResult.Handled : HookResult.Continue;
  });
  ctx.clients.onPutInServer((c) => { if (muted.has(c.steamId)) c.voiceMuted = true; });
  ctx.commands.registerAdmin("sm_gag", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Gagged", "sm_gag <target>", (p) => setGag(p, true), true));
  // …ungag/mute/unmute/silence/unsilence identically…
  ctx.topmenu.addItem("Player Commands", { id: "basecomm:gag", name: "Gag", flags: ADMFLAG.CHAT,
    onSelect: adminSlot => pickPlayer(adminSlot, t => setGag(t, true)) });
});
```

- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(basecomm)`.

---

### Task 13: Port `plugins/basebans`

**Files:** `plugins/basebans/src/plugin.ts` (167), `package.json`.
**Interfaces:** Consumes: `ctx.commands.registerAdmin` (×3), `ctx.clients.onConnect`, `ctx.topmenu.addItem` (×2); ambient `Bans`, `Clients.fromSlot`, `Player/pickPlayer`, `Menu/MenuStyle`, `ADMFLAG`.

- [ ] **Step 1:** `banMessage` stays. All three commands → `ctx.commands.registerAdmin` with `(cmd)` (bodies verbatim; `ctx.arg/argInt/argsFrom/reply/callerSlot` → `cmd.*`). The connect enforcement:

```ts
  ctx.clients.onConnect((c) => {
    if (c.isBot) return;
    const b = Bans.get(c.steamId);
    if (!b) return;
    const now = Date.now() / 1000;
    if (b.until !== 0 && b.until <= now) return;
    c.kickWithReason(banMessage(b.reason, b.until));
  });
```

Both `TopMenu.addItem` blocks → `ctx.topmenu.addItem` (inner Menu/pickPlayer bodies verbatim). Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(basebans)`.

---

### Task 14: Port `plugins/reservedslots`

**Files:** `plugins/reservedslots/src/plugin.ts` (46), `package.json`.
**Interfaces:** Consumes: `ctx.clients.onActive`; ambient `Server.maxPlayers`, `Admin/ADMFLAG`, `Player.allConnected`, `config.getInt`.

- [ ] **Step 1:** full new body: keep `KICK_MESSAGE`; `export default plugin((ctx) => { ctx.clients.onActive((c) => { …body verbatim… }); console.log(…); });` — drop the `Clients` import (only `onActive` was used), delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(reservedslots)`.

---

### Task 15: Port `plugins/basetriggers`

**Files:** `plugins/basetriggers/src/plugin.ts` (61), `package.json`.
**Interfaces:** Consumes: `ctx.clients.onSay`; ambient `Server` reads, `nextFrame` (timers — stays ambient by design), `Chat.toAll`, `HookResult`.

- [ ] **Step 1:** helpers stay; `Chat.onMessage(...)` → `ctx.clients.onSay(...)` (body verbatim incl. the `nextFrame().then(...)` deferred reply); delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(basetriggers)`.

---

### Task 16: Port `plugins/funcommands`

**Files:** `plugins/funcommands/src/plugin.ts` (96), `package.json`.
**Interfaces:** Consumes: `ctx.commands.registerAdmin` (×5); **`CommandInvocation`** (the helper takes the invocation object); ambient `delay`, `Player/Pawn/Fade`, `ADMFLAG`.

- [ ] **Step 1:** the helper's signature is the type-rename showcase:

```ts
// BEFORE
import { Commands, CommandContext } from "@s2script/sdk/commands";
function forEachPawn(ctx: CommandContext, usage: string, verb: string, fn: (p: Player, pw: Pawn) => void, filterImmunity: boolean): void {
  let pattern = ctx.arg(0);
  if (!pattern) { if (ctx.callerSlot < 0) { ctx.reply("[SM] Usage: " + usage); return; } pattern = "@me"; }
  ...
// AFTER
import { plugin } from "@s2script/sdk/plugin";
import { CommandInvocation } from "@s2script/sdk/commands";
function forEachPawn(cmd: CommandInvocation, usage: string, verb: string, fn: (p: Player, pw: Pawn) => void, filterImmunity: boolean): void {
  let pattern = cmd.arg(0);
  if (!pattern) { if (cmd.callerSlot < 0) { cmd.reply("[SM] Usage: " + usage); return; } pattern = "@me"; }
  ...
```

All five commands: `ctx.commands.registerAdmin("sm_gravity", ADMFLAG.SLAY, (cmd) => { const factor = cmd.argFloat(1, 1.0); forEachPawn(cmd, …); })` etc. The `delay(secs*1000).then(...)` unfreeze stays byte-identical. Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(funcommands)`.

---

### Task 17: Port `plugins/clientprefs` (the fail-loud acceptance case)

**Files:** `plugins/clientprefs/src/plugin.ts` (78), `package.json`.
**Interfaces:** Consumes: async factory support (T2), `ctx.clients.onPutInServer/onDisconnect`, `ctx.server.onGameFrame`; ambient `Database.open`, `Client` type, the six `declare function __s2_cookie_*` natives (unchanged).

- [ ] **Step 1:** full new file — the null-guards + try/catch zombie DELETE (spec §10):

```ts
// @s2script/clientprefs (plugin) — cookie DB lifecycle. L1: the factory awaits the DB; a failure
// FAILS the load loudly (no zombie), so `db` is non-null by construction everywhere below.
import { plugin } from "@s2script/sdk/plugin";
import { Database } from "@s2script/sdk/db";
import { Client } from "@s2script/sdk/clients";

declare function __s2_cookie_load(steamid: string, name: string, value: string, updated: number): void;
declare function __s2_cookie_mark_cached(steamid: string): void;
declare function __s2_cookie_get_dirty(steamid: string): Record<string, string>;
declare function __s2_cookie_clear(steamid: string): void;
declare function __s2_cookie_take_offline_writes(): Array<[string, string, string, number]>;
declare function __s2_cookie_dispatch_cached(slot: number): void;

export default plugin(async (ctx) => {
  const db = await Database.open("clientprefs");
  await db.execute(
    "CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))"
  );

  async function loadCookies(client: Client): Promise<void> {
    if (client.steamId === "0") return;   // skip bots
    const steamId = client.steamId;
    try {
      const rows = await db.query("SELECT name, value, updated FROM cookies WHERE steamid = ?", [steamId]);
      for (const row of rows) __s2_cookie_load(steamId, String(row.name), String(row.value), Number(row.updated));
      __s2_cookie_mark_cached(steamId);
      __s2_cookie_dispatch_cached(client.slot);
    } catch (e) {
      console.log("[clientprefs] load ERROR for " + steamId + ": " + String(e));
    }
  }

  async function saveCookies(client: Client): Promise<void> {
    if (client.steamId === "0") return;
    const steamId = client.steamId;
    const dirty = __s2_cookie_get_dirty(steamId);
    __s2_cookie_clear(steamId);
    const now = Math.floor(Date.now() / 1000);
    try {
      for (const name of Object.keys(dirty)) {
        await db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
          [steamId, name, dirty[name], now]);
      }
    } catch (e) {
      console.log("[clientprefs] save ERROR for " + steamId + ": " + String(e));
    }
  }

  function drainOfflineWrites(): void {
    const writes = __s2_cookie_take_offline_writes();
    if (writes.length === 0) return;
    for (const [steamid, name, value, updated] of writes) {
      db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
        [steamid, name, value, updated]
      ).catch((e) => console.log("[clientprefs] offline-write ERROR: " + String(e)));
    }
  }

  ctx.clients.onPutInServer(loadCookies);
  ctx.clients.onDisconnect(saveCookies);
  ctx.server.onGameFrame(drainOfflineWrites);
  console.log("[clientprefs] table ready, lifecycle hooked");
});
```

(Per-row query/execute try/catch stays — a single bad row must not kill a running plugin; only the INIT failure fails the load. The connect-window race is gone by construction: the handlers arm at Active, which is after the DB is open.)
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(clientprefs)`.

---

### Task 18: Port `plugins/adminmenu`

**Files:** `plugins/adminmenu/src/plugin.ts` (47), `package.json`.
**Interfaces:** Consumes: `ctx.topmenu.addCategory` (×3), `ctx.commands.register`; ambient `TopMenu.snapshot/select` (KEEP the `TopMenu` import), `Menu/MenuStyle`, `Admin/ADMFLAG`.

- [ ] **Step 1:** `itemsFor/showCategory` stay (they use ambient `TopMenu.snapshot/select`). Factory: 3× `ctx.topmenu.addCategory(...)`; `Commands.register("sm_admin", ctx => …)` → `ctx.commands.register("sm_admin", (cmd) => { const slot = cmd.callerSlot; … cmd.reply(…) … })`. Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(adminmenu)`.

---

### Task 19: Port `plugins/basevotes`

**Files:** `plugins/basevotes/src/plugin.ts` (66), `package.json`.
**Interfaces:** Consumes: `ctx.commands.registerAdmin` (×2), `ctx.topmenu.addItem`; ambient `Vote`, `Chat`, `config`, `Player/pickPlayer`, `ADMFLAG`.

- [ ] **Step 1:** `parseTokens/startKickVote` stay verbatim (Vote.start is ambient). Both commands → `ctx.commands.registerAdmin` with `(cmd)`; the `TopMenu.addItem` → `ctx.topmenu.addItem`. Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(basevotes)`.

---

### Task 20: Port `plugins/zones` (the dbReady-deletion + Scope acceptance case)

**Files:** `plugins/zones/src/plugin.ts` (489), `package.json`. (Do NOT touch `plugins/zones/api.d.ts`.)
**Interfaces:** Consumes: async factory, `ctx.publish<Zones>("@s2script/zones", impl): PublishHandle`, `ctx.server.onMapStart/onGameFrame`, `ctx.entities.onOutput`, `ctx.commands.registerAdmin` (×9), `ctx.createScope()` (`Scope.server.onGameFrame` + `Scope.clear()`); ambient everything else already imported (`Database`, `Server`, `config`, `Player/Pawn/TriggerZone/Beam`, `Vector`, `Chat`).

- [ ] **Step 1:** structural rewrite (bodies of helpers/commands stay verbatim; the deltas):
  1. Imports: `+ plugin` from `@s2script/sdk/plugin`; DROP `Commands`, `OnGameFrame`, `publishInterface` (KEEP `PublishHandle` type import from `@s2script/sdk/interfaces`), DROP `Entity` from `@s2script/sdk/entity` (onOutput was its only use).
  2. DELETE lines 42-43 (`dbReadyResolve`/`dbReady`) and the `await dbReady;` in `upsertZone` (line 183).
  3. `let db: Database | null = null;` → the factory-scoped `const db` — every `if (db)` guard drops (`upsertZone`, `dropZone`, `loadMap`'s `if (!db) return`, `setZoneTags`, `sm_zone_tag`); module-level fns that used `db` move INSIDE the factory (they already form one cluster: `loadMap`, `upsertZone`, `dropZone`, `zonesImpl`, plus the command bodies — move the whole former-onLoad block in; pure helpers (`sanitize*`, `normBox`, `box12`, `zoneByTriggerIndex`, …) stay module-level, passing `zones`/`shown` module maps as today).
  4. The old async IIFE (lines 204-218) becomes the factory head:

```ts
export default plugin(async (ctx) => {
  const db = await Database.open("zones");
  await db.execute("CREATE TABLE IF NOT EXISTS zones (map TEXT, name TEXT, minX REAL, minY REAL, minZ REAL, maxX REAL, maxY REAL, maxZ REAL, tags TEXT, PRIMARY KEY (map, name))");
  try { await db.execute("ALTER TABLE zones ADD COLUMN tags TEXT"); } catch { /* already migrated */ }
  await loadMap(Server.mapName);          // now defined in-factory, db in scope
  ctx.server.onMapStart((map) => { loadMap(map).catch((e) => console.log(`[zones] loadMap error: ${e}`)); });
  iface = ctx.publish<Zones>("@s2script/zones", zonesImpl);
  ctx.entities.onOutput("trigger_multiple", "OnStartTouch", (ev) => { /* body verbatim */ });
  ctx.entities.onOutput("trigger_multiple", "OnEndTouch", (ev) => { /* body verbatim */ });
  ctx.server.onGameFrame(() => { /* beams-TTL + pendingTriggers + stay re-emit — body verbatim */ });
```

  5. **The editor poll becomes a Scope** (spec §3 — the acceptance exercise for scopes): replace the second `OnGameFrame.subscribe(() => { if (edits.size === 0) return; … })` with:

```ts
  const editPoll = ctx.createScope();
  let editPollArmed = false;
  function ensureEditPoll(): void {
    if (editPollArmed) return;
    editPollArmed = true;
    editPoll.server.onGameFrame(pollEditSessions);       // the old poll body, minus the size-0 early-return
  }
  function releaseEditPollIfIdle(): void {
    if (edits.size === 0 && editPollArmed) { editPoll.clear(); editPollArmed = false; }
  }
```

  `startMarking` calls `ensureEditPoll()` after `edits.set(...)`; `cancelEdit` calls `releaseEditPollIfIdle()` after `edits.delete(slot)` (NOTE: `pollEditSessions` iterates `edits` and calls `cancelEdit` — clearing the scope from inside its own handler is safe because `__s2_scope_dispose` only prunes mux rows; the current dispatch snapshot already ran).
  6. The 9 `Commands.registerAdmin` → `ctx.commands.registerAdmin` with `(cmd)` (bodies verbatim, `ctx.args/arg/argFloat/reply/callerSlot` → `cmd.*`).
  7. `export function onUnload()` → factory returns:

```ts
  return { onUnload() { clearAllBeams(); clearAllEdits(); clearAllTriggers(); } };
});
```

  (Behavior note, deliberate: a DB failure now FAILS the load instead of running non-persistent — spec §10 fail-loud; log this in the commit body.)
- [ ] **Step 2:** apiVersion `2.x` (`package.json:12`); verification (typecheck one-liner + build); commit `port(zones): plugin(ctx), dbReady deleted, editor poll on a Scope`.

---

### Task 21: Port `plugins/disabled/nominations`

**Files:** `plugins/disabled/nominations/src/plugin.ts` (147), `package.json`.
**Interfaces:** Consumes: async factory, `ctx.commands.register`, `ctx.server.onGameFrame`; ambient `Database`, `config`, `Menu/MenuStyle`, `Server`, `Player`, `Chat`.

- [ ] **Step 1:** deltas: `let db: Database | null` dies —

```ts
export default plugin(async (ctx) => {
  loadPool();                                            // eager template write — unchanged
  const db = await Database.open("mapvote");
  await db.execute("CREATE TABLE IF NOT EXISTS map_history(id INTEGER PRIMARY KEY AUTOINCREMENT, map TEXT NOT NULL, played_at INTEGER NOT NULL)", []);
  await db.execute("CREATE TABLE IF NOT EXISTS nominations(map TEXT PRIMARY KEY, nominator INTEGER NOT NULL)", []);
  ctx.server.onGameFrame(pollMapChange);
  ctx.commands.register("sm_nominate", (cmd) => { /* body verbatim, ctx.→cmd. */ });
});
```

`cooldownSet/nominatedSet/nominate/recordMapStart/pollMapChange` move inside the factory (they read `db`); their `if (!db)` guards DELETE (`pollMapChange` keeps only the throttle + map-change logic). `mapMenu/nominateMenu/resolveMap/parseMaplist/loadPool/isCurrentMap` stay wherever they compile cleanest (in-factory is fine). Keep the map-name polling AS-IS (no onMapStart rewrite — out of scope). Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(nominations)`.

---

### Task 22: Port `plugins/disabled/rockthevote`

**Files:** `plugins/disabled/rockthevote/src/plugin.ts` (276), `package.json`.
**Interfaces:** Consumes: async factory, `ctx.server.onGameFrame`, `ctx.events.on("round_end", (ev) => void)`, `ctx.clients.onSay`, `ctx.clients.onDisconnect`, `ctx.commands.registerAdmin`; ambient `Vote/VoteResult`, `Clients.all/fromSlot`, `Server`, `config`, `Database`, `Chat`, `HookResult`, `ADMFLAG`.

- [ ] **Step 1:** deltas: factory awaits the mapvote DB (same two CREATEs as today, then `const db` non-null; `cooldownSet`/`nominationList` drop their `if (!db)` returns and move in-factory with `buildBallot/finishVote/startVote/requestRtv/pollTick`). Registrations:

```ts
  ctx.server.onGameFrame(pollTick);
  ctx.events.on("round_end", () => { if (!pendingMap) return; const m = pendingMap; Server.command(m.workshopId ? "host_workshop_map " + m.workshopId : "changelevel " + m.name); pendingMap = null; });
  ctx.clients.onSay((slot, text) => { /* rtv trigger body verbatim */ });
  ctx.commands.registerAdmin("sm_forcertv", ADMFLAG.CHANGEMAP, (cmd) => { cmd.reply(startVote(true) ? "RTV forced." : "A vote is already running."); });
  ctx.clients.onDisconnect((c) => rtvVoters.delete(c.slot));
```

Module state (`rtvVoters`, `voteRunning`, …) may stay module-level (no db dependence). Keep map polling as-is. Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(rockthevote)`.

---

### Task 23: Port `plugins/disabled/funvotes`

**Files:** `plugins/disabled/funvotes/src/plugin.ts` (86), `package.json`.
**Interfaces:** Consumes: `ctx.commands.registerAdmin` (×4); ambient `Vote`, `config`, `Chat`, `Server`, `Player`, `ADMFLAG`.

- [ ] **Step 1:** `startYesNo` stays verbatim; the 4 commands → `ctx.commands.registerAdmin(..., (cmd) => …)` (note the `ctx.reply` passed as a function value becomes `cmd.reply` — e.g. `startYesNo(cmd.reply, …)`; verify `reply` is not `this`-sensitive: it's used as `reply("…")` inside startYesNo and CommandInvocation.reply is already passed bare today (`ctx.reply`) — same pattern, keep it). Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(funvotes)`.

---

### Task 24: Port `plugins/disabled/nextmap`

**Files:** `plugins/disabled/nextmap/src/plugin.ts` (162), `package.json`.
**Interfaces:** Consumes: `ctx.server.onGameFrame`, `ctx.events.on`, `ctx.commands.registerAdmin`; ambient `Server`, `config`, `delay`, `Chat`, `ADMFLAG`.

- [ ] **Step 1:** all module state/helpers stay; factory:

```ts
export default plugin((ctx) => {
  loadPool();
  ctx.server.onGameFrame(pollTick);
  ctx.events.on("round_end", () => {
    if (changing) return;
    roundsPlayed++;
    const max = parseInt(Server.getCvar("mp_maxrounds"), 10);
    if (max > 0 && roundsPlayed >= max) changeToNext();
  });
  ctx.commands.registerAdmin("sm_setnextmap", ADMFLAG.CHANGEMAP, (cmd) => { /* body verbatim, ctx.→cmd. */ });
});
```

Delete onUnload.
- [ ] **Step 2:** apiVersion `2.x`; verification; commit `port(nextmap)`.

---

### Tasks 25–28: Port `examples/*` (4 independent batches)

**Shared for all four batches** — Files: each listed example's `src/plugin.ts` + `package.json`. Interfaces: Consumes the Port Kit ONLY. Procedure per example: (1) read its `src/plugin.ts`; (2) apply the Port Kit recipe — the spec §2 disposition table covers every verb; **if an example uses a registration verb not in the table, STOP and report it back instead of guessing**; (3) apiVersion `2.x`; (4) run the per-dir typecheck one-liner; (5) one commit per batch. Examples that publish interfaces use `ctx.publish`; consumers switch their ESM interface import to `ctx.use`/`ctx.tryUse` (keep monorepo-relative `import type` lines for payload types). `UserMessages.onPre` (usermsg-demo, gamerules-usermsg-demo) STAYS an ambient import (spec §2 exception).

**Task 25 — batch A:** `demo-plugin, s2bench, crash-test, round-control-demo, http-demo, db-demo, db-remote-demo, ws-demo, net-demo`.
Worked example (`demo-plugin`-class): `export function onLoad(){ OnGameFrame.subscribe(f); }` → `export default plugin((ctx) => { ctx.server.onGameFrame(f); });`.
Commit: `port(examples-a): plugin(ctx) lifecycle v2`.

**Task 26 — batch B:** `clients-demo, clientprefs-demo, admin-groups-demo, clientlist-convar-mapstart-demo, voice-demo, translations-demo, gamerules-usermsg-demo, usermsg-demo, sound-demo`.
Notes: `Clients.on*` → `ctx.clients.*`; `Cookies.onCached` → `ctx.clients.onCookiesCached`; `Sound.onPrecache` → `ctx.server.onPrecache`; `Server.onMapStart` → `ctx.server.onMapStart`.
Commit: `port(examples-b): plugin(ctx) lifecycle v2`.

**Task 27 — batch C:** `beam-demo, ekv-demo, entityio-demo, entity-listeners-demo, entity-name-demo, entref-producer, entref-consumer, items-demo, weapon-demo, schema-dump`.
Notes: `Entity.onOutput/onCreate/onSpawn/onDelete` → `ctx.entities.*`; `entref-producer` `publishInterface` → `ctx.publish`; `entref-consumer` ESM interface import → `ctx.use`.
Commit: `port(examples-c): plugin(ctx) lifecycle v2`.

**Task 28 — batch D:** `changeteam-demo, respawn-demo, switchteam-demo, trace-demo, transmit-demo, usercmd-demo, menu-demo, greeter-plugin, greeter-consumer, zones-consumer-demo`.
Worked example — `zones-consumer-demo` full replacement (deletes the poll-until-producer hack; topo activation guarantees the producer is Active first):

```ts
import { plugin } from "@s2script/sdk/plugin";
import type { Zones } from "../../../plugins/zones/api";
import type { ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "../../../plugins/zones/api";
import { Player } from "@s2script/cs2";

export default plugin((ctx) => {
  const zones = ctx.use<Zones>("@s2script/zones");
  zones.on("enter", (p: ZoneEvent) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] ENTER ${p.zone}: ${nm}`);
  });
  zones.on("leave", (p: ZoneEvent) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] LEAVE ${p.zone}: ${nm}`);
  });
  zones.on("stay", (p: ZoneEvent) => {
    if (p.zone !== "heal") return;
    const pw = Player.fromSlot(p.slot)?.pawn;
    if (pw && pw.health != null && pw.health < 100) {
      const nh = Math.min(100, pw.health + 1);
      pw.health = nh;
      if (nh % 20 === 0 || nh === 100) console.log(`[zones-consumer] healed slot ${p.slot} -> ${nh}`);
    }
  });
  zones.on("created", (p: ZoneCreatedEvent) => { console.log(`[zones-consumer] CREATED ${p.zone} tags=[${p.tags.join(",")}]`); });
  zones.on("deleted", (p: ZoneDeletedEvent) => { console.log(`[zones-consumer] DELETED ${p.zone}`); });
});
```

`usercmd-demo`: `UserCmd.onRun` → `ctx.clients.onRunCmd`; type `Cmd` → `UserCmdView`.
Commit: `port(examples-d): plugin(ctx) lifecycle v2`.

---

### Task 29: Remove the legacy surface (the unrepresentability flip) + fixtures + full gate

**Files:**
- Delete: `packages/sdk/frame.d.ts`
- Modify: `packages/sdk/{events,server,sound,chat,clients,cookies,usercmd,damage,entity,config,interfaces,topmenu,commands}.d.ts`; `packages/sdk/test/fixtures/**` (any fixture importing a removed verb / old shape); `packages/sdk/src/**` if any CLI test snapshots reference removed text
- Test: the FULL gate suite

**Interfaces:** Consumes: all fan-out tasks merged (this task is only green after them). Produces: the final L1 surface — registration verbs exist ONLY on `PluginContext`/`Scope`.

- [ ] **Step 1:** delete every `@deprecated`-tagged member added in T5: `Events.on/off/onPre` (KEEP `GameEvent`, `HookResult`, `HookResultValue`, `Events.fire/fireToClient`), `Server.onMapStart`, `Sound.onPrecache` (keep `emit` + `PrecacheContext`), `Chat.onMessage` (keep `color/toSlot/toAll`), `Clients.onConnect…onVoice` (keep `Client`, `fromSlot`, `all`), `Cookies.onCached`, `UserCmd` const entirely + the `Cmd` alias (keep `UserCmdView`), the `Damage` const (keep `DamageInfo`; file keeps its `EntityRef` import), `Entity.onOutput/onCreate/onSpawn/onDelete` (keep `Entity.findByClass`, `createEntity`, `OutputEvent`, `EntityRef`), `config.onChange`, `publishInterface` (keep `PublishHandle`), `TopMenu.addCategory/addItem` (keep `snapshot/select` + `TopMenuItem` — `plugin.d.ts` imports it), `Commands.register/registerServer/registerAdmin` (keep `dispatch/parseChatTrigger/handleChatTrigger/triggers/list` + `CommandInvocation`), the `CommandContext` alias. Delete `frame.d.ts`.
- [ ] **Step 2:** `grep -rn "OnGameFrame\|Chat.onMessage\|Commands.register\|publishInterface\|Damage.onPre\|UserCmd.onRun\|Cookies.onCached\|TopMenu.addItem\|TopMenu.addCategory\|config.onChange\|Server.onMapStart\|Sound.onPrecache\|CommandContext\b" plugins/ examples/ --include="*.ts"` → expected: ZERO hits outside comments (any hit = an unported file; fix it here).
- [ ] **Step 3:** fix `packages/sdk/test/fixtures/**` (typecheck fixtures' fake sdk stubs + any fixture plugin sources using the old shape/verbs) so `cd packages/sdk && npm test` shows only the 13 pre-existing failures.
- [ ] **Step 4:** FULL gate: `cargo test -p s2script-core` && `make check-boundary` && `./scripts/check-plugins-typecheck.sh` && `./scripts/check-schema-generated.sh` && `./scripts/check-nav-generated.sh` && `./scripts/check-events-generated.sh` && `./scripts/check-csitem-generated.sh` && `./scripts/test-boundary-nameleak.sh` && `./scripts/build-base-plugins.sh` → ALL PASS.
- [ ] **Step 5: Commit**: `git add -A && git commit -m "sdk: delete legacy registration surface - registration is now unrepresentable outside ctx/Scope"`.

---

### Task 30: Docs + the (held) live-gate checklist

**Files:** Modify `docs/ARCHITECTURE.md` (the lifecycle/teardown sections), Create `docs/superpowers/plans/2026-07-20-L1-live-gate-checklist.md`.

- [ ] **Step 1:** ARCHITECTURE.md: rewrite the plugin-lifecycle section to describe: `plugin(factory)` artifact + phase machine (Loading/Active/Unloading/Failed) + arm-at-Active + `ctx` subjects + Scope + `OWNER_SCOPED_STORES` + `state()`/`ctx.previous` + topological activation + apiVersion 2. Point at the L1 spec for detail. Do NOT edit CLAUDE.md.
- [ ] **Step 2:** write the checklist (each item = console proof on the Docker gate, `python3 scripts/rcon.py`):
  1. boot with the full suite → `sm plugins list` all `running`; join + `sm_help`, `!kick` smoke.
  2. hot-reload clientprefs (touch the .s2sp) → cookies survive (`state()`→`ctx.previous` path exercised by a set-cookie/reload/get-cookie sequence via clientprefs-demo).
  3. break clientprefs' DB path (chmod the data dir) → reload → `sm plugins list` shows `failed` + named WARN; NO zombie (`sm_help` still lists other plugins' commands only).
  4. zones: `sm_zone_edit` open → E-mark; `sm_zone_edit cancel` → scope cleared (add a temporary `sm_zone_debug` print of nothing — or verify via the [zones] logs).
  5. zones-consumer: boot order consumer-before-producer alphabetically → consumer still gets `created`/`enter` (topo activation).
  6. drop a legacy 1.x `.s2sp` into plugins/ → refused with the apiVersion WARN.
  7. reload-while-Loading: touch a slow-loading test plugin twice quickly → queued reload runs, no double-load.
- [ ] **Step 3:** Commit: `git add docs && git commit -m "docs: lifecycle v2 architecture + L1 live-gate checklist"`. **The live gate itself is HELD for the human** (slice convention).

---

## Self-Review

**1. Spec coverage** (L1 spec § → task):
- §1.1 artifact + legacy rejection → T2; apiVersion 2 → T4 + every port Step 2 + T6 template.
- §1.2 types → T5 (+ renames T5, alias removal T29).
- §2 disposition table → T5 (deprecate), T7–T28 (migrate), T29 (delete); timers/admin/votes/menus stay ambient — no task touches them (by design); `UserMessages.onPre` exception honored in T25/T26 notes.
- §3 Scope → T2 (prelude) + T3 (disposal natives) + T20 (live consumer) + tests in T3.
- §4 arm-at-Active / seal / use-tryUse / reconcile move / sync fast path → T2 (Steps 3-6).
- §5 phase machine, timeout, reload-queue, unload-while-Loading, Failed → T2 + T4; §5.4 topo + WAITING → T4.
- §6 teardown unification + EventMux ids → T1 (+ sweep in T2's unload rewrite).
- §7 handoff → T2 (Steps 4, 7, test 8).
- §8 loader/operator (`plugin_list` states, `PluginInfo.state`) → T4.
- §9 SDK/tsconfig/template/changeset → T5/T6; removal → T29.
- §10 ports + acceptance + live gate → T7–T28, T30.
- Gaps: none found. (Deliberate non-tasks: Hud, B1/B2, E1 — out of scope per spec header.)

**2. Placeholder scan:** the spine tasks contain two explicitly-delegated lookups that are NOT placeholders but named verification points anchored to real code: T3 Step 4's `zero_current_damage()` (worker must reuse the existing DamageInfo-setter op — the plan names the grep) and T2 Step 8's `set_plugin_version` choice (two options given, one recommended). Port tasks use "body verbatim" only where the body is untouched by the transformation and the worker holds the file — every CHANGED line is shown. No TBD/TODO/"handle edge cases" items remain.

**3. Type consistency across the spine→ports boundary:** `plugin`/`PluginContext`/`PluginHooks`/`Scope`/`CommandInvocation`/`UserCmdView` names match between the spec §1.2, T2's runtime (prelude member names `events/clients/entities/server/commands/config/topmenu/publish/use/tryUse/createScope/previous/id`), T5's d.ts, the Port Kit, and every port task. `ctx.clients.onSay` keeps `(slot, text, teamonly)` everywhere; `onRunCmd` uses `(cmd: UserCmdView, info: {slot})` in both T5 and T28's usercmd-demo note; `PluginInfo.state` string values match T4 Step 5 and T30's checklist items 1/3. `__s2_scope_dispose` name matches T2's guard note and T3's native. `LOAD_TIMEOUT_FRAMES = 1920` consistent in T2/T4.

# Slice 4 — One `.s2sp` That Hot-Reloads (Context + Ledger) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the whole plugin lifecycle end-to-end — one TS plugin built to a `.s2sp`, loaded into its own V8 context, exercising Slices 1–3, that hot-reloads and tears down cleanly via a ledger.

**Architecture:** Refactor `v8host` from a single shared context to a per-plugin-context registry inside the one isolate; the calling plugin is identified by the current context's embedder slot. A V8-free `plugin` module owns the ledger/registry/generation/teardown; a `loader` module watches `/plugins`, reads `.s2sp` archives, and drives load/unload/reload; an `@s2script/cli` (TypeScript, esbuild) builds plugins into `.s2sp`. The async-liveness guard drops continuations whose plugin is gone.

**Tech Stack:** Rust (`s2script-core` cdylib, `v8` 149.4.0), C++ MM:S shim, TypeScript + esbuild (the CLI + plugins), Node v24, Docker (`joedwards32/cs2`) + sniper build.

## Global Constraints

- **Context-per-plugin:** one shared isolate; each plugin gets its own `v8::Context`. The calling plugin is identified by **the current context's embedder-data slot** (`get_current_context()` from a native scope), NEVER a thread-local "current plugin" — so it stays correct across the microtask checkpoint (which runs each plugin's continuation in its own context).
- **The ledger is the teardown authority:** every persistent effect (hook subs, timers, pending async) is auto-recorded per-plugin; teardown walks the ledger in reverse at a frame boundary, then disposes the context. The plugin's own cleanup code is never trusted.
- **Async-liveness guard:** each timer/job resolver is tagged `(plugin_id, generation)`; the frame drain drops a continuation whose plugin is absent or whose generation advanced (reloaded). No continuation ever runs into a disposed/replaced context.
- **Naming convention (locked):** PascalCase events + types (`OnGameFrame`, `Pawn`), camelCase functions + properties (`delay`, `nextTick`, `nextFrame`, `threadSleep`, `pawn.health`). This renames `onGameFrame → OnGameFrame.subscribe` and `Delay/NextTick/NextFrame → delay/nextTick/nextFrame`.
- **TS authoring, transpile-only:** plugins + the CLI are TypeScript, bundled by esbuild; NO blocking `tsc` typecheck gate (deferred).
- **Degrade per-descriptor, never crash:** a bad `.s2sp` / manifest / load failure degrades with a named reason; the server keeps running. No Rust panic crosses the FFI boundary (`catch_unwind`).
- **`cargo test -p s2script-core -- --test-threads=1`**; all Slice 0–3 behavior stays green (under the renamed API + the new plugin-context test harness). `make check-boundary` + the name-leak gate stay green. Sniper build loadable.
- **Deferred (do NOT build):** `tsc` gate; inter-plugin deps/proxies (Slice 4.5); handle/`EntityRef` (Slice 5); config materialization; permissions enforcement; reload state-handoff; topo-sort load order.

---

### Task 1: Spike — CJS eval wrapper + rusty_v8 context embedder slot (findings doc; no production code)

**Purpose:** Validate the two §13 unknowns before the refactor is built on them. Output is a committed findings doc + throwaway proofs. Reconnaissance, not TDD.

**Files:**
- Create: `docs/superpowers/specs/2026-07-01-slice-4-spike-findings.md`
- Throwaway (deleted at task end): a scratch Rust test + a scratch esbuild bundle.

**Interfaces:**
- Produces: the findings doc that Tasks 4–7 cite for (a) the exact CJS eval-wrapper shape and (b) the exact rusty_v8 embedder-slot / current-context API calls.

- [ ] **Step 1: Prove the CJS eval wrapper in bare V8.** In a scratch `#[test]` (a fresh isolate+context, mirror `v8host::init`'s context construction), esbuild-bundle a tiny TS entry:
  ```ts
  // scratch/entry.ts
  import { greet } from "@s2script/std";
  export function onLoad() { globalThis.__loaded = greet("world"); }
  ```
  built with `esbuild scratch/entry.ts --bundle --platform=neutral --format=cjs --external:@s2script/std`. Eval the produced `plugin.js` wrapped as:
  ```js
  (function (require, module, exports) { /* <plugin.js> */ })(__s2require, module, module.exports);
  ```
  where `__s2require("@s2script/std")` returns `{ greet: (s) => "hi " + s }` and `module = { exports: {} }`. Confirm: after eval, `module.exports.onLoad` is a function; calling it sets `globalThis.__loaded === "hi world"`. Record the exact wrapper string + how `require`/`module` are provided (an injected native `__s2require` + a JS-constructed `module` object, or all-JS). **This is the load-bearing mechanism for §6 API injection.**
- [ ] **Step 2: Prove the context embedder slot + current-context read.** In a scratch test: create two `v8::Context`s in one isolate; on each, set embedder-data slot 0 to a distinct value (a `v8::Integer` plugin-index, or an external). From inside a `FunctionCallback` native installed on both, read `scope.get_current_context()` → slot 0 → confirm it returns the owning context's value when the native is called from each context. Record the exact rusty_v8 calls: `Context::set_embedder_data(scope, index, value)` / `get_embedder_data` (or `set_slot`/`get_slot`), and `HandleScope::get_current_context()` / `PinScope::get_current_context()`. Confirm a `Global<Context>` can be created, entered via `ContextScope`, and dropped (dispose).
- [ ] **Step 3: Write the findings doc** answering: the CJS wrapper string + require/module provisioning; the embedder-slot set/get calls; the current-context read from a native; `Global<Context>` create/enter/dispose; and any surprise (e.g. esbuild cjs interop `__esModule`/`exports` shape). Tag each **[HC] proven** (a scratch test/bundle passed) or **[risk]** (needs care in the refactor).
- [ ] **Step 4: Commit** (delete scratch first): `git add docs/superpowers/specs/2026-07-01-slice-4-spike-findings.md && git commit -m "docs(slice4): spike — CJS eval wrapper + rusty_v8 context embedder slot"`.

---

### Task 2: `plugin.rs` — the V8-free registry, ledger, generation & teardown logic

**Purpose:** The pure lifecycle logic (no V8): a registry of plugin instances, per-plugin ledgers of resource ids, a generation counter, the reverse-walk teardown order, and the async-liveness predicate. Fully unit-tested.

**Files:**
- Create: `core/src/plugin.rs`
- Modify: `core/src/lib.rs` (`mod plugin;`)
- Test: `core/src/plugin.rs` `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub struct PluginLedger { pub hook_subs: Vec<u64>, pub timers: Vec<u64>, pub jobs: Vec<u64> }` with `new()`, `record_hook(id)`, `record_timer(id)`, `record_job(id)`, and `pub fn teardown_order(&self) -> Vec<Resource>` returning resources in REVERSE acquisition order.
  - `pub enum Resource { Hook(u64), Timer(u64), Job(u64) }`
  - `pub struct Registry` mapping `plugin_id: String -> PluginEntry { generation: u64, ledger: PluginLedger }` (the V8 `context` lives in `v8host`, keyed by the same id) with: `insert(id) -> u64` (returns the assigned generation; a re-insert of an existing id bumps the generation — that IS reload), `remove(id) -> Option<PluginEntry>`, `is_live(id, generation) -> bool` (present AND generation matches), `ledger_mut(id) -> Option<&mut PluginLedger>`, `ids() -> Vec<String>`.
- Consumes: nothing (pure logic).

- [ ] **Step 1: Write the failing tests** (`core/src/plugin.rs` `#[cfg(test)]`):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teardown_is_reverse_acquisition_order() {
        let mut l = PluginLedger::new();
        l.record_hook(1); l.record_timer(2); l.record_job(3); l.record_hook(4);
        // reverse of [Hook(1),Timer(2),Job(3),Hook(4)]:
        assert_eq!(l.teardown_order(),
            vec![Resource::Hook(4), Resource::Job(3), Resource::Timer(2), Resource::Hook(1)]);
    }

    #[test]
    fn insert_assigns_and_reload_bumps_generation() {
        let mut r = Registry::new();
        let g1 = r.insert("a");
        assert!(r.is_live("a", g1));
        let g2 = r.insert("a");                 // reload
        assert_ne!(g1, g2);
        assert!(!r.is_live("a", g1), "old generation is stale after reload");
        assert!(r.is_live("a", g2));
    }

    #[test]
    fn remove_makes_it_not_live_and_returns_ledger() {
        let mut r = Registry::new();
        let g = r.insert("a");
        r.ledger_mut("a").unwrap().record_timer(7);
        let entry = r.remove("a").expect("present");
        assert_eq!(entry.ledger.timers, vec![7]);
        assert!(!r.is_live("a", g), "removed plugin is not live");
        assert!(r.remove("a").is_none());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core plugin:: -- --test-threads=1`
Expected: FAIL — `PluginLedger`/`Registry` undefined.

- [ ] **Step 3: Implement `plugin.rs`.** `PluginLedger` pushes ids onto the three `Vec`s in acquisition order; `teardown_order` interleaves them into one `Vec<Resource>` in the exact reverse of a single acquisition sequence — track a single `order: Vec<Resource>` on the ledger (push each resource as recorded) and return `order.iter().rev().cloned().collect()`. (Keep `hook_subs`/`timers`/`jobs` as convenience accessors if useful, but `order` drives teardown.) `Registry` uses a `HashMap<String, PluginEntry>` + a monotonic `next_gen: u64`; `insert` sets `entry.generation = next_gen; next_gen += 1`; `is_live` checks `get(id).map_or(false, |e| e.generation == gen)`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core plugin:: -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add core/src/plugin.rs core/src/lib.rs
git commit -m "feat(core): plugin registry + ledger + generation + reverse teardown (V8-free)"
```

---

### Task 3: Multiplexer — tag each subscription with an owner plugin id

**Purpose:** So dispatch can enter the owning plugin's context and teardown can remove a plugin's subscriptions. Keeps the Slice-1 contract; adds an owner tag.

**Files:**
- Modify: `core/src/multiplexer.rs`
- Test: `core/src/multiplexer.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: the existing `Descriptor<H>` / `Subscription` / `subscribe` / `snapshot` (Slice 1).
- Produces: `subscribe(priority, phase, owner: String, handler) -> (SubId, DetourChange)`; `snapshot(phase) -> Vec<(SubId, Priority, String /*owner*/, H)>`; `remove_by_owner(&mut self, owner: &str) -> DetourChange` (unsubscribe all of a plugin's subs at once, recomputing the detour).

- [ ] **Step 1: Write the failing test** (`multiplexer.rs` `#[cfg(test)]`):
```rust
    #[test]
    fn remove_by_owner_drops_that_plugins_subs_only() {
        let mut d = Descriptor::<Mock>::new();
        d.subscribe(Priority::Normal, Phase::Pre, "a".into(), Mock { tag: "a1", ret: HookResult::Continue });
        d.subscribe(Priority::Normal, Phase::Pre, "b".into(), Mock { tag: "b1", ret: HookResult::Continue });
        d.subscribe(Priority::Normal, Phase::Pre, "a".into(), Mock { tag: "a2", ret: HookResult::Continue });
        d.remove_by_owner("a");
        let snap = d.snapshot(Phase::Pre);
        let tags: Vec<_> = snap.iter().map(|(_, _, owner, h)| (owner.clone(), h.tag)).collect();
        assert_eq!(tags, vec![("b".to_string(), "b1")], "only b's sub remains, still owner-tagged");
    }
```
> Update the existing multiplexer tests' `subscribe(...)` calls to pass an owner (e.g. `"test".into()`) and the `snapshot` destructuring to the 4-tuple — they must stay green.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core multiplexer:: -- --test-threads=1`
Expected: FAIL — `subscribe`/`snapshot` arity changed / `remove_by_owner` missing.

- [ ] **Step 3: Implement.** Add `owner: String` to `Subscription`; thread it through `subscribe`; include it in `snapshot`'s tuple; add `remove_by_owner` (retain subs whose `owner != owner`, recompute `enabled_count`, return the `DetourChange` the same way `apply_errors`/unsubscribe does).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core multiplexer:: -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add core/src/multiplexer.rs
git commit -m "feat(core): tag OnGameFrame subscriptions with an owner plugin id + remove_by_owner"
```

---

### Task 4: v8host refactor A — isolate + plugin-context registry + embedder slot

**Purpose:** Split the single `HOST { isolate, context }` into a shared isolate + a per-plugin context registry; create/dispose contexts; stamp each context with its plugin id (embedder slot) and read it back from a native. Engine-integration — the pure parts of the model are testable; exact V8 calls come from the Task-1 spike.

**Files:**
- Modify: `core/src/v8host.rs`, `core/src/ffi.rs`, `core/src/lib.rs`
- Test: `core/src/v8host.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1 spike (embedder-slot + current-context calls); Task 2 `Registry`.
- Produces (crate-internal):
  - `ISOLATE` holding the single `OwnedIsolate` (was `HOST.isolate`); `PLUGINS: RefCell<HashMap<String, v8::Global<v8::Context>>>` (the contexts, keyed by id) alongside a `REGISTRY: RefCell<plugin::Registry>` (Task 2).
  - `pub(crate) fn create_plugin_context(id: &str) -> u64` — creates a fresh `v8::Context`, sets embedder slot 0 to the plugin id (via an index into a side table, since embedder data is a V8 value — store `id → small u32 index` and stash the index as a `v8::Integer`), installs the API (Task 5 fills this), inserts into `PLUGINS` + `REGISTRY` (returns the generation).
  - `pub(crate) fn dispose_plugin_context(id: &str)` — drop the `Global<Context>` (after the ledger walk has dropped all `Global`s into it) and remove from `PLUGINS`.
  - `pub(crate) fn current_plugin(scope) -> Option<String>` — read `get_current_context()`'s embedder slot → the id.
  - Test helper `eval_in_context(id, src)` (create-if-absent + eval `src` in that context) for the integration tests + the reworked Slice 0–3 tests.

- [ ] **Step 1: Write the failing integration test:**
```rust
    #[test]
    fn two_contexts_have_distinct_plugin_identity() {
        init(dummy_logger()).unwrap();
        create_plugin_context("alpha");
        create_plugin_context("beta");
        // A tiny probe native reads current_plugin() and stashes it on the context global.
        eval_in_context("alpha", "globalThis.__who = __s2_current_plugin();").unwrap();
        eval_in_context("beta",  "globalThis.__who = __s2_current_plugin();").unwrap();
        assert_eq!(read_string_global_in("alpha", "__who"), "alpha");
        assert_eq!(read_string_global_in("beta",  "__who"), "beta");
        dispose_plugin_context("alpha");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("alpha")));
        shutdown();
    }
```
> Provide `__s2_current_plugin()` (a native returning `current_plugin(scope)` as a JS string) and `read_string_global_in(id, name)` (enter the id's context, read the global). These natives/helpers are the test surface for the identity model.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core two_contexts_have_distinct_plugin_identity -- --test-threads=1`
Expected: FAIL — `create_plugin_context`/`current_plugin` undefined.

- [ ] **Step 3: Implement refactor A** per the Task-1 spike: move the isolate to `ISOLATE`; add `PLUGINS` + `REGISTRY`; implement `create_plugin_context` (context + embedder-slot id-index + register), `dispose_plugin_context`, `current_plugin`, `eval_in_context`, and the `__s2_current_plugin` probe native (installed by Task 5's per-context install; for this task a minimal install path is fine). Keep `init` (isolate + platform + policy) and `shutdown` (dispose all contexts + clear registry) working. Use a `RefCell<HashMap<u32,String>>` id-table since V8 embedder data holds a value, not a Rust `String`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core two_contexts_have_distinct_plugin_identity -- --test-threads=1`
Expected: PASS. Also `cargo build -p s2script-core` links.

- [ ] **Step 5: Commit**
```bash
git add core/src/v8host.rs core/src/ffi.rs core/src/lib.rs
git commit -m "feat(core): per-plugin v8::Context registry + embedder-slot plugin identity"
```

---

### Task 5: v8host refactor B — per-context API install, the naming rename & the CJS require-shim

**Purpose:** Move the natives + a renamed, reshaped PRELUDE into `create_plugin_context`, expose them as the injected `@s2script/std` + `@s2script/cs2` API via a `require` shim, and route every subscription/timer to the calling plugin's ledger. Reworks the Slice 0–3 tests onto the new API + plugin-context harness.

**Files:**
- Modify: `core/src/v8host.rs` (PRELUDE → per-context injected API; natives tag by `current_plugin`); `games/cs2/js/pawn.js` → the injected `@s2script/cs2` (`Pawn.forSlot`); the Slice 0–3 `#[cfg(test)]` tests (renamed API + `eval_in_context`).
- Test: reworked existing integration tests + a new CJS-load test.

**Interfaces:**
- Consumes: Task 1 (CJS wrapper), Task 4 (`create_plugin_context`, `current_plugin`), Task 2 (`Registry::ledger_mut`).
- Produces: `pub(crate) fn load_plugin_js(id: &str, plugin_js: &str)` — create the context, install the API, eval the CJS bundle wrapped `(function(require,module,exports){...})(s2require, module, module.exports)`, capture `module.exports` (`onLoad`/`onUnload`) as `Global<Function>`s stored on the `PluginInstance`, call `onLoad`. The injected `s2require("@s2script/std")` → `{ OnGameFrame, delay, nextTick, nextFrame, threadSleep, console }`; `s2require("@s2script/cs2")` → `{ Pawn }`.

- [ ] **Step 1: Write the failing test** (a CJS bundle string standing in for a built plugin.js):
```rust
    #[test]
    fn load_plugin_js_runs_onload_and_tags_subscription() {
        init(dummy_logger()).unwrap();
        // Minimal CJS bundle: require the injected API, subscribe, export onLoad.
        let plugin_js = r#"
            const { OnGameFrame, delay } = require("@s2script/std");
            module.exports.onLoad = function () {
                OnGameFrame.subscribe(function () { globalThis.__ticks = (globalThis.__ticks||0)+1; });
            };
        "#;
        load_plugin_js("demo", plugin_js);
        // One frame → the demo's handler ran, tagged to "demo".
        dispatch_game_frame_pre_post();  // helper: Pre then Post dispatch (drives the multiplexer)
        assert_eq!(read_i32_global_in("demo", "__ticks"), 1);
        // The subscription is owned by "demo":
        assert!(FRAME.with(|f| f.borrow().snapshot(Phase::Pre).iter().any(|(_,_,owner,_)| owner=="demo")));
        shutdown();
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core load_plugin_js_runs_onload_and_tags_subscription -- --test-threads=1`
Expected: FAIL — `load_plugin_js` undefined; `require` unavailable.

- [ ] **Step 3: Implement refactor B.** Per-context install (in `create_plugin_context`): install all the natives (`__s2_subscribe`/`__s2_delay`/… — unchanged internals) on the context global, plus `__s2require` (a native mapping `"@s2script/std"`/`"@s2script/cs2"` to per-context API objects built by an injected prelude). The injected prelude (evaluated per-context) defines `OnGameFrame = { subscribe: (fn, opts) => ({ dispose: () => __s2_unsubscribe(__s2_subscribe("OnGameFrame", fn, opts||{})) }) }` — actually capture the id: `subscribe: (fn,opts) => { const id = __s2_subscribe("OnGameFrame", fn, opts||{}); return { dispose: () => __s2_unsubscribe(id) }; }` — and `delay = (ms)=>__s2_delay(ms||0)`, `nextTick`, `nextFrame`, `threadSleep`, `console`. The natives now read `current_plugin(scope)` to (a) pass the owner to `FRAME.subscribe(..., owner, ...)` and (b) `REGISTRY.ledger_mut(owner).record_hook/timer/job(id)`. `load_plugin_js` wraps + evals the CJS bundle (Task-1 shape), captures `module.exports`. Move `games/cs2/js/pawn.js`'s `pawnForSlot` logic into the injected `@s2script/cs2` `Pawn` object (`Pawn.forSlot(slot)` → the Slice-3 walk; `pawn.health` unchanged). **Remove** the old single-context PRELUDE + `load_cs2_file` path (subsumed).

- [ ] **Step 4: Rework the Slice 0–3 integration tests** onto the new API + harness: replace `eval(...)` + `onGameFrame`/`Delay` with `load_plugin_js`/`eval_in_context` + `OnGameFrame.subscribe`/`delay`. Run the FULL suite: `cargo test -p s2script-core -- --test-threads=1` → all green (Slice 0–3 behavior preserved under the new names/model + the new load test).

- [ ] **Step 5: Verify + commit**

Run: `cargo build -p s2script-core && cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh`
Expected: builds; all green; `core boundary OK`.
```bash
git add core/src/v8host.rs games/cs2/js/pawn.js
git commit -m "feat(core): per-context injected API (@s2script/std+cs2) via CJS require-shim; rename to OnGameFrame.subscribe/delay"
```

---

### Task 6: v8host refactor C — per-handler dispatch context + per-context drain + the async-liveness guard

**Purpose:** Run each `OnGameFrame` handler in its owning context; resolve each timer/job in its owning context; and DROP any continuation whose plugin is gone or reloaded (the use-after-free killer). Wire teardown (`unload_plugin`) to the ledger reverse-walk.

**Files:**
- Modify: `core/src/v8host.rs` (`dispatch_onframe`, `frame_async_drain`, add `unload_plugin`)
- Test: `core/src/v8host.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 2 (`is_live`, `teardown_order`, `remove`), Task 3 (`snapshot` owner, `remove_by_owner`), Tasks 4–5.
- Produces: `pub(crate) fn unload_plugin(id: &str)` — mark unloading; call `onUnload` best-effort; `FRAME.remove_by_owner(id)` (→ refresh_detour); for each `Resource` in `REGISTRY.remove(id).ledger.teardown_order()` cancel timers / drop job+timer resolvers; `dispose_plugin_context(id)`. RESOLVERS entries carry `(plugin_id, generation)`; `frame_async_drain` checks `REGISTRY.is_live(id, gen)` before resolving, else drops the resolver.

- [ ] **Step 1: Write the failing tests:**
```rust
    #[test]
    fn unload_removes_the_plugins_hook_and_disposes_context() {
        init(dummy_logger()).unwrap();
        load_plugin_js("demo", r#"const {OnGameFrame}=require("@s2script/std");
            module.exports.onLoad=()=>OnGameFrame.subscribe(()=>{globalThis.__n=(globalThis.__n||0)+1;});"#);
        dispatch_game_frame_pre_post();
        unload_plugin("demo");
        dispatch_game_frame_pre_post();            // demo's handler must NOT run now
        assert!(!FRAME.with(|f| f.borrow().snapshot(Phase::Pre).iter().any(|(_,_,o,_)| o=="demo")));
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("demo")), "context disposed");
        shutdown();
    }

    #[test]
    fn delay_continuation_for_unloaded_plugin_is_dropped() {
        init(dummy_logger()).unwrap();
        load_plugin_js("demo", r#"const {delay}=require("@s2script/std");
            module.exports.onLoad=()=>{ (async()=>{ await delay(30); globalThis.__resumed=true; })(); };"#);
        unload_plugin("demo");                     // unload BEFORE the deadline
        std::thread::sleep(std::time::Duration::from_millis(40));
        frame_async_drain();                       // must NOT run the continuation into a disposed context
        // The plugin/context is gone; nothing to read — assert no panic + the resolver was dropped:
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("demo")));
        shutdown();
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core -- --test-threads=1` (the two new tests)
Expected: FAIL — `unload_plugin` undefined / continuation runs / panic.

- [ ] **Step 3: Implement refactor C.** `dispatch_onframe`: for each `(SubId, Priority, owner, handler)` in the snapshot, enter `PLUGINS[owner]`'s context (a `ContextScope`) before `handler.call(...)` (skip if the owner is not live). `frame_async_drain`: RESOLVERS now maps `id -> (owner, generation, Global<PromiseResolver>)`; when a timer/job is due, look up its `(owner, generation)`, `if !REGISTRY.is_live(owner, generation) { drop the resolver; continue; }` else enter the owner's context, resolve, and run the checkpoint (the continuation runs in that context). `unload_plugin` as specified. Keep the Slice-1/2 re-entrancy discipline (no `FRAME`/`RESOLVERS`/`TIMERS` borrow held across a JS call / checkpoint).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: PASS (both new tests + all prior). `cargo build` links; boundary green.

- [ ] **Step 5: Commit**
```bash
git add core/src/v8host.rs
git commit -m "feat(core): per-context dispatch/drain + async-liveness guard + ledger teardown (unload_plugin)"
```

---

### Task 7: `loader.rs` — `.s2sp` read, manifest validate, `/plugins` watch, load/unload/reload

**Purpose:** Watch the plugins dir on the frame drain, read `.s2sp` archives in-memory, validate the derived manifest, and drive `load_plugin_js`/`unload_plugin`. Remove the baked demo from the shim.

**Files:**
- Create: `core/src/loader.rs`
- Modify: `core/src/lib.rs`, `core/src/v8host.rs` (call the watch from `frame_async_drain`'s Post path or a dedicated tick), `core/src/ffi.rs` + `shim/include/s2script_core.h` + `shim/src/s2script_mm.cpp` (pass the plugins-dir path via `dladdr`; REMOVE the baked demo `eval` + `load_cs2`), `core/Cargo.toml` (a small zip-reading crate, e.g. `zip`)
- Test: `core/src/loader.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `v8host::{load_plugin_js, unload_plugin}`, Task 2/6.
- Produces: `pub fn set_plugins_dir(path)`, `pub fn poll_plugins()` (called each Post drain, throttled), and `pub fn read_s2sp(bytes) -> Result<(Manifest, String /*plugin.js*/), String>` (unzip in-memory; parse+validate `manifest.json`).

- [ ] **Step 1: Write the failing test** (the pure `.s2sp` read/validate is unit-testable with an in-memory zip):
```rust
    #[test]
    fn read_s2sp_extracts_manifest_and_plugin_js() {
        // Build an in-memory .s2sp: zip { manifest.json, plugin.js }.
        let bytes = make_test_s2sp(
            r#"{"id":"@demo/hello","version":"0.1.0","apiVersion":"1.x"}"#,
            "module.exports.onLoad=()=>{};");
        let (m, js) = read_s2sp(&bytes).expect("valid s2sp");
        assert_eq!(m.id, "@demo/hello");
        assert!(js.contains("onLoad"));
    }

    #[test]
    fn read_s2sp_rejects_missing_manifest_named() {
        let bytes = make_test_s2sp_missing_manifest("module.exports={};");
        assert!(read_s2sp(&bytes).is_err(), "a .s2sp without manifest.json is rejected with a reason");
    }
```
> Provide `make_test_s2sp(manifest_json, plugin_js)` (writes a zip to a `Vec<u8>` via the `zip` crate) in the test module.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core loader:: -- --test-threads=1`
Expected: FAIL — `read_s2sp` undefined.

- [ ] **Step 3: Implement.** `read_s2sp`: open the zip from bytes, read `manifest.json` (serde into `Manifest { id, version, apiVersion, .. }`; reject on missing/parse error with a named `Err`), read `plugin.js` as a String. `poll_plugins`: keep a `HashMap<PathBuf, mtime>`; each throttled call, diff the dir listing: new/changed → `read_s2sp` + (unload existing id if reload) + `load_plugin_js`; vanished → `unload_plugin`. Degrade-never-crash on any read/parse error (named WARN, keep serving). Wire `poll_plugins` into the Post drain (throttle ~1s). Shim: pass the plugins-dir path; delete the baked demo `eval` + `load_cs2`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh`
Expected: PASS; boundary green. `cargo build` links.

- [ ] **Step 5: Commit**
```bash
git add core/src/loader.rs core/src/lib.rs core/src/v8host.rs core/src/ffi.rs core/Cargo.toml shim/include/s2script_core.h shim/src/s2script_mm.cpp
git commit -m "feat(core): .s2sp loader + /plugins watch (load/reload/unload); remove baked shim demo"
```

---

### Task 8: `@s2script/cli` (TS, npx) + `@s2script/std`/`@s2script/cs2` type stubs

**Purpose:** `s2script build <plugin-dir>` → a `.s2sp`. TypeScript CLI (esbuild-built `bin`, `npx`-invokable) that bundles the plugin (esbuild, cjs, `@s2script/*` external), derives `manifest.json`, and zips. Plus minimal author-time type-stub packages.

**Files:**
- Create: `packages/cli/` (`package.json` name `@s2script/cli`, `bin: { "s2script": "dist/cli.js" }`, `src/cli.ts`, `src/build.ts`, `tsconfig.json`), `packages/std/` (`package.json` name `@s2script/std`, `index.d.ts`), `packages/cs2/` (`package.json` name `@s2script/cs2`, `index.d.ts`)
- Test: `packages/cli/test/build.test.mjs` (a node test building a fixture)

**Interfaces:**
- Produces: `npx s2script build <dir>` writing `<dir>/dist/<id>.s2sp`. The `.s2sp` matches Task 7's `read_s2sp` (zip with `manifest.json` + cjs `plugin.js`).

- [ ] **Step 1: Write the failing test** (`packages/cli/test/build.test.mjs`, node's built-in test runner):
```js
import { test } from "node:test";
import assert from "node:assert";
import { buildPlugin } from "../src/build.ts";   // run via tsx/esbuild-register, or point at dist
import { readFileSync } from "node:fs";
import AdmZip from "adm-zip";                     // or `unzipper`; a dev-dep of the CLI

test("build produces a .s2sp with derived manifest + cjs plugin.js", async () => {
  const out = await buildPlugin("test/fixtures/hello");   // a fixture plugin dir
  const zip = new AdmZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  assert.equal(manifest.id, "@demo/hello");
  assert.ok(manifest.apiVersion);
  const js = zip.readAsText("plugin.js");
  assert.ok(js.includes("require(\"@s2script/std\")"), "@s2script/* left external as cjs require");
});
```
> Include `test/fixtures/hello/` (`package.json` with `name:"@demo/hello"`, `s2script.apiVersion:"1.x"`, `main:"src/plugin.ts"`; `src/plugin.ts` importing from `@s2script/std`).

- [ ] **Step 2: Run to verify it fails**

Run: `cd packages/cli && npm install && npm test`
Expected: FAIL — `buildPlugin` undefined.

- [ ] **Step 3: Implement the CLI.** `src/build.ts` `buildPlugin(dir)`: read `dir/package.json`; run esbuild (`build({ entryPoints:[main], bundle:true, platform:"neutral", format:"cjs", external:["@s2script/std","@s2script/cs2"], target:"es2020", write:false })`) → the bundled cjs string; derive `manifest.json` (`{ id:name, version, apiVersion:s2script.apiVersion, pluginDependencies:s2script.pluginDependencies||{}, publishes:s2script.publishes }`); zip `{manifest.json, plugin.js}` → `dir/dist/<sanitized-id>.s2sp`; return the path. `src/cli.ts`: arg-parse `s2script build <dir>` → `buildPlugin`. Build the CLI itself with esbuild to `dist/cli.js` (a `prepare`/`build` npm script). Write `packages/std/index.d.ts` + `packages/cs2/index.d.ts` declaring the injected API types (`OnGameFrame.subscribe`, `delay`, `Pawn.forSlot`, `pawn.health`).

- [ ] **Step 4: Run to verify it passes**

Run: `cd packages/cli && npm test`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add packages/
git commit -m "feat(cli): @s2script/cli (s2script build → .s2sp via esbuild) + @s2script/std+cs2 type stubs"
```

---

### Task 9: Demo plugin + live gate + README + CLAUDE.md

**Purpose:** Prove the milestone on a live CS2 server: build the demo `.s2sp`, drop/reload/delete, observe load → hot-reload → clean teardown. Docs. Controller-driven (Claude drives the container), like prior live gates.

**Files:**
- Create: `examples/demo-plugin/` (`package.json`, `src/plugin.ts`)
- Modify: `README.md`, `CLAUDE.md` (Current state)

**Interfaces:**
- Consumes: everything (Tasks 1–8).

- [ ] **Step 1: Write the demo plugin.** `examples/demo-plugin/package.json` (`name:"@demo/hello"`, `main:"src/plugin.ts"`, `s2script:{apiVersion:"1.x", pluginDependencies:{"@s2script/std":"^1.0.0","@s2script/cs2":"^1.0.0"}}`). `src/plugin.ts`:
```ts
import { OnGameFrame, delay } from "@s2script/std";
import { Pawn } from "@s2script/cs2";
let n = 0;
export function onLoad() {
  console.log("[demo] onLoad");
  OnGameFrame.subscribe(() => { if (n++ % 256 === 0) { const p = Pawn.forSlot(0); console.log("[demo] tick " + n + " hp=" + (p ? p.health : "none")); } });
  (async () => { console.log("[demo] before delay"); await delay(1000); console.log("[demo] after delay(1000)"); })();
}
export function onUnload() { console.log("[demo] onUnload"); }
```

- [ ] **Step 2: Build the CLI + the demo `.s2sp`, sniper build, recreate the container.**
```bash
(cd packages/cli && npm install && npm run build)
node packages/cli/dist/cli.js build examples/demo-plugin       # → examples/demo-plugin/dist/@demo-hello.s2sp
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
# ensure the server plugins dir is mounted/created (addons/s2script/plugins/), re-patch gameinfo if the image updated:
bash docker/patch-gameinfo.sh docker/cs2-data/game/csgo/gameinfo.gi || true
docker compose -f docker/docker-compose.yml up -d --force-recreate cs2
```
Wait for load (poll for the MM:S plugin `[s2script]` banner).

- [ ] **Step 3: Live gate — drop / reload / delete.** Copy the `.s2sp` into the server's `addons/s2script/plugins/`; observe `[demo] onLoad` + `[demo] tick …` + `[demo] after delay(1000)`. Edit `src/plugin.ts` (change the log), rebuild, re-copy; observe the old instance's `[demo] onUnload` then the new `[demo] onLoad` (hot-reload, no restart). Delete the `.s2sp`; observe `[demo] onUnload`, no more ticks, no crash. **Acceptance to record (spec §10):** load, hot-reload, clean teardown, async-liveness (delete while `delay` pending → no post-teardown log/crash), all live.

- [ ] **Step 4: README + CLAUDE.md.** Add a "Plugin lifecycle (Slice 4)" README section (the `s2script build` → drop → reload → delete runbook + the live evidence + a Slice-4 acceptance table). Update `CLAUDE.md`'s "## Current state" to reflect Slices 0–4 complete and Slice 4.5/5 next (remove the stale "Do not build past Slice 0").

- [ ] **Step 5: Commit + stop the container.**
```bash
git add examples/ README.md CLAUDE.md
git commit -m "docs+demo: Slice 4 live gate (.s2sp load/hot-reload/teardown) + acceptance; update CLAUDE.md state"
docker stop s2script-cs2 && docker rm s2script-cs2
```

---

## Self-Review (completed during planning)

- **Spec coverage:** §1 thesis → all tasks. §2.1/§3 context-per-plugin → T4/T5/T6. §2.5/§4 ledger+teardown+liveness → T2/T6. §5 loader/watch/reload → T7. §6 build/.s2sp/injection → T1(spike)/T5(require-shim)/T8(cli). §7 rename → T5. §8 demo+live → T9. §9 testing → each task's tests + T9 live. §10 acceptance → T2–T9. §11 out-of-scope honored (no tsc gate, no inter-plugin, no EntityRef, no config/permissions/state-handoff/topo-sort). §12 files → matches. §13 open items → T1 spike resolves them, cited by T4–T7. CLAUDE.md update → T9.
- **Placeholder scan:** engine-integration steps (T4–T6) delegate exact V8 calls to the T1 spike + the existing v8host patterns (not vague "handle it") — the V8-free logic (T2 ledger/registry, T3 multiplexer, T7 read_s2sp, T8 build) has complete code + tests. No "TBD".
- **Type consistency:** `PluginLedger`/`Registry`/`Resource` (T2) used by T6/T7; `subscribe(...owner...)`/`snapshot → 4-tuple`/`remove_by_owner` (T3) used by T5/T6; `create_plugin_context`/`current_plugin`/`dispose_plugin_context` (T4) used by T5/T6; `load_plugin_js`/`unload_plugin` (T5/T6) used by T7; `read_s2sp`/`Manifest` (T7) matches the CLI's `.s2sp` (T8). The `(plugin_id, generation)` resolver tag is consistent T2↔T6. Injected API names (`OnGameFrame.subscribe`, `delay`, `Pawn.forSlot`, `pawn.health`) consistent T5/T8/T9.

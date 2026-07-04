# Reload State-Handoff (Slice 5E.3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On a same-id hot-reload, carry runtime state from the old plugin instance to the new one — `onUnload(): State` return is held across the teardown→load gap and passed to `onLoad(prev)`.

**Architecture:** Entirely in-core, reusing the existing inter-plugin marshalling. `unload_plugin` serializes `onUnload()`'s return in the old context via `iface_to_json` (`JSON.stringify` + the EntityRef replacer) into a host-side `PENDING_HANDOFF: {id → String}`; `load_plugin_js` consumes it via `iface_from_json` (`JSON.parse` + the EntityRef reviver) and passes it to `onLoad(prev)`. The loader's Vanished path clears it; shutdown resets it.

**Tech Stack:** Rust (`rusty_v8`), the core cdylib. No shim/C++/native change. TypeScript demo (esbuild). Docker CS2 live gate.

## Global Constraints

- **Core is engine-generic.** `core/src/{v8host.rs,loader.rs}` name no CS2/game symbol; the demo (CS2) owns game facts. CI gates `scripts/check-core-boundary.sh` + `scripts/test-boundary-nameleak.sh` must stay green.
- **Reuse the existing marshalling — do NOT write a new serializer.** `iface_to_json(scope, value) -> Option<String>` (JSON.stringify + `__s2_entref_replacer`; non-serializable → `None`) and `iface_from_json(scope, json) -> Option<Local<Value>>` (JSON.parse + `__s2_entref_reviver` → live serial-gated EntityRefs) already exist in `core/src/v8host.rs` and both accept a `TryCatch` (`tc`) as the scope. The per-context replacer/reviver are installed by the prelude in every plugin context.
- **Degrade-never-crash.** `onUnload()` throws → no capture (existing WARN); non-serializable return → `None` + a named WARN, no capture; revival fails → `onLoad(undefined)`; `onLoad(prev)` throws → the existing onLoad WARN; an `EntityRef` whose entity died in the gap → serial-gated `null`. No path throws into the engine.
- **Consume-once.** On load the blob is `remove`d from `PENDING_HANDOFF` whether or not revival succeeds — a stale blob never reaches a later load.
- **Handoff only on a same-id Reload.** First **Load** → `onLoad(undefined)`. A **Vanished** (final removal) discards any captured blob (`clear_pending_handoff`). `shutdown` resets the map.
- **No shim/native/op change.** One sniper rebuild (core `.so` only) at Task 3.
- **Plugins are pure ESM** (the 5E.1 typecheck gate); the demo must pass full `strict`.
- **Commit trailer:** every commit message MUST end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`.

---

## File Structure

- `core/src/v8host.rs` — the `PENDING_HANDOFF` thread-local; capture in `unload_plugin`; revive+inject in `load_plugin_js`; `clear_pending_handoff`; the shutdown reset; the in-isolate tests.
- `core/src/loader.rs` — the Vanished branch calls `clear_pending_handoff`.
- `examples/demo-plugin/src/plugin.ts` + `package.json` — the live-gate demo.
- `README.md`, `CLAUDE.md` — docs + live evidence.

---

### Task 1: Core capture + hold + clear

**Files:**
- Modify: `core/src/v8host.rs` (thread-local ~284; `unload_plugin` onUnload site ~3190; new `clear_pending_handoff`; shutdown reset ~3016), `core/src/loader.rs` (Unload action ~280)
- Test: `core/src/v8host.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `iface_to_json(scope, value) -> Option<String>`; the `unload_plugin` teardown; the `Action::Unload` loader branch.
- Produces: `PENDING_HANDOFF: RefCell<HashMap<String,String>>` (thread-local); `pub(crate) fn clear_pending_handoff(id: &str)`; `unload_plugin` now captures `onUnload()`'s return into the map.

- [ ] **Step 1: Add the `PENDING_HANDOFF` thread-local**

In `core/src/v8host.rs`, inside the `thread_local! { … }` block, right after the `CONFIG_SUBS` static (currently ends ~line 284):

```rust
    /// Slice 5E.3: reload state-handoff blobs (id → the JSON string produced by `iface_to_json` in the
    /// OLD context during `onUnload`). Consumed by `load_plugin_js` on the next load of that id (a
    /// Reload) and revived via `iface_from_json`; cleared by the loader on a final removal (Vanished);
    /// reset on `shutdown`. It holds a plain `String`, so it survives the old context's disposal.
    static PENDING_HANDOFF: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
```

- [ ] **Step 2: Write the failing capture test**

In the `v8host.rs` `#[cfg(test)]` module (near the other lifecycle tests), add:

```rust
    /// Slice 5E.3: unload_plugin captures a serializable onUnload() return into PENDING_HANDOFF; a
    /// non-serializable return is dropped with a WARN (no entry); a throwing onUnload leaves no entry.
    #[test]
    fn unload_captures_onunload_return_as_handoff_blob() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // (a) serializable return → captured
        load_plugin_js("cap", r#"
            module.exports.onUnload = function(){ return { count: 7, name: "hi" }; };
        "#, "{}");
        unload_plugin("cap");
        let blob = PENDING_HANDOFF.with(|h| h.borrow().get("cap").cloned());
        let blob = blob.expect("handoff blob captured");
        assert!(blob.contains("\"count\":7"), "blob has the state: {blob}");

        // (b) non-serializable return (a function) → no entry
        load_plugin_js("nos", r#"
            module.exports.onUnload = function(){ return function(){}; };
        "#, "{}");
        unload_plugin("nos");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("nos").is_none()), "non-serializable → no blob");

        // (c) throwing onUnload → no entry
        load_plugin_js("thr", r#"
            module.exports.onUnload = function(){ throw new Error("boom"); };
        "#, "{}");
        unload_plugin("thr");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("thr").is_none()), "throwing onUnload → no blob");
        shutdown();
    }
```

(Harness idiom — every lifecycle test in this module opens with `LOG.lock().unwrap().clear(); init(logger).unwrap();` and closes with `shutdown();`; there is NO `#[serial]` (isolation comes from `--test-threads=1`). Copy the exact setup from a neighbor such as `load_plugin_js_runs_onload_and_tags_subscription`. `logger`/`LOG` are the module's test log fn + buffer; `eval_in_context_string(id, src)` runs `src` in the plugin's context and returns the result string.)

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p s2script-core unload_captures_onunload_return -- --test-threads=1`
Expected: FAIL — `PENDING_HANDOFF` is populated by nobody yet (the blob is `None`).

- [ ] **Step 4: Implement the capture in `unload_plugin`**

In `core/src/v8host.rs`, the onUnload call site (currently ~3190). Replace:

```rust
        if let Some(k) = v8::String::new(tc, "onUnload") {
            if let Some(v) = exports_local.get(tc, k.into()) {
                if let Ok(f) = v8::Local::<v8::Function>::try_from(v) {
                    let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                    if f.call(tc, recv, &[]).is_none() {
                        let msg = tc
                            .exception()
                            .map(|e| e.to_rust_string_lossy(&*tc))
                            .unwrap_or_else(|| "onUnload threw".into());
                        log_warn(&format!("WARN: unload_plugin('{}'): onUnload error: {}", id, msg));
                    }
                }
            }
        }
```

with:

```rust
        if let Some(k) = v8::String::new(tc, "onUnload") {
            if let Some(v) = exports_local.get(tc, k.into()) {
                if let Ok(f) = v8::Local::<v8::Function>::try_from(v) {
                    let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                    match f.call(tc, recv, &[]) {
                        Some(ret) => {
                            // Slice 5E.3: capture the onUnload return as the reload-handoff blob.
                            // Serialize in THIS (old) context via iface_to_json (JSON.stringify + the
                            // EntityRef replacer) so the string survives the context's disposal. A
                            // null/undefined return means "no state to carry"; a non-serializable one
                            // (function, cycle) → iface_to_json None → WARN + no handoff.
                            if !ret.is_undefined() && !ret.is_null() {
                                match iface_to_json(tc, ret) {
                                    Some(blob) => PENDING_HANDOFF.with(|h| {
                                        h.borrow_mut().insert(id.to_string(), blob);
                                    }),
                                    None => log_warn(&format!(
                                        "WARN: unload_plugin('{}'): onUnload return not serializable — no state handoff",
                                        id
                                    )),
                                }
                            }
                        }
                        None => {
                            let msg = tc
                                .exception()
                                .map(|e| e.to_rust_string_lossy(&*tc))
                                .unwrap_or_else(|| "onUnload threw".into());
                            log_warn(&format!("WARN: unload_plugin('{}'): onUnload error: {}", id, msg));
                        }
                    }
                }
            }
        }
```

- [ ] **Step 5: Add `clear_pending_handoff` + the shutdown reset**

Add the fn (place it near `unload_plugin`):

```rust
/// Slice 5E.3: drop any pending reload-handoff blob for `id` WITHOUT consuming it — called by the
/// loader on a FINAL removal (Vanished) so a deleted plugin's captured state is discarded rather than
/// handed to a future re-add of the same id.
pub(crate) fn clear_pending_handoff(id: &str) {
    PENDING_HANDOFF.with(|h| { h.borrow_mut().remove(id); });
}
```

In `shutdown()`, alongside the other mux resets (after the `CONFIG_SUBS` reset ~line 3016):

```rust
    // Reset the reload-handoff map (Slice 5E.3) so a re-init starts clean.
    PENDING_HANDOFF.with(|h| h.borrow_mut().clear());
```

- [ ] **Step 6: Wire the loader Vanished branch**

In `core/src/loader.rs`, the `Action::Unload` branch (~line 280):

```rust
            Action::Unload { path, id } => {
                crate::v8host::unload_plugin(&id);
                crate::v8host::clear_pending_handoff(&id);   // Slice 5E.3: a final removal discards any captured handoff
                removes.push(path);
            }
```

- [ ] **Step 7: Run the test to verify it passes + the full suite + boundary**

Run:
```bash
cargo test -p s2script-core unload_captures_onunload_return -- --test-threads=1
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
Expected: the new test PASSES; full suite green; both gates green.

- [ ] **Step 8: Commit**

```bash
git add core/src/v8host.rs core/src/loader.rs
git commit -m "$(printf 'feat(slice5e3): capture onUnload return into PENDING_HANDOFF + clear\n\nunload_plugin serializes onUnload() return via iface_to_json (JSON.stringify + EntityRef replacer)\ninto a host-side PENDING_HANDOFF map (survives context disposal); non-serializable/throwing -> WARN,\nno capture. clear_pending_handoff (loader Vanished) discards a final-removal blob; shutdown resets.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

### Task 2: Core revive + inject into `onLoad(prev)`

**Files:**
- Modify: `core/src/v8host.rs` (`load_plugin_js` onLoad site ~2723)
- Test: `core/src/v8host.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `PENDING_HANDOFF` + `iface_from_json` (Task 1 + existing); the `load_plugin_js` onLoad call.
- Produces: `load_plugin_js` calls `onLoad(prev)` with the revived blob when one is pending (else `onLoad()`), consuming (removing) the blob whether or not revival succeeds.

- [ ] **Step 1: Write the failing round-trip test**

In the `v8host.rs` `#[cfg(test)]` module:

```rust
    /// Slice 5E.3: a same-id reload carries state — onUnload's return revives into onLoad(prev). Covers
    /// the primitive/nested round-trip, first-load undefined, a live EntityRef revival, and consume-once.
    #[test]
    fn reload_hands_off_state_to_onload_prev() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        // A plugin that seeds a counter from prev on load and bumps it on unload.
        const JS: &str = r#"
            var count = 0;
            module.exports.onLoad   = function(prev){ if (prev) { count = prev.count; }
                                                      globalThis.__count = count;
                                                      globalThis.__hadPrev = (prev !== undefined); };
            module.exports.onUnload = function(){ return { count: count + 1 }; };
        "#;
        // First load → onLoad(undefined)
        load_plugin_js("rh", JS, "{}");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__hadPrev)"), "false", "first load: no prev");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "0");
        // Reload: unload (captures {count:1}) then load again (consumes → onLoad(prev))
        unload_plugin("rh");
        load_plugin_js("rh", JS, "{}");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__hadPrev)"), "true", "reload: prev present");
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "1", "count carried across the reload");
        // Consume-once: the blob is gone, so a fresh load with no new unload sees undefined again.
        unload_plugin("rh");                                   // captures {count:2}
        load_plugin_js("rh", JS, "{}");                        // consumes → count=2
        assert_eq!(eval_in_context_string("rh", "String(globalThis.__count)"), "2");
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("rh").is_none()), "blob consumed");
        shutdown();
    }

    /// Slice 5E.3: an EntityRef in the handoff state revives into a live, serial-gated EntityRef bound
    /// to the NEW context (reusing the inter-plugin reviver).
    #[test]
    fn reload_revives_entityref_in_state() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        const JS: &str = r#"
            module.exports.onLoad   = function(prev){ globalThis.__revived = prev && prev.ref; };
            module.exports.onUnload = function(){ return { ref: new (__s2pkg_entity.EntityRef)(1, 7) }; };
        "#;
        load_plugin_js("er", JS, "{}");
        unload_plugin("er");                                   // captures { ref: <tagged EntityRef> }
        load_plugin_js("er", JS, "{}");                        // revives → live EntityRef
        assert_eq!(eval_in_context_string("er", "String(globalThis.__revived instanceof __s2pkg_entity.EntityRef)"), "true");
        assert_eq!(eval_in_context_string("er", "globalThis.__revived.index + ',' + globalThis.__revived.serial"), "1,7");
        shutdown();
    }

    /// Slice 5E.3: a throwing onLoad(prev) degrades (WARN) without crashing the reload.
    #[test]
    fn reload_onload_throw_degrades_no_crash() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        load_plugin_js("ot", r#"
            module.exports.onLoad   = function(prev){ if (prev) throw new Error("boom"); };
            module.exports.onUnload = function(){ return { x: 1 }; };
        "#, "{}");
        unload_plugin("ot");
        load_plugin_js("ot", r#"
            module.exports.onLoad   = function(prev){ if (prev) throw new Error("boom"); };
            module.exports.onUnload = function(){ return { x: 1 }; };
        "#, "{}");
        // No panic; the blob was consumed despite the throw.
        assert!(PENDING_HANDOFF.with(|h| h.borrow().get("ot").is_none()), "blob consumed even though onLoad threw");
        shutdown();
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core reload_hands_off_state -- --test-threads=1`
Expected: FAIL — `onLoad` is still called with `&[]`, so `__hadPrev` is `false` after the reload and `__count` stays `0`.

- [ ] **Step 3: Implement the revive + inject in `load_plugin_js`**

In `core/src/v8host.rs`, the onLoad call site (~2723). Replace:

```rust
            if let Some(k) = v8::String::new(tc, "onLoad") {
                if let Some(v) = exports.get(tc, k.into()) {
                    if let Ok(f) = v8::Local::<v8::Function>::try_from(v) {
                        let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                        if f.call(tc, recv, &[]).is_none() {
                            let msg = tc
                                .exception()
                                .map(|e| e.to_rust_string_lossy(&*tc))
                                .unwrap_or_else(|| "onLoad threw".into());
                            log_warn(&format!("WARN: load_plugin_js('{}'): onLoad error: {}", id, msg));
                        }
                    }
                }
            }
```

with:

```rust
            if let Some(k) = v8::String::new(tc, "onLoad") {
                if let Some(v) = exports.get(tc, k.into()) {
                    if let Ok(f) = v8::Local::<v8::Function>::try_from(v) {
                        let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                        // Slice 5E.3: consume this id's reload-handoff blob (if a prior unload captured
                        // one) and revive it in THIS (new) context via iface_from_json (JSON.parse + the
                        // EntityRef reviver → live serial-gated refs). Pass it as onLoad's single arg;
                        // consume-once (remove regardless of revival/throw). No blob → onLoad() (prev
                        // is JS `undefined`).
                        let prev = PENDING_HANDOFF.with(|h| h.borrow_mut().remove(id))
                            .and_then(|blob| iface_from_json(tc, &blob));
                        let call_args: Vec<v8::Local<v8::Value>> = match prev {
                            Some(p) => vec![p],
                            None => vec![],
                        };
                        if f.call(tc, recv, &call_args).is_none() {
                            let msg = tc
                                .exception()
                                .map(|e| e.to_rust_string_lossy(&*tc))
                                .unwrap_or_else(|| "onLoad threw".into());
                            log_warn(&format!("WARN: load_plugin_js('{}'): onLoad error: {}", id, msg));
                        }
                    }
                }
            }
```

- [ ] **Step 4: Run to verify it passes + the full suite + boundary**

Run:
```bash
cargo test -p s2script-core reload_ -- --test-threads=1
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
Expected: the three `reload_*` tests PASS; full suite green; both gates green.

- [ ] **Step 5: Commit**

```bash
git add core/src/v8host.rs
git commit -m "$(printf 'feat(slice5e3): revive handoff blob into onLoad(prev) on reload\n\nload_plugin_js consumes PENDING_HANDOFF[id] and revives it via iface_from_json (JSON.parse + EntityRef\nreviver -> live serial-gated refs), passing it as onLoad(prev); consume-once (removed regardless of\nrevival/throw). No blob -> onLoad() (prev undefined). EntityRef in state revives live; onLoad(prev)\nthrow degrades with a WARN.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

### Task 3: Demo + sniper build + live gate + docs

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `examples/demo-plugin/package.json`, `README.md`, `CLAUDE.md`

Controller-driven (the sniper build + Docker server).

- [ ] **Step 1: Demo carries a counter + a tracked pawn ref across reloads**

`examples/demo-plugin/src/plugin.ts` (pure ESM; passes the 5E.1 typecheck gate). It keeps a reload counter and (optionally) a tracked pawn `EntityRef`, seeding both from `prev`:

```typescript
import { EntityRef } from "@s2script/entity";
import { Player } from "@s2script/cs2";

// Slice 5E.3 live gate — reload state-handoff. onUnload returns state (a reload counter + a tracked
// pawn EntityRef); the host carries it across the reload gap; onLoad(prev) restores it. A file edit
// (touch → Reload) increments the counter WITHOUT losing it; the pawn ref survives as a live,
// serial-gated ref (reads null if the entity died during the gap). First load → prev === undefined.
interface State { reloads: number; pawn: EntityRef | null; }

let reloads = 0;
let pawn: EntityRef | null = null;

export function onLoad(prev?: State): void {
  if (prev) { reloads = prev.reloads; pawn = prev.pawn; }
  const health = pawn ? pawn.readInt32(0) : null;   // any read proves the ref is live/serial-gated
  console.log("[demo] onLoad — reloads=" + reloads + " hadPrev=" + (prev !== undefined)
    + " pawnHealth=" + String(health));
  // Track the first live player's pawn so the NEXT reload can prove EntityRef survival.
  const p = Player.all()[0];
  if (p && p.pawn) { pawn = p.pawn.ref; }
}

export function onUnload(): State {
  reloads += 1;
  console.log("[demo] onUnload — handing off reloads=" + reloads);
  return { reloads, pawn };
}
```

(Note: `pawn.readInt32(0)` reads offset 0 purely to exercise a live serial-gated read; the exact value is not asserted — the live gate checks it degrades to `null` after the entity dies, and is a number while alive. If `Player.all()[0].pawn.ref` is the wrong accessor, use whatever the current CS2 package exposes for a pawn's `EntityRef` — confirm against `packages/cs2/index.d.ts`.)

`examples/demo-plugin/package.json` `s2script` block declares the cs2 dependency (the demo imports `@s2script/cs2`):

```json
  "s2script": {
    "apiVersion": "1.x",
    "pluginDependencies": {
      "@s2script/cs2": "^1.0.0"
    }
  }
```

Build: `node packages/cli/dist/cli.js build examples/demo-plugin` (must pass the typecheck gate). Confirm `examples/demo-plugin/dist/_demo_hello.s2sp` is produced.

- [ ] **Step 2: Controller — one sniper build (core only)**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: core + shim build clean (the shim is unchanged; only the core `.so` gains the handoff logic). GLIBC ≤ 2.31.

- [ ] **Step 3: Controller — redeploy + live gate**

```bash
mkdir -p dist/addons/s2script/plugins && cp examples/demo-plugin/dist/*.s2sp dist/addons/s2script/plugins/
docker compose -f docker/docker-compose.yml restart cs2      # re-binds mount + keeps gameinfo
# wait past the boot window; poll for "[demo] onLoad — reloads=0 hadPrev=false"
```
Then force reloads by touching the file (updates mtime → the loader's Reload path):
```bash
touch dist/addons/s2script/plugins/_demo_hello.s2sp
# observe: [demo] onUnload — handing off reloads=1, then [demo] onLoad — reloads=1 hadPrev=true
# repeat once more → reloads=2. Confirm NO reset to 0 (state survives).
```
Expected live evidence: first load `reloads=0 hadPrev=false`; after each touch, `onUnload handing off reloads=N` then `onLoad reloads=N hadPrev=true` (the counter climbs, proving handoff); `pawnHealth` is a number while a bot is alive and `null` after `bot_kick` between reloads (proving the revived EntityRef is live + serial-gated, degrading not crashing). Finally delete the `.s2sp` then re-add it → `onLoad reloads=0 hadPrev=false` (Vanished cleared the pending blob — a fresh identity). Server ticking throughout, `RestartCount=0`. Record the exact lines.

- [ ] **Step 4: Docs + live-gate findings**

- README: a "Reload state-handoff (Slice 5E.3)" section (the `onUnload(): State → onLoad(prev)` shape, the reuse of the inter-plugin copy incl. live EntityRef revival, the Reload-vs-Vanished semantics) + the captured live log.
- CLAUDE.md `## Current state`: append a 5E.3 paragraph + update `Current focus` (config + reload-handoff done; **permissions** the last lifecycle sub-slice remaining).

- [ ] **Step 5: Full sweep + commit**

Run:
```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs && cd -
for g in check-examples-typecheck check-nav-generated check-schema-generated check-events-generated check-core-boundary test-boundary-nameleak; do bash scripts/$g.sh >/dev/null 2>&1 && echo "$g PASS" || echo "$g FAIL"; done
```
Expected: core green; CLI green; all 6 gates PASS.

```bash
git add examples/demo-plugin README.md CLAUDE.md
git commit -m "$(printf 'feat(slice5e3): live gate PASSED — reload state-handoff\n\n<fill with the exact live evidence>\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Self-Review notes (author checklist — completed)

- **Spec coverage:** §1 shape → T1 (onUnload capture) + T2 (onLoad(prev)); §2 mechanism (capture/hold/revive, loader orchestration) → T1 (capture+hold+clear+Vanished) + T2 (revive+consume); §3 State contents (primitives/bigint/nested/EntityRef) → T2 tests; §4 degrade → T1 (non-serializable/throw) + T2 (onLoad throw) tests; §5 boundary (core-only, reuse marshalling) → constraints + no shim change; §6 tests+gate → per-task + T3; §7 tasks → T1–T3 (spec's T1/T2 capture+revive split matches; spec T3 loader folded into T1 Step 6; spec T4 → T3).
- **Type consistency:** `iface_to_json(scope, value) -> Option<String>` / `iface_from_json(scope, json) -> Option<Local>` used identically in T1/T2 as in the existing marshalling. `PENDING_HANDOFF: HashMap<String,String>` keyed by id across T1 (insert/clear/reset) + T2 (remove). `clear_pending_handoff(id)` defined in T1, called in T1 Step 6 (loader). `onUnload(): State`/`onLoad(prev?: State)` shapes consistent across the tests + the demo.
- **No placeholders:** complete Rust for every core step (the exact replace-with blocks at the real line anchors); complete test bodies; the demo carries a note to confirm the pawn-`EntityRef` accessor against `packages/cs2/index.d.ts` (the one genuine game-package lookup, not a placeholder).
- **YAGNI/degrade:** no new serializer (reuse), no shim/op/native, no crash-survival/disk/KV/migration/lifecycle-types (all §8 out-of-scope). Consume-once + Vanished-clear + shutdown-reset prevent stale/leaked blobs.

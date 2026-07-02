# EntityRef on the Inter-Plugin Wire — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an `EntityRef` round-trip across the inter-plugin wire as a *live* `EntityRef` (not plain data), so a producer can hand an entity to a consumer and the consumer's ref validates against the shared entity system — going `null` on entity death.

**Architecture:** Add an `EntityRef`-aware JSON **replacer** (tags `EntityRef` → `{__entref__:[index,serial]}`) and **reviver** (rebuilds `new EntityRef(i,s)` in the target context) to the `@s2script/std` prelude, and wire them into the two existing marshalling helpers `iface_to_json`/`iface_from_json`. All `iface_call`/`iface_emit` sites benefit automatically — they call those two helpers. Structured-copy is preserved (only `{index,serial}` numbers cross).

**Tech Stack:** Rust `cdylib` core (rusty_v8, v8 crate 149.4.0), the `@s2script/std` injected JS prelude, Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-07-01-entityref-wire-design.md`.

## Global Constraints

Every task's requirements implicitly include these (from spec §10):

- **Core stays engine-generic.** No CS2 identifiers, no `include_str!`/`include_bytes!`, no `games/` in `core/src`. The replacer/reviver live in `@s2script/std` (`EntityRef` is engine-generic). Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Structured-copy at the boundary.** Only `{index, serial}` numbers cross (as the tag); no shared object identity. The consumer's `EntityRef` is a fresh copy bound to its own context.
- **Degrade-never-crash.** A missing/non-function replacer/reviver global falls back to plain `stringify`/`parse`; the existing `TryCatch` guards remain; no panic crosses the FFI boundary.
- **`T | null` / host-invalidation preserved.** The wired `EntityRef` is a full `EntityRef` — every access serial-gated; a dead entity reads `null`, never a stale deref.
- **Naming:** PascalCase types (`EntityRef`), camelCase fns/props. **cdylib:** unit tests inline `#[cfg(test)] mod` (no `core/tests/`).
- **Reserved wire key (documented):** `__entref__` — a plain object shaped `{__entref__:[a,b]}` sent as data would be revived as an `EntityRef`. Acceptable for the slice.
- **Commit trailer:** every commit ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-entref-wire`; do not push.

**Deferred — do NOT build:** the raw-live block-scoped fast-path (5A's "5A.1"); non-`i32` field access over a wired ref (5B); write-permission gating (permissions, later); a general typed-token wire for handle types OTHER than `EntityRef`; splitting `@s2script/std` into SourceMod-style modules (5C); the `tsc` gate; 5B/5C; the registry (5.5); the base-plugin suite (6).

---

## File Structure

- **Modify `core/src/v8host.rs`** — add `__s2_entref_replacer`/`__s2_entref_reviver` to `INJECTED_STD_PRELUDE` (between the `EntityRef` definition ~L305 and `globalThis.__s2pkg_std = std;` ~L313); wire `iface_to_json` (~L937) + `iface_from_json` (~L957) to fetch + pass them best-effort; the in-isolate tests in `#[cfg(test)] mod frame_tests`. `iface_call`/`iface_emit` are UNTOUCHED.
- **Create `examples/entref-producer/` + `examples/entref-consumer/`** — the two demo plugins for the live gate.
- **Modify `README.md`, `CLAUDE.md`.**

No new C-ABI, no new natives, no game-package change.

---

## Task 1: The replacer/reviver + marshalling wiring (core, in-isolate cargo)

**Files:**
- Modify: `core/src/v8host.rs` — `INJECTED_STD_PRELUDE` (~L305–313), `iface_to_json` (~L937), `iface_from_json` (~L957), `#[cfg(test)] mod frame_tests`.

**Interfaces:**
- Consumes: the existing `EntityRef` prelude constructor + `isValid`/`readInt32`; the existing `iface_to_json(scope, value) -> Option<String>` / `iface_from_json(scope, json) -> Option<Local<Value>>` shape (both TryCatch-guarded); the `frame_tests` helpers `init`/`set_engine_ops`/`register_injected_package`/`load_plugin_js`/`read_global_string`/`eval_in_context_string`/`shutdown`.
- Produces: `globalThis.__s2_entref_replacer` / `__s2_entref_reviver` (prelude JS); `iface_to_json`/`iface_from_json` now round-trip `EntityRef` as a live ref. No signature changes.

- [ ] **Step 1: Write the failing tests** (append to `#[cfg(test)] mod frame_tests` in `v8host.rs`). All run on the DEGRADE path (`set_engine_ops(None)`), where a *real* `EntityRef` reports `isValid()===false` / `readInt32()===null` — something a plain `{index,serial}` object cannot do, which is exactly what proves rehydration happened:

```rust
    #[test]
    fn iface_call_return_rehydrates_entityref() {
        let _ = init(dummy_logger());
        set_engine_ops(None); // degrade path: a real EntityRef -> isValid()==false, readInt32()==null
        set_plugin_imports("cons", vec![("@x/ent".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        // Producer returns an EntityRef from a method.
        load_plugin_js("prod", r#"
            const { publishInterface, EntityRef } = require("@s2script/std");
            publishInterface("@x/ent", "1.0.0", { getRef: function(){ return new EntityRef(1, 7); } });
        "#);
        // Consumer receives it: must be a LIVE EntityRef (methods present), not plain data.
        load_plugin_js("cons", r#"
            const { EntityRef } = require("@s2script/std");
            const r = require("@x/ent").getRef();
            globalThis.__isRef  = String(r instanceof EntityRef);        // "true" — rehydrated
            globalThis.__idx    = String(r.index) + "," + String(r.serial); // "1,7" — data crossed
            globalThis.__valid  = String(r.isValid());                   // "false" (no ops) — it's callable
            globalThis.__read   = String(r.readInt32(8));                // "null"  (no ops)
        "#);
        assert_eq!(read_global_string("cons", "__isRef"), "true");
        assert_eq!(read_global_string("cons", "__idx"), "1,7");
        assert_eq!(read_global_string("cons", "__valid"), "false");
        assert_eq!(read_global_string("cons", "__read"), "null");
        shutdown();
    }

    #[test]
    fn iface_emit_payload_rehydrates_entityref() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        set_plugin_imports("cons", vec![("@x/ent".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"
            const { publishInterface, EntityRef } = require("@s2script/std");
            globalThis.__h = publishInterface("@x/ent", "1.0.0", { noop: function(){} });
        "#);
        load_plugin_js("cons", r#"
            const { EntityRef } = require("@s2script/std");
            const g = require("@x/ent");
            globalThis.__seen = "none";
            g.on("spawned", function (r) {
                globalThis.__seen = (r instanceof EntityRef) ? (r.index + "," + r.serial) : "plain";
            });
        "#);
        eval_in_context("prod", r#"__h.emit("spawned", new EntityRef(2, 9));"#);
        assert_eq!(read_global_string("cons", "__seen"), "2,9"); // live EntityRef, not "plain"
        shutdown();
    }

    #[test]
    fn non_entityref_payload_round_trips_unchanged() {
        let _ = init(dummy_logger());
        set_plugin_imports("cons", vec![("@x/data".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/std");
            publishInterface("@x/data", "1.0.0", { echo: function(){ return { a: 1, b: "hi", c: [1,2,3] }; } });
        "#);
        load_plugin_js("cons", r#"
            const d = require("@x/data").echo();
            globalThis.__out = d.a + "," + d.b + "," + d.c.join("-");
        "#);
        assert_eq!(read_global_string("cons", "__out"), "1,hi,1-2-3"); // ordinary data intact
        shutdown();
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core frame_tests::iface_call_return_rehydrates_entityref -- --test-threads=1`
Expected: FAIL — today the returned value is a plain object, so `r instanceof EntityRef` is `false` and `r.isValid` is not a function (the `__valid`/`__read` reads throw or mismatch).

- [ ] **Step 3: Add the replacer/reviver to the prelude.** In `INJECTED_STD_PRELUDE`, between `std.EntityRef = EntityRef;` (~L312) and `globalThis.__s2pkg_std = std;` (~L313), insert:

```js
  // Inter-plugin wire tagging: an EntityRef crosses the structured-copy (JSON) boundary as a tagged
  // envelope so the target context rehydrates it into a LIVE EntityRef (bound to ITS natives), not
  // plain data. `__entref__` is a reserved wire key. Used by iface_to_json / iface_from_json.
  globalThis.__s2_entref_replacer = function (key, value) {
    return (value instanceof EntityRef) ? { __entref__: [value.index, value.serial] } : value;
  };
  globalThis.__s2_entref_reviver = function (key, value) {
    return (value && typeof value === "object" && Array.isArray(value.__entref__))
      ? new EntityRef(value.__entref__[0], value.__entref__[1])
      : value;
  };
```

- [ ] **Step 4: Wire `iface_to_json` to pass the replacer (best-effort).** Replace the call line
`let out = strfn.call(tc, recv, &[value])?;` (keep the surrounding TryCatch + the `is_undefined` check) with a best-effort fetch of the replacer from the current context's global:

```rust
    // Best-effort: pass the @s2script/std EntityRef replacer so an EntityRef in `value` crosses the
    // wire as a tagged envelope. Absent (e.g. the shared HOST context) -> plain stringify (no crash).
    let replacer = global
        .get(tc, v8::String::new(tc, "__s2_entref_replacer")?.into())
        .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok());
    let out = match replacer {
        Some(rep) => strfn.call(tc, recv, &[value, rep.into()])?,
        None => strfn.call(tc, recv, &[value])?,
    };
    if out.is_undefined() { return None; }
    Some(out.to_rust_string_lossy(tc))
```

(`global` is already bound at the top of `iface_to_json`; if it was consumed earlier, re-fetch it via `tc.get_current_context().global(tc)`. Fetch the replacer through `tc` so it shares the TryCatch.)

- [ ] **Step 5: Wire `iface_from_json` to pass the reviver (best-effort).** Replace the call line
`parsefn.call(tc, recv, &[arg.into()])` (keep the surrounding TryCatch) with:

```rust
    // Best-effort: pass the reviver so a tagged EntityRef rehydrates into a live ref in THIS context.
    let reviver = global
        .get(tc, v8::String::new(tc, "__s2_entref_reviver")?.into())
        .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok());
    match reviver {
        Some(rev) => parsefn.call(tc, recv, &[arg.into(), rev.into()]),
        None => parsefn.call(tc, recv, &[arg.into()]),
    }
```

(`global` is bound at the top of `iface_from_json`; re-fetch via `tc.get_current_context().global(tc)` if needed. The function returns this `Option<Local<Value>>` directly — it is the last expression.)

- [ ] **Step 6: Run the tests + full suite + gates**

Run: `cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`
Expected: green — the three new tests pass (was 82, now 85); both gates pass (the replacer/reviver are in `@s2script/std`, no CS2 ids).

- [ ] **Step 7: Commit**

```bash
git add core/src/v8host.rs
git commit -m "feat(entref-wire): round-trip EntityRef across the inter-plugin wire (replacer/reviver)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: Demos + cross-plugin host-invalidation LIVE gate + README/CLAUDE (LIVE-ONLY)

**Files:**
- Create: `examples/entref-producer/{package.json, src/plugin.ts}`, `examples/entref-consumer/{package.json, src/plugin.ts, src/entref-iface.d.ts}`.
- Modify: `README.md`, `CLAUDE.md`.

**Interfaces:**
- Consumes: Task 1's wired marshalling; the merged `@s2script/std` (`OnGameFrame`, `publishInterface`, `EntityRef`) + `@s2script/cs2` (`Pawn`).
- Produces: the cross-plugin host-invalidation acceptance proven live.

- [ ] **Step 1: Create the producer `examples/entref-producer`.**

`package.json`:
```json
{
  "name": "@demo/entref-producer",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x", "pluginDependencies": { "@s2script/std": "^1.0.0", "@s2script/cs2": "^1.0.0" }, "publishes": { "@demo/ent": "1.0.0" } }
}
```
`src/plugin.ts` — publish an interface whose method returns slot 0's pawn `EntityRef`:
```ts
import { publishInterface } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

export function onLoad(): void {
  console.log("[producer] onLoad — publishing @demo/ent@1.0.0");
  publishInterface("@demo/ent", "1.0.0", {
    // Return the pawn's EntityRef across the wire (null if no such pawn yet).
    pawnRef(slot: number) { const p = Pawn.forSlot(slot); return p ? p.ref : null; },
    // Producer-side health read — lets the consumer show a real number while alive without needing
    // a schema offset itself (typed cs2 accessors over a wired ref come in 5B).
    pawnHealth(slot: number) { const p = Pawn.forSlot(slot); return p ? p.health : null; },
  });
}
export function onUnload(): void { console.log("[producer] onUnload"); }
```

- [ ] **Step 2: Create the consumer `examples/entref-consumer`.**

`package.json`:
```json
{
  "name": "@demo/entref-consumer",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x", "pluginDependencies": { "@s2script/std": "^1.0.0", "@demo/ent": "^1.0.0" } }
}
```
`src/entref-iface.d.ts` (hand-written ambient type — interface `.d.ts` codegen is deferred):
```ts
declare module "@demo/ent" {
  import { EntityRef } from "@s2script/std";
  interface Ent {
    pawnRef(slot: number): EntityRef | null;
    pawnHealth(slot: number): number | null;
  }
  const _default: Ent;
  export = _default;
}
```
`src/plugin.ts` — read the producer-passed `EntityRef`'s health; it must go `null` on death:
```ts
import { OnGameFrame } from "@s2script/std";
import ent = require("@demo/ent"); // hard dep -> proxy

let ticks = 0;
export function onLoad(): void {
  console.log("[consumer] onLoad");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    try {
      const ref = ent.pawnRef(0);                        // a LIVE EntityRef received across the wire
      // ref.isValid() validates against the SHARED entity system: TRUE while the pawn lives, FALSE
      // once it is destroyed — the cross-plugin host-invalidation proof (offset-free, no schema on
      // the consumer side). `pawnHealth(0)` shows a real number while alive.
      const alive = ref ? ref.isValid() : false;
      console.log("[consumer] tick " + ticks + " received-ref valid=" + alive
        + " health=" + (alive ? ent.pawnHealth(0) : "null"));
    } catch (e) { console.log("[consumer] failed (degraded): " + String(e)); }
  });
}
export function onUnload(): void { console.log("[consumer] onUnload"); }
```

- [ ] **Step 3: Build both `.s2sp`s + the sniper runtime.**

```bash
cd /home/gkh/projects/s2script
node packages/cli/build.mjs
npx s2script build examples/entref-producer
npx s2script build examples/entref-consumer
bash scripts/build-sniper.sh   # fresh s2script.so (GLIBC <= 2.30); must post-date the Task-1 commit
```
If a CS2 update reset `gameinfo.gi` (addon loads 0 plugins), run `bash docker/patch-gameinfo.sh` and restart.

- [ ] **Step 4: Run the cross-plugin host-invalidation LIVE GATE on Docker CS2.** Drop both `.s2sp`s into `dist/addons/s2script/plugins/`; via `scripts/rcon.py` + container logs, get the map ticking (`bot_quota 1`, `sv_hibernate_when_empty 0`; wait past the boot window):
  1. Load both + a live bot → `[producer] onLoad`, `[consumer] onLoad`, and `[consumer] … received-ref valid=true health=100` — the **producer-passed** `EntityRef` arrived LIVE and validates against the shared entity system.
  2. Destroy the pawn — **`bot_kick` / lethal damage, NOT `mp_restartgame`** (the 5A spike found `mp_restartgame` doesn't destroy the pawn) → `[consumer] … received-ref valid=false health=null` — the received ref invalidated across the plugin boundary; server keeps ticking (`Up` in `docker ps`), no crash.
  Capture the log excerpts. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done (demos, both `.s2sp`s, sniper `.so`, README/CLAUDE drafted) and report BLOCKED with the exact commands/errors so the controller can drive the gate.

- [ ] **Step 5: README + CLAUDE.md.** Add a `## EntityRef across the wire (5A fast-follow)` section to `README.md` (build→drop→read→kill runbook + the captured `received-ref valid=false` log + an acceptance note). Update `CLAUDE.md` "## Current state": the 4.5/5A "entity refs on the inter-plugin wire" deferral is now CLOSED — an `EntityRef` round-trips as a live ref (host-invalidation across the plugin boundary). Do NOT alter the standing conventions above it.

- [ ] **Step 6: Final verification + commit** (do NOT commit build artifacts — `.s2sp`/`dist`/`.so` gitignored):

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/entref-producer examples/entref-consumer README.md CLAUDE.md
git commit -m "feat(entref-wire): cross-plugin host-invalidation live gate — producer-passed ref goes null on death

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §7)

1. `cargo test -p s2script-core` green (82 prior + 3 new in-isolate tests); both boundary gates green; sniper build OK.
2. `s2script build` produces the producer + consumer `.s2sp`s passing an `EntityRef`.
3. Live gate: the consumer reads a producer-passed pawn's health (or `isValid()`) through the received `EntityRef`, and it goes `null`/`false` when the pawn is destroyed — no crash.
4. README documents the runbook + acceptance; CLAUDE.md "Current state" notes the wire deferral closed.

# Sound (EmitSound + Precache) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the Sound slice per the approved spec (`docs/superpowers/specs/2026-07-13-sound-emitsound-precache-design.md`): (1) **Emit** — play a named built-in CS2 SoundEvent from a serial-gated source entity to a recipient slot set with volume (`Sound.emit`, `pawn.emitSound`), returning the engine sound GUID or 0; (2) **Precache** — let plugins register custom sound/resource paths into the session resource manifest at map load (`Sound.onPrecache(ctx => ctx.add(path))`).

**Architecture:** Two ops ABI-appended after `entity_set_model` (the current struct tail): `sound_emit` (shim: sig-resolved `CBaseEntity::EmitSound` — the PREFERRED ModSharp member prototype `(const char* name, const float* volume, const IRecipientFilter*)`, with the source-verified CSSharp `EmitSound_t` static prototype as the RE-decided fallback — over a minimal ported `S2RecipientFilter` with per-slot bot-skip) and `sound_precache_add` (shim: `IResourceManifest::AddResource`, vtable slot 0, on a manifest pointer stashed only for the duration of the hook dispatch). Precache delivery is a new notify-mux: a **manual SourceHook** on the existing `CGameRulesGameSystem::OnPrecacheResource` (instance resolved by walking the sig-resolved game-system factory list; vtable index from gamedata) → a new FFI export `s2script_core_dispatch_precache()` → `PRECACHE_MUX` (event_mux reuse, the `dispatch_map_start` pattern) → the `Sound.onPrecache` subscribers, whose block-scoped `PrecacheContext.add` hits the op. Engine-generic module `@s2script/sound` (`__s2pkg_sound`) in the core prelude; CS2 sugar (`pawn.emitSound`, curated `Sounds`) in the game layer.

**Tech Stack:** Rust core (`core/src/v8host.rs` + `core/src/ffi.rs` + `core/src/loader.rs`, rusty_v8), C++ shim (`shim/src/s2script_mm.cpp` + `shim/src/s2script_mm.h` + `shim/include/s2script_core.h`, hl2sdk cs2 + SourceHook incl. the first MANUAL hook), `gamedata/core.gamedata.jsonc`, CS2 JS (`games/cs2/js/pawn.js`), TypeScript types (`packages/sound`, `packages/cs2`), demo plugin (`examples/sound-demo`), changeset.

## Global Constraints

- **Core owns every engine touchpoint; dependencies game → core only.** A soundevent NAME, a recipient slot set, and a resource path are Source2-generic → the ops, natives, mux, and `@s2script/sound` live in core. `CBaseEntity::EmitSound`, `EmitSound_t`, `IRecipientFilter`/`CRecipientFilter`, `CGameRulesGameSystem`, `IResourceManifest`, `CBaseGameSystemFactory` are Source2 ENGINE types → shim-only. CS2 soundevent name strings (`Sounds`) live exclusively in `games/cs2/js/pawn.js` + `packages/cs2` — **ZERO CS2 identifiers in `core/src`**. Both boundary gates (`bash scripts/check-core-boundary.sh`, `bash scripts/test-boundary-nameleak.sh`) must stay green after every task that touches core/shim.
- **ABI-append discipline (mandatory).** Exactly TWO new ops, `sound_emit` then `sound_precache_add`, appended **after `entity_set_model`** — the current last op (verify before starting: typedef `shim/include/s2script_core.h:218`, struct member `:316` with the struct closing at `:317`; Rust mirror `core/src/v8host.rs:312`) — never inserted mid-struct — in lockstep across FOUR touchpoints: (1) the C header typedefs + struct fields; (2) the Rust `type SoundEmitFn`/`SoundPrecacheAddFn` + `pub sound_emit`/`pub sound_precache_add` fields; (3) **both** in-isolate test op-struct literals (exactly two — the ones containing `entity_set_model: None,`, near `core/src/v8host.rs:9791` and inside `fn mock_event_ops()` near `:10308`; `grep -c "entity_set_model: None" core/src/v8host.rs` must go 2→2 with both gaining the two new lines); (4) the shim `ops.sound_emit = …; ops.sound_precache_add = …;` after `ops.entity_set_model`. The precache DELIVERY is a FUNCTION EXPORT (`s2script_core_dispatch_precache`, like `s2script_core_dispatch_map_start`), not an op.
- **Rebase check FIRST (and again before the live gate).** PR #23 (`feat/entity-lifecycle-listeners`) touched the same ABI tail / gamedata / `Load()`. Before Task 1: `git fetch origin && gh pr view 23 --json state,mergedAt`; if `origin/main` moved, `git rebase origin/main` and RE-VERIFY the actual struct tail (`grep -n "} S2EngineOps" shim/include/s2script_core.h` and read the 10 lines above it) — if PR #23 appended ops, the two sound ops append after PR #23's NEW tail across all four touchpoints, and every ":line" anchor in this plan shifts (re-grep, don't trust line numbers).
- **Every RE fact is a HINT, never a trusted number** (`docs/re-strategy.md`). The ModSharp/CSSharp byte patterns and the `OnPrecacheResource` vtable index 7 in this plan are STARTING VALUES: each is re-resolved/validated UNIQUE against our pinned `libserver.so` (`/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so` — the exact binary the boot gate runs on) by the offline RE steps, `.text`-guarded at every call site (`IsAddressInServerText`), and gate-validated at boot (`GamedataResult` → the `GAMEDATA VALIDATION` line).
- **Degrade-never-crash.** `sound_emit` returns 0 WITHOUT calling the engine only when: unresolved emit sig / `.text` fail / `!soundName` / stale-or-null source entity / the CALLER requested no recipients (`slotCount <= 0`). An all-bot-skipped (post-filter empty) recipient set is NOT a degrade — the engine fn IS called (a PVS/PAS filter excluding everyone plays to nobody, no netchannel touched; this also exercises the resolved fn on a bots-only gate). Unresolved precache sigs/instance → the hook is never installed (onPrecache never fires; emit still works). `sound_precache_add` outside a live hook dispatch → 0. Every new native `catch_unwind`s with the safe default set first; every shim path null-guards (`s_pEmitSound`/`s_pEngine`/`s_currentPrecacheManifest`/factory-list pointers).
- **Tests run serial:** `.cargo/config.toml` sets `RUST_TEST_THREADS = "1"`. Run with `cd core && cargo test`.
- **The shim C++ is NOT compiled locally** — only at the docker sniper build (`rust:bullseye`; GLIBC floors: core `.so` ≤ 2.30, shim `.so` ≤ 2.14). Write it carefully; the adversarial reviews are the compile-gate proxy. The `hl2sdk` submodule is NOT checked out in this fresh worktree — `git submodule update --init` before the sniper build (and before reading SDK headers; until then read them from `/home/gkh/projects/s2script/third_party/hl2sdk`).
- **Do NOT run the sniper build or touch Docker inside the Workflow tasks** — the build + live gate is the final human-in-the-loop section.
- Git commits use `git commit -F - <<'EOF' … EOF` (never backticks) and end with the Claude-Session trailer shown in each step.

---

## File Structure

- `shim/include/s2script_core.h` — Task 1: `s2_sound_emit_fn` + `s2_sound_precache_add_fn` typedefs + struct fields (after `entity_set_model`). Task 3: the `s2script_core_dispatch_precache` export decl (next to `s2script_core_dispatch_map_start` at `:348`).
- `core/src/v8host.rs` — Task 1: op mirrors + both test structs + `s2_sound_emit`/`s2_sound_precache_add` natives + registration + degrade/marshalling tests. Task 3: `PRECACHE_MUX` + `dispatch_precache` + `s2_precache_subscribe` native + teardown wiring + dispatch test. Task 4: the `@s2script/sound` prelude module + module tests.
- `core/src/ffi.rs` — Task 3: `s2script_core_dispatch_precache` export.
- `core/src/loader.rs` — Task 4: `"@s2script/sound"` into `BUILTIN_MODULES` (`:63`).
- `gamedata/core.gamedata.jsonc` — Task 2: the `EmitSound` signature. Task 5: the `GameSystemFactoryList` signature + the `CGameRulesGameSystem_OnPrecacheResource` offsets (vtable-index) entry.
- `shim/src/s2script_mm.cpp` — Task 2: `#include "irecipientfilter.h"`, `S2RecipientFilter`, `s_pEmitSound` + `s2_sound_emit`, `Load()` sig resolution, both `ops.` assignments. Task 5: the factory-list statics + `TryInstallPrecacheHook` + the manual hook decl/body + `s2_sound_precache_add` + Load/StartupServer/Unload wiring.
- `shim/src/s2script_mm.h` — Task 5: `Hook_OnPrecacheResource` + `TryInstallPrecacheHook` decls + `m_precacheHookInstalled`.
- `packages/sound/{package.json,index.d.ts}` — Task 4 (new types-only package).
- `games/cs2/js/pawn.js` — Task 6: `Pawn.prototype.emitSound` + `Sounds` + the tail-export merge.
- `packages/cs2/index.d.ts` — Task 6: `emitSound` on `Pawn` + `export declare const Sounds`.
- `examples/sound-demo/{package.json,tsconfig.json,src/plugin.ts}` — Task 7.
- `.changeset/sound-slice.md` — Task 7.

---

## Task 1: ABI plumbing — the two ops + the two natives + degrade/marshalling tests

**Files:**
- Modify: `shim/include/s2script_core.h` (typedefs after `s2_entity_set_model_fn` `:218`; struct fields after `entity_set_model` `:316`)
- Modify: `core/src/v8host.rs` (op mirror types after `EntitySetModelFn` `:210`; struct fields after `entity_set_model` `:312`; BOTH test op-struct literals `:9791` + `:10308`; the two natives; `set_native` registration; tests)

**Interfaces:**
- Produces: op `int sound_emit(const char* soundName, int entIndex, int entSerial, const int* slots, int slotCount, float volume)` (returns the `SndOpEventGuid` uint32 as int, 0 = fail; `entSerial < 0` = no-serial-check sentinel); op `int sound_precache_add(const char* path)` (1 = added, 0 = no live manifest/unresolved); native `__s2_sound_emit(soundName: string, entIndex: number, entSerial: number, slotsArray: number[], volume: number) -> number`; native `__s2_sound_precache_add(path: string) -> boolean`. These exact signatures are consumed by Tasks 2/4/5 — do not drift.

- [ ] **Step 0: Rebase check.** `git fetch origin && gh pr view 23 --json state,mergedAt`. If `origin/main` moved: `git rebase origin/main`, then re-verify the ABI tail (`grep -n "entity_set_model" shim/include/s2script_core.h core/src/v8host.rs`) — if a merged slice appended past `entity_set_model`, substitute the NEW tail everywhere this plan says "after `entity_set_model`".

- [ ] **Step 1: C header.** In `shim/include/s2script_core.h`, after the `s2_entity_set_model_fn` typedef (`:218`):

```c
/* Sound slice — APPENDED after entity_set_model; order is the ABI.
 * sound_emit: play a named CS2 SoundEvent from a serial-gated source entity to a slot set.
 * Sig-resolved CBaseEntity::EmitSound (preferred member overload (name, volume*, IRecipientFilter*);
 * the CSSharp EmitSound_t static path as fallback — see the 2026-07-13 sound spec). soundName = the
 * soundevent name (the engine resolves name->hash). entSerial < 0 = emit from entIndex with NO serial
 * check (worldspawn / global 2D). slots[0..slotCount) = recipient slots (bot slots are skipped — no
 * netchannel). volume in [0,1]. Returns the SndOpEventGuid (nonzero uint32 as int) or 0 (unresolved
 * sig / stale entity / caller requested no recipients (slotCount <= 0)). An all-bot-skipped filter
 * still CALLS the engine (plays to nobody), not a degrade. ENGINE-GENERIC. */
typedef int (*s2_sound_emit_fn)(const char* soundName, int entIndex, int entSerial,
                                const int* slots, int slotCount, float volume);
/* sound_precache_add: add a resource path (e.g. "soundevents/mypack.vsndevts") to the session
 * resource manifest currently being built. Valid ONLY during a precache-hook dispatch (the manifest
 * pointer is live only then; block-scoped like a game event). Returns 1 on add, 0 if no active
 * manifest / unresolved. ENGINE-GENERIC. */
typedef int (*s2_sound_precache_add_fn)(const char* path);
```

and in `struct S2EngineOps` after `s2_entity_set_model_fn entity_set_model;` (`:316`, just before the closing `} S2EngineOps;`):

```c
    /* Sound slice — APPENDED after entity_set_model (the struct tail); order is the ABI. */
    s2_sound_emit_fn         sound_emit;
    s2_sound_precache_add_fn sound_precache_add;
```

- [ ] **Step 2: Rust op mirror.** In `core/src/v8host.rs`, after `type EntitySetModelFn = …` (`:210`):

```rust
// --- Sound slice (APPENDED after entity_set_model; order is the ABI). ENGINE-GENERIC: a soundevent
// NAME + a recipient slot set + a resource path are Source2-generic; no CS2 names in the C ABI.
type SoundEmitFn = extern "C" fn(*const c_char, c_int, c_int, *const c_int, c_int, f32) -> c_int;
type SoundPrecacheAddFn = extern "C" fn(*const c_char) -> c_int;
```

and in `pub struct S2EngineOps` after `pub entity_set_model: Option<EntitySetModelFn>,` (`:312`):

```rust
    // --- Sound slice (APPENDED after entity_set_model; order is the ABI; do not reorder above) ---
    pub sound_emit: Option<SoundEmitFn>,
    pub sound_precache_add: Option<SoundPrecacheAddFn>,
```

- [ ] **Step 3: BOTH test op-structs.** Add to both literals, directly after their `entity_set_model: None,` line (the full literal near `:9791`, and the one inside `fn mock_event_ops()` near `:10308`):

```rust
            sound_emit: None,
            sound_precache_add: None,
```

Verify: `grep -c "sound_precache_add: None" core/src/v8host.rs` → `2`.

- [ ] **Step 4: Write the failing degrade test** (same harness as `register_cvar_degrades_false_without_op` near `:11040` — `init`/`set_engine_ops(None)`/`create_plugin_context`/`eval_in_context_string`/`shutdown`), in the same `#[cfg(test)]` module as the existing degrade tests:

```rust
    /// Both sound natives degrade with no ops table: emit -> 0, precache-add -> false. Raw-native
    /// level (the @s2script/sound module surface is Task-4-tested).
    #[test]
    fn sound_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("psnd");
        assert_eq!(eval_in_context_string("psnd",
            "String(__s2_sound_emit('Weapon_AK47.Single', 0, -1, [0, 1], 1.0))"), "0");
        assert_eq!(eval_in_context_string("psnd",
            "String(__s2_sound_precache_add('soundevents/test.vsndevts'))"), "false");
        shutdown();
    }
```

- [ ] **Step 5: Run it — expect FAIL** — `cd core && cargo test sound_natives_degrade` → FAIL (`__s2_sound_emit is not defined`).

- [ ] **Step 6: The two natives.** In `core/src/v8host.rs`, directly below `s2_ent_set_model` (`:5060`):

```rust
/// Native `__s2_sound_emit(soundName, entIndex, entSerial, slotsArray, volume) -> number`. Over the
/// `sound_emit` op. Reads the JS slot array into a `Vec<i32>` (mirrors `__s2_user_message_send`); a
/// non-array slots arg -> an empty set (the op returns 0 — caller requested no recipients). An
/// all-bot-skipped non-empty request still calls the engine shim-side (plays to nobody). Returns the
/// SndOpEventGuid as a uint32 number, 0 = failed. Degrades to 0 with no op; never throws.
fn s2_sound_emit(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_uint32(0);
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.sound_emit) else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(c_name) = std::ffi::CString::new(name) else { return };
        let ent_index = args.get(1).integer_value(scope).unwrap_or(0) as i32;
        let ent_serial = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let mut slots: Vec<i32> = Vec::new();
        if let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(3)) {
            let n = arr.length();
            slots.reserve(n as usize);
            for i in 0..n {
                let s = match arr.get_index(scope, i) {
                    Some(v) => v.integer_value(scope).unwrap_or(-1) as i32,
                    None => -1,
                };
                slots.push(s);
            }
        }
        let volume = args.get(4).number_value(scope).unwrap_or(1.0) as f32;
        let guid = f(c_name.as_ptr(), ent_index, ent_serial, slots.as_ptr(), slots.len() as i32, volume);
        rv.set_uint32(guid as u32);
    }));
}

/// Native `__s2_sound_precache_add(path) -> boolean`. Over the `sound_precache_add` op — valid only
/// during a precache-hook dispatch (block-scoped; the shim's manifest stash is null otherwise).
/// Degrades to `false` with no op / no active manifest / a NUL in the path. Never throws.
fn s2_sound_precache_add(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(f) = ops.and_then(|o| o.sound_precache_add) else { return };
        let path = args.get(0).to_rust_string_lossy(scope);
        let Ok(c_path) = std::ffi::CString::new(path) else { return };
        rv.set_bool(f(c_path.as_ptr()) == 1);
    }));
}
```

Register both next to the `__s2_ent_set_model` registration (grep `set_native(scope, global_obj, "__s2_ent_set_model"`):

```rust
    set_native(scope, global_obj, "__s2_sound_emit", s2_sound_emit);
    set_native(scope, global_obj, "__s2_sound_precache_add", s2_sound_precache_add);
```

- [ ] **Step 7: Run the degrade test — expect PASS** — `cd core && cargo test sound_natives_degrade` → PASS.

- [ ] **Step 8: Write the marshalling test (mock op).** In the test module, next to `mock_event_ops()` (`:10249`), add the capture + mock (the `RUST_TEST_THREADS=1` serial convention — same as the `LOG`/`EV_SUBSCRIBED` buffers):

```rust
    static SOUND_EMIT_CALLS: std::sync::Mutex<Vec<(String, i32, i32, Vec<i32>, f32)>> =
        std::sync::Mutex::new(Vec::new());
    extern "C" fn mock_sound_emit(name: *const c_char, ent_index: c_int, ent_serial: c_int,
                                  slots: *const c_int, slot_count: c_int, volume: f32) -> c_int {
        let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
        let s = if slots.is_null() || slot_count <= 0 { Vec::new() }
                else { unsafe { std::slice::from_raw_parts(slots, slot_count as usize) }.to_vec() };
        SOUND_EMIT_CALLS.lock().unwrap().push((n, ent_index, ent_serial, s, volume));
        7   // a fake nonzero guid
    }
```

and the test:

```rust
    /// __s2_sound_emit marshals (name, entIndex, entSerial, slots[], volume) into the op and
    /// returns its guid (struct-update over mock_event_ops, the entity_spawn_kv capture precedent).
    #[test]
    fn sound_emit_marshals_args_to_op() {
        let _ = init(dummy_logger());
        SOUND_EMIT_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(S2EngineOps { sound_emit: Some(mock_sound_emit), ..mock_event_ops() }));
        create_plugin_context("psm");
        let out = eval_in_context_string("psm",
            "String(__s2_sound_emit('Weapon_AK47.Single', 42, 99, [3, 5], 0.5))");
        assert_eq!(out, "7");
        let calls = SOUND_EMIT_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Weapon_AK47.Single");
        assert_eq!(calls[0].1, 42);
        assert_eq!(calls[0].2, 99);
        assert_eq!(calls[0].3, vec![3, 5]);
        assert!((calls[0].4 - 0.5).abs() < 1e-6);
        shutdown();
    }
```

- [ ] **Step 9: Run — expect PASS** — `cd core && cargo test sound_emit_marshals` → PASS. Then the full suite + gates: `cd core && cargo test` → all green; `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` → green.

- [ ] **Step 10: Commit**
```bash
git add shim/include/s2script_core.h core/src/v8host.rs
git commit -F - <<'EOF'
feat(sound): sound_emit + sound_precache_add ABI plumbing + natives

Two ops ABI-appended after entity_set_model in lockstep across the C header,
the Rust mirror, and BOTH in-isolate test op-structs. __s2_sound_emit reads the
JS slot array into a Vec<i32> (the user_message_send pattern) and returns the
SndOpEventGuid as uint32; __s2_sound_precache_add is block-scoped over the
shim's manifest stash. Both catch_unwind + degrade (0/false) with no op; a
mock-op test proves the arg marshalling.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 2: Emit shim — offline RE, the `EmitSound` gamedata sig, `S2RecipientFilter`, and the guarded call

**Files:**
- Modify: `gamedata/core.gamedata.jsonc` (`EmitSound` signature into `signatures`, after the last existing entry)
- Modify: `shim/src/s2script_mm.cpp` (SDK include; `S2RecipientFilter` + `SndOpEventGuid_t` + `s_pEmitSound` + `s2_sound_emit` below `Shim_EntitySetModel`; the `Load()` resolution block after the `SetModel` block `:2400-2411`; both `ops.` assignments after `ops.entity_set_model` `:2722`)

**Interfaces:**
- Consumes: op signature `s2_sound_emit_fn` from Task 1 (must match byte-for-byte); `ResolveEntityBySerial(int index, int serial) -> CEntityInstance*` (`:~1656`); `s2_ent_by_index(int idx) -> void*` (`:209`); `IsAddressInServerText(void* fn) -> bool` (`:1670`); `s_pEngine->GetPlayerNetInfo(CPlayerSlot)` (the client_print bot-skip precedent, `:641`); `LoadSignatures`/`ResolveSigValidated`/`FindModuleText`/`GamedataResult` (the `CommitSuicide` block `:2340-2352` is the exact model); SDK `public/irecipientfilter.h` (`IRecipientFilter` 4-method interface: `GetNetworkBufType`/`IsInitMessage`/`GetRecipients`/`GetPredictedPlayerSlot`), `CPlayerBitVec` (`public/const.h:40`), `BUF_RELIABLE` (`public/inetchannel.h:54`), `CEntityIndex` (`public/entity2/entityidentity.h:31`).
- Produces: `static int s2_sound_emit(const char* soundName, int entIndex, int entSerial, const int* slots, int slotCount, float volume)`; `ops.sound_emit`/`ops.sound_precache_add` slots filled (`sound_precache_add` points at Task 5's fn — see Step 6 ordering note).

- [ ] **Step 1: Offline RE — resolve + disambiguate the EmitSound prototype.** This step is LOAD-BEARING (the two reference frameworks disagree on the prototype; calling with the wrong one corrupts the stack). Work against the pinned binary `/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so`. Write this scanner to the scratchpad (it mirrors `shim/src/sigscan.cpp` semantics — PF_X segments only, wildcard `?` tokens):

```python
#!/usr/bin/env python3
# sigscan-offline.py — scan an ELF .so's executable segments for a wildcard byte pattern.
# Usage: python3 sigscan-offline.py <libserver.so> "55 48 89 E5 41 57 ? ? 0F 7E"
import sys, struct
path, pat = sys.argv[1], sys.argv[2]
toks = [None if t in ("?", "??") else int(t, 16) for t in pat.split()]
data = open(path, "rb").read()
assert data[:4] == b"\x7fELF"
e_phoff = struct.unpack_from("<Q", data, 0x20)[0]
e_phentsize = struct.unpack_from("<H", data, 0x36)[0]
e_phnum = struct.unpack_from("<H", data, 0x38)[0]
hits = []
for i in range(e_phnum):
    o = e_phoff + i * e_phentsize
    p_type, p_flags = struct.unpack_from("<II", data, o)
    p_offset, p_vaddr = struct.unpack_from("<QQ", data, o + 8)
    p_filesz = struct.unpack_from("<Q", data, o + 32)[0]
    if p_type != 1 or not (p_flags & 1):    # PT_LOAD + PF_X only
        continue
    seg = data[p_offset:p_offset + p_filesz]
    n = len(toks)
    for j in range(len(seg) - n + 1):
        if all(t is None or seg[j + k] == t for k, t in enumerate(toks)):
            hits.append((p_offset + j, p_vaddr + j))
print(f"{len(hits)} match(es)")
for fo, va in hits:
    print(f"  file=0x{fo:x} vaddr=0x{va:x}")
```

Then:
1. Scan BOTH hint patterns from the spec — ModSharp `CBaseEntity::EmitSoundFilter`: `55 48 89 E5 41 57 66 41 0F 7E C7 41 56 4D 89 C6`; CSSharp: `55 48 89 E5 53 48 89 FB 48 83 EC ? E8 ? ? ? ? 48 89 D8 48 8B 5D ? C9 C3 CC CC CC CC CC CC 48 B8`. Record match count + vaddr for each. Expect 1 match each (they may be the same or different functions).
2. Disassemble each match: `objdump -d --start-address=0x<vaddr> --stop-address=0x<vaddr+0x200> <libserver.so>`.
3. **Decide the prototype.** The return type is CSSharp's 24-byte `SndOpEventGuid_t` → SysV returns it via a HIDDEN sret pointer in `rdi`, shifting every arg one register right. For the PREFERRED member overload `SndOpEventGuid_t CBaseEntity::EmitSound(const char* name, const float* volume, const IRecipientFilter* filter)` expect: `rdi`=sret out, `rsi`=this, `rdx`=name, `rcx`=volume*, `r8`=filter. Confirm from the disasm: (a) the `const float*` slot is null-checked then dereferenced as a float (`movss (%rcx),%xmm…` or after a reg move) and flows into a volume-looking store — this confirms it is VOLUME, not classic-Source duration; (b) the name arg is passed onward as a string pointer; (c) the epilogue writes 16-24 bytes through the saved first register (the sret out). For the FALLBACK static `SndOpEventGuid_t EmitSound(CRecipientFilter& filter, CEntityIndex ent, const EmitSound_t& params)` expect: `rdi`=sret, `rsi`=filter (vtable calls through it), `edx`=ent index, `rcx`=params. If ambiguity remains, cross-check via the spec's string refs (`EmitSoundByHandle`, `public.distance_volume_mapping_curve`, `Playing sound on non-networked entity %s`: `strings -t x <so> | grep <str>` → the xref'd function neighborhood).
4. **Record the decision** (member vs fallback + the confirmed register layout + which hint pattern is unique) in the gamedata comment (Step 2) and the shim block comment (Step 4). If the member overload resolves AND its `const float*` is volume → implement Variant A (Step 4). Else → implement Variant B (Step 5) with the CSSharp pattern as the sig.

- [ ] **Step 2: gamedata.** In `gamedata/core.gamedata.jsonc` `signatures`, append after the last existing signature entry (keep the file's `// comment` style):

```jsonc
    // CBaseEntity::EmitSound (Sound slice): the engine's soundevent-emit entry — ModSharp's
    // CBaseEntity::EmitSoundFilter == CSSharp's CBaseEntity_EmitSoundFilter key. The two frameworks
    // disagree on the prototype (member (name, volume*, IRecipientFilter*) vs static
    // (CRecipientFilter&, CEntityIndex, EmitSound_t&)); the Task-2 offline RE step disassembled our
    // pinned libserver.so and confirmed <RECORD THE FINDING HERE: variant + register layout>.
    // A DIRECT prologue pattern (match offset == function start), validated UNIQUE offline + by the
    // boot gate. If it drifts on an update, re-locate via the string refs: "EmitSoundByHandle",
    // "public.distance_volume_mapping_curve", "Playing sound on non-networked entity %s".
    "EmitSound": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 41 57 66 41 0F 7E C7 41 56 4D 89 C6",
        "resolve": "direct"
      }
    }
```

(The pattern shown is the ModSharp hint — the Step-1 scan either confirms it UNIQUE on our binary as-is, or you extend/replace it from the disasm until the offline scanner prints exactly `1 match(es)`. The committed pattern MUST be the offline-validated one; update the comment placeholder with the actual finding.)

- [ ] **Step 3: SDK include + recipient filter.** In `shim/src/s2script_mm.cpp`, add to the SDK include block (grep `#include "eiface.h"` or the nearest SDK include cluster):

```cpp
#include "irecipientfilter.h"   // Sound slice: the modern 4-method IRecipientFilter + CPlayerBitVec
```

Then, directly below `Shim_EntitySetModel`'s definition (grep `static int Shim_EntitySetModel`):

```cpp
// ---------------------------------------------------------------------------
// Sound slice — emit (see docs/superpowers/specs/2026-07-13-sound-emitsound-precache-design.md).
// A minimal modern recipient filter over the SDK's 4-method IRecipientFilter
// (public/irecipientfilter.h), ported from CSSharp's recipientfilters.h: a slot-indexed
// CPlayerBitVec, bounded 0..63. Reliable buffer, never an init message, no predicted slot.
// ---------------------------------------------------------------------------
class S2RecipientFilter : public IRecipientFilter {
public:
    S2RecipientFilter() { m_Recipients.ClearAll(); }
    ~S2RecipientFilter() override {}
    NetChannelBufType_t GetNetworkBufType() const override { return BUF_RELIABLE; }
    bool IsInitMessage() const override { return false; }
    const CPlayerBitVec& GetRecipients() const override { return m_Recipients; }
    CPlayerSlot GetPredictedPlayerSlot() const override { return CPlayerSlot(-1); }
    void AddRecipient(int slot) { if (slot >= 0 && slot < 64) m_Recipients.Set(slot); }
    int Count() const {
        int n = 0;
        for (int s = 0; s < 64; s++) if (m_Recipients.IsBitSet(s)) n++;
        return n;
    }
private:
    CPlayerBitVec m_Recipients;
};
```

- [ ] **Step 4: The emit call — Variant A (PREFERRED member prototype; commit this one iff Step 1 confirmed it).** Directly below `S2RecipientFilter`:

```cpp
// CBaseEntity::EmitSound — the ModSharp member overload, RE-CONFIRMED on our pinned libserver.so
// (Task-2 offline step): SndOpEventGuid_t is 24 bytes (CSSharp entity_manager.h:250 — uint32 guid +
// uint64 stack hash + uint64 pad) -> SysV returns it via a hidden sret pointer; declaring the struct
// return lets the compiler emit that convention (rdi=sret, rsi=this, rdx=name, rcx=volume*,
// r8=filter). The const float* arg is VOLUME (disasm-confirmed — classic Source used that slot for
// duration, which is why the RE step had to check). A null filter would broadcast to everyone
// (ModSharp semantics); we ALWAYS pass our filter so the bot-skip recipient set is authoritative.
struct SndOpEventGuid_t {
    uint32 m_nGuid;
    uint64 m_hStackHash;
    uint64 pad;   // CSSharp: "size might be incorrect" — harmless for an out-value we only read m_nGuid from
};
typedef SndOpEventGuid_t (*EmitSoundFn_t)(void* thisptr, const char* soundName,
                                          const float* volume, const IRecipientFilter* filter);
static EmitSoundFn_t s_pEmitSound = nullptr;   // sig-resolved in Load(); null -> op no-ops

// The sound_emit op. Degrade-never-crash — return 0 WITHOUT calling the engine ONLY when: unresolved
// sig / out-of-.text fn / !soundName / stale-or-null source entity / the CALLER requested no
// recipients (slotCount <= 0 || !slots). An all-bot-skipped filter (Count()==0 after the loop) is
// NOT a degrade — build it and CALL the engine anyway: a PVS/PAS filter excluding everyone is a
// normal, safe engine path (plays to nobody, no netchannel touched), more correct than a "failed" 0
// for a bot-only target, and it exercises the resolved fn + its 24-byte sret ABI + prototype on a
// bots-only live gate. entSerial >= 0 -> serial-gated via ResolveEntityBySerial (the
// pawn_commit_suicide pattern); entSerial < 0 -> the sentinel: entIndex used directly (worldspawn /
// global 2D emit from index 0). Recipient bot-skip: a fake client has no netchannel — it can't hear
// the sound AND a null-netchannel send is the client_print / user_message_send crash surface, so each
// requested slot is admitted only if GetPlayerNetInfo(slot) != null. Volume clamped into [0,1]
// (NaN/out-of-range -> 1.0).
static int s2_sound_emit(const char* soundName, int entIndex, int entSerial,
                         const int* slots, int slotCount, float volume) {
    if (!s_pEmitSound || !soundName || !soundName[0]) return 0;
    if (!IsAddressInServerText(reinterpret_cast<void*>(s_pEmitSound))) return 0;
    if (!slots || slotCount <= 0) return 0;                    // CALLER requested no recipients -> no-op
    void* ent = nullptr;
    if (entSerial >= 0) {
        ent = ResolveEntityBySerial(entIndex, entSerial);
    } else {
        ent = s2_ent_by_index(entIndex);
    }
    if (!ent) return 0;                                        // stale/free slot -> no-op
    S2RecipientFilter filter;
    for (int i = 0; i < slotCount; i++) {
        int slot = slots[i];
        if (slot < 0 || slot >= 64) continue;
        if (!s_pEngine || !s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot))) continue;   // bot-skip
        filter.AddRecipient(slot);
    }
    // An all-bot-skipped filter (Count()==0) is NOT a degrade — call the engine anyway (plays to
    // nobody, no netchannel touched). This also exercises the resolved fn on a bots-only live gate.
    float vol = volume;
    if (!(vol >= 0.0f) || vol > 1.0f) vol = 1.0f;               // !(>=0) also catches NaN
    SndOpEventGuid_t guid = s_pEmitSound(ent, soundName, &vol, &filter);
    META_CONPRINTF("[s2script] EmitSound '%s' recipients=%d -> guid=%u\n",
                   soundName, filter.Count(), guid.m_nGuid);
    return static_cast<int>(guid.m_nGuid);
}
```

- [ ] **Step 5: The emit call — Variant B (FALLBACK CSSharp static prototype; commit this INSTEAD of Variant A's typedef+call iff Step 1 disproved the member shape).** Same `S2RecipientFilter`, same `s2_sound_emit` guards/filter-build; only the struct, typedef, and the final call differ:

```cpp
// CBaseEntity::EmitSound — the CSSharp static prototype (entity_manager.h:257), used because the
// Task-2 offline RE step disproved the member overload on our binary. EmitSound_t is a byte-exact
// port of CSSharp's (entity_manager.h:221, live-proven by CSSharp on this engine); the ctor defaults
// are CSSharp's verbatim (m_nSourceSoundscape 0, m_nPitch PITCH_NORM=100). SndOpEventGuid_t is
// 24 bytes -> SysV sret: rdi=sret, rsi=filter, edx=ent index, rcx=params.
typedef uint32 SoundEventGuid_t;
struct EmitSound_t {
    const char*      m_pSoundName        = nullptr;
    Vector           m_vecOrigin         = Vector(0.0f, 0.0f, 0.0f);   // 3D positional deferred — zeroed
    float            m_flVolume          = 1.0f;
    float            m_flSoundTime       = 0.0f;
    CEntityIndex     m_nSpeakerEntity    = CEntityIndex(-1);
    SoundEventGuid_t m_nForceGuid        = 0;
    CEntityIndex     m_nSourceSoundscape = CEntityIndex(0);
    uint16           m_nPitch            = 100;   // PITCH_NORM; dead in the engine (CSSharp comment)
    uint8            m_nFlags            = 0;     // 0 = attach to the entity index
};
struct SndOpEventGuid_t {
    uint32 m_nGuid;
    uint64 m_hStackHash;
    uint64 pad;
};
typedef SndOpEventGuid_t (*EmitSoundFn_t)(S2RecipientFilter& filter, CEntityIndex ent,
                                          const EmitSound_t& params);
static EmitSoundFn_t s_pEmitSound = nullptr;
// ... s2_sound_emit identical to Variant A (same top guards INCLUDING the `if (!slots ||
// slotCount <= 0) return 0;` no-recipients check, the serial-gate, the bot-skip filter loop with
// NO Count()==0 early-out) down to the call site, then:
//     EmitSound_t params;
//     params.m_pSoundName = soundName;
//     params.m_flVolume   = vol;
//     SndOpEventGuid_t guid = s_pEmitSound(filter, CEntityIndex(entIndex), params);
//     META_CONPRINTF("[s2script] EmitSound '%s' recipients=%d -> guid=%u\n",
//                    soundName, filter.Count(), guid.m_nGuid);
//     return static_cast<int>(guid.m_nGuid);
// NOTE (Variant B only): the engine call takes the ENTITY INDEX, not the pointer — keep the
// serial-gate anyway (resolve `ent` and return 0 if stale) so a dead EntityRef still degrades to 0,
// exactly like Variant A; the resolved pointer is simply unused past the gate. An all-bot-skipped
// filter still CALLS the engine (plays to nobody), same as Variant A.
```

- [ ] **Step 6: `Load()` resolution + ops fill.** In the `Load()` signature block, directly after the `SetModel` resolution (`:2400-2411`), a verbatim mirror of the `CommitSuicide` shape:

```cpp
            // Sound slice: resolve CBaseEntity::EmitSound (soundevent emit; Sound.emit /
            // pawn.emitSound). A DIRECT prologue signature self-validated on OUR libserver.so
            // (the two reference frameworks' prototypes disagree — the committed pattern + call
            // shape are the Task-2 offline-RE finding). Unresolved -> s_pEmitSound stays null ->
            // sound_emit no-ops (degrade-never-crash).
            auto esit = sigs.find("EmitSound");
            if (esit == sigs.end()) {
                GamedataResult("EmitSound", false, "signature absent from gamedata");
            } else {
                int64_t esOff = ResolveSigValidated("EmitSound", esit->second);
                ModText esmt = FindModuleText(esit->second.module.c_str());
                if (esOff != s2sig::kFail && esmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pEmitSound = reinterpret_cast<EmitSoundFn_t>(const_cast<uint8_t*>(esmt.text) + esOff);
                    META_CONPRINTF("[s2script] EmitSound resolved @%p (Sound.emit)\n",
                                   reinterpret_cast<void*>(s_pEmitSound));
                }   // esOff == kFail: ResolveSigValidated already recorded the reason
            }
```

Then fill BOTH op slots after `ops.entity_set_model = &Shim_EntitySetModel;` (`:2722`) — `s2_sound_precache_add` is Task 5's function; to keep this task self-contained and compilable, add a forward declaration above the ops block and the assignment now:

```cpp
    // Sound slice — APPENDED after entity_set_model; order MUST match S2EngineOps.
    ops.sound_emit         = &s2_sound_emit;
    ops.sound_precache_add = &s2_sound_precache_add;
```

with, placed just above `s2_sound_emit`'s definition:

```cpp
static int s2_sound_precache_add(const char* path);   // defined with the precache hook (Task 5)
```

(Task 5 provides the definition; until then the shim would not link — acceptable, the shim only compiles at the sniper build which runs after all tasks. Flag this in the task's commit message so the reviewer knows it is intentional.)

- [ ] **Step 7: Gates.** `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` → green (no CS2 name enters core; `EmitSound`/`IRecipientFilter` are Source2-generic and shim-side). `cd core && cargo test` → all green (no core change in this task — regression sanity).

- [ ] **Step 8: Commit**
```bash
git add gamedata/core.gamedata.jsonc shim/src/s2script_mm.cpp
git commit -F - <<'EOF'
feat(sound): shim EmitSound — sig-resolved, serial-gated, bot-skip filtered

CBaseEntity::EmitSound resolved from gamedata ("EmitSound", direct prologue,
offline-validated UNIQUE on the pinned libserver.so; the prototype variant is
the offline-RE finding recorded in the gamedata comment). S2RecipientFilter
ports the modern 4-method IRecipientFilter over a CPlayerBitVec; recipients
are bot-skipped via GetPlayerNetInfo (null netchannel = the client_print crash
surface). Serial-gated source entity (entSerial<0 = worldspawn sentinel);
.text-guarded call; degrade to 0 WITHOUT calling only on unresolved/stale/
caller-requested-no-recipients — an all-bot-skipped filter still calls the
engine (plays to nobody), which also exercises the resolved fn on the bots
gate. sound_precache_add is forward-declared (defined in the precache task;
the shim links only at the sniper build).

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 3: Precache core — FFI export, `PRECACHE_MUX`, `dispatch_precache`, subscribe native

**Files:**
- Modify: `core/src/v8host.rs` (`PRECACHE_MUX` beside `MAP_MUX` `:511`; `dispatch_precache` below `dispatch_map_start` `:3715`; `s2_precache_subscribe` below `s2_map_start_subscribe` `:4431`; registration beside `__s2_map_start_subscribe` `:6048`; shutdown reset `:7865`; unload `remove_by_owner` `:8199`; test)
- Modify: `core/src/ffi.rs` (`s2script_core_dispatch_precache` below `s2script_core_dispatch_map_start` `:115`)
- Modify: `shim/include/s2script_core.h` (the export decl below `s2script_core_dispatch_map_start` `:348`)

**Interfaces:**
- Consumes: `crate::event_mux::EventMux`, the `dispatch_map_start` body (`:3679-3715`) as the verbatim mirror source, `current_plugin`/`PLUGINS`/`REGISTRY`/`HOST`.
- Produces: FFI export `void s2script_core_dispatch_precache(void)`; `pub(crate) fn dispatch_precache()`; native `__s2_precache_subscribe(handler: () => void)` — the handler is called with NO args (Task 4's prelude wrapper builds the `PrecacheContext`).

- [ ] **Step 1: `PRECACHE_MUX`.** In the `thread_local!` block beside `MAP_MUX` (`:511`):

```rust
    /// Precache subscribers (Sound slice). Fixed key "" (a precache-manifest build has no name
    /// dimension, like MAP_MUX); notify-only. The stored handler is the PRELUDE's wrapper closure —
    /// it constructs the block-scoped PrecacheContext and calls the plugin's handler.
    static PRECACHE_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
```

- [ ] **Step 2: Write the failing test** (beside `map_start_dispatch_delivers_map_name` `:9476`):

```rust
    /// dispatch_precache runs a Sound.onPrecache-level subscriber (raw __s2_precache_subscribe —
    /// the module wrapper is Task-4-tested); the block-scoped add degrades false with no op.
    #[test]
    fn precache_dispatch_runs_subscriber() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("ppc");
        eval_in_context_string("ppc", r#"
            globalThis.__fired = 0; globalThis.__addResult = null;
            __s2_precache_subscribe(function () {
                globalThis.__fired++;
                globalThis.__addResult = __s2_sound_precache_add("soundevents/x.vsndevts");
            });
            "ok"
        "#);
        dispatch_precache();
        assert_eq!(eval_in_context_string("ppc", "String(globalThis.__fired)"), "1");
        assert_eq!(eval_in_context_string("ppc", "String(globalThis.__addResult)"), "false");
        shutdown();
    }
```

- [ ] **Step 3: Run it — expect FAIL** — `cd core && cargo test precache_dispatch` → FAIL (`__s2_precache_subscribe is not defined` / `dispatch_precache` not found).

- [ ] **Step 4: `dispatch_precache`.** Directly below `dispatch_map_start` (`:3715`) — a verbatim mirror with a zero-arg call:

```rust
/// Deliver a precache-manifest-build notification to the `Sound.onPrecache` subscribers. Called
/// from ffi.rs's `s2script_core_dispatch_precache` (the shim's CGameRulesGameSystem::
/// OnPrecacheResource MANUAL hook, which stashes the live IResourceManifest* around this call so
/// the `sound_precache_add` op can AddResource into it — block-scoped: the stash is cleared when
/// the hook returns, so a handler must use its PrecacheContext synchronously). Mirrors
/// `dispatch_map_start` verbatim: snapshot (release the mux borrow), `try_borrow_mut` re-entrancy
/// guard, per-subscriber `is_live` + context clone + HandleScope/ContextScope/TryCatch +
/// WARN-on-throw. Notify-only — each handler is called with NO args (the prelude wrapper builds
/// the PrecacheContext) and its return is ignored.
pub(crate) fn dispatch_precache() {
    let snap = PRECACHE_MUX.with(|m| m.borrow().snapshot(""));
    if snap.is_empty() { return; }

    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, generation, handler_g) in &snap {
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let func = v8::Local::new(tc, handler_g);
            if func.call(tc, recv, &[]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_precache: handler '{}': {}", owner, msg));
            }
        }
    });
}
```

- [ ] **Step 5: Subscribe native.** Directly below `s2_map_start_subscribe` (`:4431`) — a verbatim mirror over `PRECACHE_MUX`:

```rust
/// `__s2_precache_subscribe(handler)` — subscribe a JS fn to the precache-manifest-build event.
/// Owner-tracked (mirrors `__s2_map_start_subscribe`); fixed mux key "". The handler is called
/// with no args during `dispatch_precache`; the `@s2script/sound` prelude wrapper constructs the
/// block-scoped PrecacheContext.
fn s2_precache_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        PRECACHE_MUX.with(|m| { m.borrow_mut().subscribe("", owner, generation, handler_g); });
    }));
}
```

Register it beside `__s2_map_start_subscribe` (`:6048`):

```rust
    set_native(scope, global_obj, "__s2_precache_subscribe", s2_precache_subscribe);
```

- [ ] **Step 6: Teardown wiring.** Beside the `MAP_MUX` reset in `shutdown()` (`:7865`):

```rust
    PRECACHE_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());
```

and beside the `MAP_MUX` `remove_by_owner` in plugin unload (`:8199`):

```rust
    PRECACHE_MUX.with(|m| m.borrow_mut().remove_by_owner(id));
```

- [ ] **Step 7: FFI export.** In `core/src/ffi.rs`, below `s2script_core_dispatch_map_start` (`:115`):

```rust
/// Shim → core: the CGameRulesGameSystem::OnPrecacheResource manual hook reports that the session
/// resource manifest is being built (Sound slice). The live IResourceManifest* is stashed
/// shim-side around this call for the `sound_precache_add` op (block-scoped — cleared when the
/// hook returns). Notify-only: dispatches to the `Sound.onPrecache` JS subscribers.
/// `catch_unwind`-wrapped (never panic across the FFI boundary).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_precache() {
    let _ = catch_unwind(|| {
        v8host::dispatch_precache();
    });
}
```

And declare it in `shim/include/s2script_core.h`, below `s2script_core_dispatch_map_start` (`:348`):

```c
/* Shim -> core: the CGameRulesGameSystem::OnPrecacheResource manual hook reports the session
 * resource-manifest build (Sound slice). The shim stashes the live IResourceManifest* around this
 * call so the sound_precache_add op can AddResource into it; the stash is cleared when this
 * returns (block-scoped — a handler must use its PrecacheContext synchronously). Notify-only:
 * runs the JS Sound.onPrecache subscribers. */
void s2script_core_dispatch_precache(void);
```

- [ ] **Step 8: Run the test — expect PASS** — `cd core && cargo test precache_dispatch` → PASS; `cd core && cargo test` → all green; both boundary gates green.

- [ ] **Step 9: Commit**
```bash
git add core/src/v8host.rs core/src/ffi.rs shim/include/s2script_core.h
git commit -F - <<'EOF'
feat(sound): PRECACHE_MUX + dispatch_precache + the FFI export

A new notify-mux for the precache-manifest build: the shim's OnPrecacheResource
manual hook (Task 5) calls s2script_core_dispatch_precache() -> PRECACHE_MUX
(event_mux reuse, fixed "" key) -> the Sound.onPrecache subscribers, each called
with no args (the prelude wrapper builds the block-scoped PrecacheContext).
Dispatch mirrors dispatch_map_start verbatim (snapshot-release, try_borrow_mut
re-entrancy guard, is_live, per-sub TryCatch); torn down on unload/shutdown.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 4: The `@s2script/sound` module + `packages/sound` + `BUILTIN_MODULES`

**Files:**
- Modify: `core/src/v8host.rs` (the prelude module after `globalThis.__s2pkg_clients = …` `:1785`; module tests)
- Modify: `core/src/loader.rs` (`BUILTIN_MODULES` `:63-71`)
- Create: `packages/sound/package.json`, `packages/sound/index.d.ts`

**Interfaces:**
- Consumes: `__s2_sound_emit`/`__s2_sound_precache_add` (Task 1), `__s2_precache_subscribe` (Task 3), the prelude-scope `__s2_MAX_CLIENTS` (`:1741`) + `__s2_client_valid` (both in the same `INJECTED_STD_PRELUDE` IIFE scope — the insertion point at `:1785` is after both).
- Produces: `globalThis.__s2pkg_sound = { Sound }` with `Sound.emit(name: string, opts?: { entity?: EntityRef, recipients?: number[], volume?: number }) -> number` and `Sound.onPrecache(handler: (ctx: { add(path: string): boolean }) => void)`. Consumed by Tasks 6/7 exactly as typed here.

- [ ] **Step 1: Write the failing tests** (in the test module, near the Task-1 sound tests):

```rust
    /// @s2script/sound module surface (defaults): no entity -> worldspawn (0, -1); no recipients ->
    /// the all-valid-clients enumeration (client_valid is None under mock_event_ops -> empty ->
    /// the op still receives slotCount 0); volume defaults 1.0.
    #[test]
    fn sound_module_emit_defaults() {
        let _ = init(dummy_logger());
        SOUND_EMIT_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(S2EngineOps { sound_emit: Some(mock_sound_emit), ..mock_event_ops() }));
        create_plugin_context("psd");
        let out = eval_in_context_string("psd",
            "String(__s2pkg_sound.Sound.emit('Weapon_AK47.Single'))");
        assert_eq!(out, "7");
        let calls = SOUND_EMIT_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, 0);                 // worldspawn index
        assert_eq!(calls[0].2, -1);                // the no-serial-check sentinel
        assert_eq!(calls[0].3, Vec::<i32>::new()); // no valid clients under the mock ops
        assert!((calls[0].4 - 1.0).abs() < 1e-6);  // default volume
        shutdown();
    }

    /// @s2script/sound module surface (explicit opts): entity {index,serial} -> (idx, serial);
    /// recipients passed through; volume passed through. And the module resolves via require.
    #[test]
    fn sound_module_emit_explicit_opts() {
        let _ = init(dummy_logger());
        SOUND_EMIT_CALLS.lock().unwrap().clear();
        set_engine_ops(Some(S2EngineOps { sound_emit: Some(mock_sound_emit), ..mock_event_ops() }));
        load_plugin_js("psx", r#"
            const { Sound } = require("@s2script/sound");
            globalThis.__g = Sound.emit("UI.PlayerPing",
                { entity: { index: 42, serial: 99 }, recipients: [3, 5], volume: 0.5 });
        "#);
        assert_eq!(eval_in_context_string("psx", "String(globalThis.__g)"), "7");
        let calls = SOUND_EMIT_CALLS.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "UI.PlayerPing");
        assert_eq!(calls[0].1, 42);
        assert_eq!(calls[0].2, 99);
        assert_eq!(calls[0].3, vec![3, 5]);
        assert!((calls[0].4 - 0.5).abs() < 1e-6);
        shutdown();
    }

    /// Sound.onPrecache wraps the raw subscribe: the handler receives a ctx whose add() hits the
    /// (absent) op and returns false; the ctx is freshly built per dispatch.
    #[test]
    fn sound_module_onprecache_builds_ctx() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("ppx");
        eval_in_context_string("ppx", r#"
            globalThis.__ctxAdd = null;
            __s2pkg_sound.Sound.onPrecache(function (ctx) {
                globalThis.__ctxAdd = ctx.add("soundevents/y.vsndevts");
            });
            "ok"
        "#);
        dispatch_precache();
        assert_eq!(eval_in_context_string("ppx", "String(globalThis.__ctxAdd)"), "false");
        shutdown();
    }
```

- [ ] **Step 2: Run — expect FAIL** — `cd core && cargo test sound_module` → FAIL (`__s2pkg_sound` undefined).

- [ ] **Step 3: The prelude module.** In `INJECTED_STD_PRELUDE`, directly after `globalThis.__s2pkg_clients = { Client: Client, Clients: __s2_clients };` (`:1785` — this point is after both `__s2_MAX_CLIENTS` and `__s2_client_valid` uses in the same IIFE scope):

```js
  // --- @s2script/sound — engine-generic sound (Sound slice). A soundevent NAME, a recipient slot
  //     set, and a precache resource path are Source2-generic; CS2 soundevent names live in the
  //     game layer (games/cs2/js/pawn.js `Sounds`), never here.
  //     emit: no entity -> worldspawn (index 0, serial sentinel -1 = no serial gate) = a global/2D
  //     sound; no recipients -> every valid client slot (bot slots are additionally skipped
  //     shim-side — no netchannel). Returns the engine SndOpEventGuid (nonzero) or 0.
  //     onPrecache: handler(ctx) gets a BLOCK-SCOPED PrecacheContext — ctx.add(path) is valid only
  //     during the dispatch (the shim's manifest stash is live only then; a stashed ctx used after
  //     the handler returns is a no-op false). Fires at map load / mapchange. ---
  var __s2_sound = {
    emit: function (name, opts) {
      opts = opts || {};
      var idx = 0, serial = -1;                    // worldspawn / global-2D default
      var e = opts.entity;
      if (e && typeof e.index === "number" && typeof e.serial === "number") {
        idx = e.index | 0; serial = e.serial | 0;
      }
      var slots = opts.recipients;
      if (!Array.isArray(slots)) {
        slots = [];
        for (var s = 0; s < __s2_MAX_CLIENTS; s++) if (__s2_client_valid(s)) slots.push(s);
      }
      var vol = (opts.volume == null) ? 1.0 : +opts.volume;
      return __s2_sound_emit(String(name), idx, serial, slots, vol);
    },
    onPrecache: function (h) {
      __s2_precache_subscribe(function () {
        h({ add: function (p) { return __s2_sound_precache_add(String(p)); } });
      });
    },
  };
  globalThis.__s2pkg_sound = { Sound: __s2_sound };   // named export `Sound`
```

- [ ] **Step 4: `BUILTIN_MODULES`.** In `core/src/loader.rs` (`:63-71`), add `"@s2script/sound"` to the array (append inside the last line, keeping the format):

```rust
    "@s2script/trace", "@s2script/usermessages", "@s2script/math", "@s2script/events",
    "@s2script/cs2", "@s2script/sound",
```

- [ ] **Step 5: `packages/sound/package.json`** (mirror `packages/net/package.json`, new package starts at 0.0.0 — the Task-7 changeset bumps it to 0.1.0):

```json
{
  "name": "@s2script/sound",
  "version": "0.0.0",
  "types": "index.d.ts",
  "publishConfig": {
    "access": "public"
  },
  "files": [
    "index.d.ts"
  ],
  "repository": {
    "type": "git",
    "url": "https://github.com/GabeHirakawa/s2script.git"
  }
}
```

- [ ] **Step 6: `packages/sound/index.d.ts`:**

```ts
/** @s2script/sound — engine-generic sound: emit a named SoundEvent + register custom precache paths. */
import type { EntityRef } from "@s2script/entity";

export interface SoundEmitOptions {
  /** Source entity (serial-gated; a stale ref emits nothing and returns 0). Omitted -> worldspawn
   *  (a global/2D sound). */
  entity?: EntityRef;
  /** Recipient player slots. Omitted -> every valid client. Bot slots are always skipped
   *  (no netchannel); an all-bot-skipped set still emits to nobody (a real, safe engine call),
   *  whereas requesting NO recipients (an empty array) returns 0 without emitting. */
  recipients?: number[];
  /** Volume in [0, 1]. Default 1.0 (out-of-range/NaN clamps to 1.0). */
  volume?: number;
}

/** Block-scoped precache context — valid ONLY during the onPrecache dispatch. Synchronous use
 *  only: a stashed context used after the handler returns (or past an await) is a no-op `false`
 *  (the engine manifest is gone). */
export interface PrecacheContext {
  /** Add a resource path (e.g. "soundevents/mypack.vsndevts") to the session resource manifest.
   *  True iff the engine accepted the add. */
  add(path: string): boolean;
}

export declare const Sound: {
  /** Play a named SoundEvent (the engine resolves name->hash; built-in soundevents need no
   *  precache). Returns the engine sound GUID (nonzero) or 0 on failure (unresolved engine
   *  function / stale source entity / an empty `recipients` array). An all-bot-skipped recipient
   *  set still emits to nobody (a real engine call, may return a nonzero GUID). */
  emit(name: string, opts?: SoundEmitOptions): number;
  /** Subscribe to the session resource-manifest build (fires at map load / mapchange). Register
   *  custom .vsndevts/.vsnd content here so a later emit can play it. A plugin hot-loaded mid-map
   *  gets its first fire at the NEXT map change. */
  onPrecache(handler: (ctx: PrecacheContext) => void): void;
};
```

- [ ] **Step 7: Run — expect PASS** — `cd core && cargo test sound_module` → PASS; `cd core && cargo test` → all green; both boundary gates green.

- [ ] **Step 8: Commit**
```bash
git add core/src/v8host.rs core/src/loader.rs packages/sound
git commit -F - <<'EOF'
feat(sound): @s2script/sound module (Sound.emit + Sound.onPrecache) + types

Engine-generic prelude module __s2pkg_sound: emit defaults to worldspawn
(index 0, serial sentinel -1) + all-valid-client recipients (__s2_client_valid
enumeration) + volume 1.0; onPrecache wraps __s2_precache_subscribe and builds
a fresh block-scoped PrecacheContext per dispatch. New types-only
packages/sound; "@s2script/sound" added to loader BUILTIN_MODULES.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 5: Precache shim — factory-list resolve, the manual `OnPrecacheResource` hook, `sound_precache_add`

**Files:**
- Modify: `gamedata/core.gamedata.jsonc` (`GameSystemFactoryList` signature; `CGameRulesGameSystem_OnPrecacheResource` offsets entry after `CCSPlayer_ItemServices_DropActivePlayerWeapon`)
- Modify: `shim/src/s2script_mm.cpp` (manual-hook decl near the SH_DECLs `:114`; statics + `s2_sound_precache_add` + `Hook_OnPrecacheResource` + `TryInstallPrecacheHook`; `Load()` sig resolution + offsets pick + install call; the `Hook_StartupServer` lazy retry `:2969`; the Unload removal beside `:2823`)
- Modify: `shim/src/s2script_mm.h` (decls + `m_precacheHookInstalled` beside `m_startupServerHookInstalled` `:78`)

**Interfaces:**
- Consumes: `s2script_core_dispatch_precache()` (Task 3's export, declared in the header); `LoadSignatures`/`ResolveSigValidated`/`FindModuleText`/`GamedataResult` (`"lea-disp"` resolve: the pattern must START at the rip-relative instruction — `ResolveSigValidated` hardcodes dispOff=3/instrLen=7, `:1628`); `LoadOffsets` + the `pick()` lambda (`:2556-2558`); `IsAddressInServerText`; SourceHook manual hooks (`SH_DECL_MANUALHOOK1_void` / `SH_MANUALHOOK_RECONFIGURE` / `SH_ADD_MANUALHOOK` / `SH_REMOVE_MANUALHOOK` — the FIRST manual hook in the shim; the interface hooks at `:2086-2187` are the style model).
- Produces: `static int s2_sound_precache_add(const char* path)` (satisfies Task 2's forward decl); `void S2ScriptPlugin::Hook_OnPrecacheResource(void* pManifest)`; `bool S2ScriptPlugin::TryInstallPrecacheHook()`.

- [ ] **Step 1: Offline RE — the factory-list global, the node layout, and the vtable index.** All against the pinned `/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so`, using the Task-2 `sigscan-offline.py`. Three sub-findings, each recorded in the gamedata comments:
  1. **Locate `UTIL_GetGameSystemFactory`** via the ModSharp hint `48 8D 05 ? ? ? ? 48 89 77 ? 48 89 07 48 8B 05` (scan; expect 1 match). Disassemble (`objdump -d --start-address=0x<vaddr> --stop-address=0x<vaddr+0x100> <so>`). ModSharp derives the list-head global from this function at `+17 r d` — i.e. the `48 8B 05 <disp32>` (`mov rax, [rip+disp]`) instruction at match+14 whose displacement (at +17) targets the storage of `CBaseGameSystemFactory::sm_pFirst`. **Author the committed `GameSystemFactoryList` pattern to START at that `48 8B 05` instruction** (so our fixed `lea-disp` dispOff=3/instrLen=7 applies): take `48 8B 05 ? ? ? ?` plus enough of the FOLLOWING bytes (from the disasm) to make it UNIQUE (re-scan until exactly 1 match). Record the resolved data vaddr.
  2. **Confirm the node layout + indirection depth** from the same disasm: the function walks the list comparing names — expect ONE deref of the rip-relative slot to get the head node (CSSharp assigns the resolved address to a `CBaseGameSystemFactory** sm_pFirst`, i.e. resolved-addr → head-node-ptr → node), then per-node reads of the name at `[node+16]` and the next pointer at `[node+8]`. If the disasm shows different offsets or an extra deref, adjust `S2GsFactoryNode` (Step 3) and record it.
  3. **Confirm `m_pInstance` (+24) and the OnPrecacheResource index (7).** `strings -t x <so> | grep -n "GameSystemStaticFactory"` → find the RTTI typeinfo-name for the `CGameRulesGameSystem` static factory; locate its vtable (scan for pointers to the typeinfo in the data segments — extend `sigscan-offline.py` to search 8-byte little-endian values, or use `objdump -s -j .data.rel.ro`); disassemble the vtable's slot bodies and find the trivial accessor `mov rax, [rdi+0x18]; ret` (GetStaticGameSystem) — the `0x18` confirms `m_pInstance` at +24. If N differs, update the struct + record it. For the vtable index: the ModSharp gamedata gives `CGameRulesGameSystem::OnPrecacheResource` = 7 on linux AND windows (a strong cross-platform signal); it stays a gamedata offsets entry (a HINT) because the runtime install additionally requires `vtbl[idx]` to land in libserver `.text` before hooking. If sub-finding 3's vtable walk is tractable, also disassemble slot 7 of the CGameRulesGameSystem (game-system) vtable itself and sanity-check the body (it should read/loop over the manifest arg, not look like `GameInit`/`GameShutdown` stubs).

- [ ] **Step 2: gamedata.** Append to `signatures` (after `EmitSound`):

```jsonc
    // CBaseGameSystemFactory::sm_pFirst — the game-system factory-list head storage (Sound slice:
    // precache). The pattern starts AT the `mov rax,[rip+disp]` inside UTIL_GetGameSystemFactory
    // that loads the list head (ModSharp anchors the same storage at UTIL_GetGameSystemFactory+17;
    // our fixed lea-disp resolve is dispOff=3/instrLen=7, so the pattern is authored to begin on
    // the mov itself), trailing bytes from the Task-5 offline disasm for uniqueness. Resolves to
    // the DATA address holding the head-node pointer (one deref -> the first
    // CBaseGameSystemFactory node: vtbl@0, m_pNext@8, m_pName@16, m_pInstance@24 —
    // disasm-confirmed in the Task-5 offline step). Unresolved -> the precache hook is never
    // installed (onPrecache never fires; emit unaffected).
    "GameSystemFactoryList": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "48 8B 05 ? ? ? ? <TRAILING BYTES FROM THE STEP-1 DISASM — re-scan until UNIQUE>",
        "resolve": "lea-disp"
      }
    }
```

(As with `EmitSound`: the committed pattern MUST be the offline-validated unique one; the placeholder text above is replaced by real bytes before commit.) And append to `offsets` (after `CCSPlayer_ItemServices_DropActivePlayerWeapon`):

```jsonc
    // Sound slice: CGameRulesGameSystem::OnPrecacheResource's vtable INDEX (not a byte offset) —
    // the precache manual-hook position. Borrowed from the ModSharp gamedata (linux AND windows
    // both 7 — a strong cross-platform signal) — per the RE doctrine a borrowed index is a HINT:
    // TryInstallPrecacheHook validates the resolved vtbl[idx] lands inside libserver.so's own
    // .text range BEFORE hooking; a stale/wrong index leaves the hook uninstalled (onPrecache
    // never fires), never a crash.
    "CGameRulesGameSystem_OnPrecacheResource": { "linuxsteamrt64": 7 }
```

- [ ] **Step 3: Shim statics + the op + the manual hook decl.** In `shim/src/s2script_mm.cpp`, near the other SH_DECLs (`:114`):

```cpp
// Precache manual hook (Sound slice) — the FIRST manual SourceHook in the shim: CGameRulesGameSystem
// is not an SDK-declared interface, so OnPrecacheResource is hooked by VTABLE POSITION (declared
// index 0 here; SH_MANUALHOOK_RECONFIGURE applies the gamedata index at install time). Signature:
// void OnPrecacheResource(CGameRulesGameSystem* this, IResourceManifest* pManifest) — the arg is
// carried as void* (the manifest type stays opaque; only its vtable slot 0 is called).
SH_DECL_MANUALHOOK1_void(GameRules_OnPrecacheResource, 0, 0, 0, void*);
```

Then, directly below the Task-2 emit block (replacing Task 2's forward declaration of `s2_sound_precache_add` — delete that line):

```cpp
// ---------------------------------------------------------------------------
// Sound slice — precache. CS2 builds the session resource manifest at map load; custom resources
// are added by hooking the EXISTING CGameRulesGameSystem's OnPrecacheResource(IResourceManifest*)
// (a manual SourceHook at the gamedata vtable index — the ModSharp mechanism, decompile-confirmed;
// NOT a new game-system registration, CSSharp's heavier fallback). The instance is found by
// walking the game-system factory list from the sig-resolved sm_pFirst storage
// ("GameSystemFactoryList"): node layout vtbl@0 / m_pNext@8 / m_pName@16 / m_pInstance@24
// (disasm-confirmed offline against UTIL_GetGameSystemFactory's own walk + the static factory's
// GetStaticGameSystem accessor). Degrade-never-crash: any unresolved step leaves the hook
// uninstalled (onPrecache never fires; emit unaffected). The manifest pointer is stashed ONLY for
// the synchronous duration of the hook dispatch — it never crosses to JS.
// ---------------------------------------------------------------------------
struct S2GsFactoryNode {                  // minimal CBaseGameSystemFactory view (offsets RE-confirmed)
    void**           vtbl;                // +0
    S2GsFactoryNode* m_pNext;             // +8
    const char*      m_pName;             // +16
    void*            m_pInstance;         // +24 (the static factory's instance slot)
};
static S2GsFactoryNode** s_ppGameSystemFactoryList = nullptr;  // sig-resolved &sm_pFirst storage
static void* s_pGameRulesGameSystem = nullptr;                 // the hooked instance (for removal)
static void* s_currentPrecacheManifest = nullptr;              // live ONLY during the hook dispatch
static int   s_precacheVtblIdx = -1;                           // gamedata offsets entry

// The sound_precache_add op. Block-scoped: valid only while the hook stash is live.
// IResourceManifest::AddResource(const char*) is vtable slot 0 (ModSharp decompile-confirmed:
// mov (%rdi),%rdi; mov (%rdi),%rax; mov (%rax),%rax; jmp *%rax). .text-guarded per call.
static int s2_sound_precache_add(const char* path) {
    if (!s_currentPrecacheManifest || !path || !path[0]) return 0;
    void** vtbl = *reinterpret_cast<void***>(s_currentPrecacheManifest);
    if (!vtbl) return 0;
    void* fn = vtbl[0];
    if (!IsAddressInServerText(fn)) return 0;
    reinterpret_cast<void (*)(void*, const char*)>(fn)(s_currentPrecacheManifest, path);
    return 1;
}
```

- [ ] **Step 4: Header decls.** In `shim/src/s2script_mm.h`, next to `Hook_StartupServer` (`:68`) and the flag block (`:78`):

```cpp
    // Precache hook (Sound slice) — a MANUAL SourceHook on CGameRulesGameSystem::OnPrecacheResource
    // (vtable index from gamedata; the instance factory-list-resolved). Stashes the live
    // IResourceManifest* for the sound_precache_add op, dispatches Sound.onPrecache, clears the
    // stash. The manifest arg is void* (opaque engine type; META_NO_HL2SDK discipline).
    void Hook_OnPrecacheResource(void* pManifest);
    // Idempotent installer: resolves the CGameRulesGameSystem instance off the factory list and
    // installs the manual hook. Called at Load and retried from Hook_StartupServer each map start
    // until installed (the factory/instance may not exist at Load).
    bool TryInstallPrecacheHook();
```

```cpp
    bool m_precacheHookInstalled = false;          // Sound slice: the OnPrecacheResource manual hook
```

- [ ] **Step 5: Hook body + installer.** In `shim/src/s2script_mm.cpp`, near the other `Hook_*` bodies:

```cpp
// The manifest is an argument — live for exactly this call. Stash -> dispatch (core fans out to
// the Sound.onPrecache subscribers, whose ctx.add() -> s2_sound_precache_add -> AddResource on the
// stash) -> clear. PRE hook (the game's own precache adds run after ours via the original; the
// manifest accepts adds either side — PRE mirrors ModSharp).
void S2ScriptPlugin::Hook_OnPrecacheResource(void* pManifest) {
    s_currentPrecacheManifest = pManifest;
    s2script_core_dispatch_precache();
    s_currentPrecacheManifest = nullptr;
    RETURN_META(MRES_IGNORED);
}

bool S2ScriptPlugin::TryInstallPrecacheHook() {
    if (m_precacheHookInstalled) return true;
    if (!s_ppGameSystemFactoryList || s_precacheVtblIdx < 0) return false;
    S2GsFactoryNode* node = *s_ppGameSystemFactoryList;
    void* inst = nullptr;
    for (int guard = 0; node && guard < 1024; node = node->m_pNext, guard++) {
        if (node->m_pName && strcmp(node->m_pName, "CGameRulesGameSystem") == 0) {
            inst = node->m_pInstance;
            break;
        }
    }
    if (!inst) {
        // One-time diagnostic: dump the registered factory names so a renamed factory is a
        // data/name fix from the boot log, not a debugging session. (Bounded; names are engine
        // string literals.)
        static bool s_dumpedFactories = false;
        if (!s_dumpedFactories && *s_ppGameSystemFactoryList) {
            s_dumpedFactories = true;
            META_CONPRINTF("[s2script] precache: CGameRulesGameSystem factory not found; registered factories:\n");
            S2GsFactoryNode* n = *s_ppGameSystemFactoryList;
            for (int i = 0; n && i < 64; n = n->m_pNext, i++)
                META_CONPRINTF("[s2script]   factory[%d] = %s\n", i, n->m_pName ? n->m_pName : "<null>");
        }
        return false;   // not registered yet — the StartupServer retry gets another chance
    }
    void** vtbl = *reinterpret_cast<void***>(inst);
    if (!vtbl || !IsAddressInServerText(vtbl[s_precacheVtblIdx])) {
        META_CONPRINTF("[s2script] WARN: precache — OnPrecacheResource vtable[%d] out of libserver .text; hook OFF\n",
                       s_precacheVtblIdx);
        m_precacheHookInstalled = true;   // poison: don't retry into a bad vtable every map start
        return false;
    }
    SH_MANUALHOOK_RECONFIGURE(GameRules_OnPrecacheResource, s_precacheVtblIdx, 0, 0);
    SH_ADD_MANUALHOOK(GameRules_OnPrecacheResource, inst,
                      SH_MEMBER(this, &S2ScriptPlugin::Hook_OnPrecacheResource), false);   // PRE
    s_pGameRulesGameSystem = inst;
    m_precacheHookInstalled = true;
    META_CONPRINTF("[s2script] precache hook installed (CGameRulesGameSystem @%p, vtbl idx %d)\n",
                   inst, s_precacheVtblIdx);
    return true;
}
```

NOTE for the reviewer: the "poison" arm sets `m_precacheHookInstalled = true` WITHOUT `s_pGameRulesGameSystem`, so Unload's removal (guarded on `s_pGameRulesGameSystem`) correctly no-ops.

- [ ] **Step 6: `Load()` wiring.** (a) In the signature block, after the `EmitSound` resolution (Task 2's block):

```cpp
            // Sound slice: resolve the game-system factory-list head storage (precache hook).
            // lea-disp: the unique match starts at the mov rax,[rip+disp] that loads sm_pFirst;
            // the resolved DATA address is the head-pointer storage (one deref = the first node).
            auto gflit = sigs.find("GameSystemFactoryList");
            if (gflit == sigs.end()) {
                GamedataResult("GameSystemFactoryList", false, "signature absent from gamedata");
            } else {
                int64_t gflOff = ResolveSigValidated("GameSystemFactoryList", gflit->second);
                ModText gflmt = FindModuleText(gflit->second.module.c_str());
                if (gflOff != s2sig::kFail && gflmt.text) {
                    s_ppGameSystemFactoryList = reinterpret_cast<S2GsFactoryNode**>(
                        const_cast<uint8_t*>(gflmt.text) + gflOff);
                    META_CONPRINTF("[s2script] GameSystemFactoryList resolved @%p (precache)\n",
                                   (void*)s_ppGameSystemFactoryList);
                }   // gflOff == kFail: ResolveSigValidated already recorded the reason
            }
```

(b) In the offsets block (beside `s_teleportVtblIndex = pick("CBaseEntity_Teleport");` `:2563`):

```cpp
            // Sound slice: the OnPrecacheResource vtable index (a HINT — TryInstallPrecacheHook
            // .text-validates vtbl[idx] before hooking; see the gamedata comment).
            s_precacheVtblIdx = pick("CGameRulesGameSystem_OnPrecacheResource");
            GamedataResult("CGameRulesGameSystem_OnPrecacheResource", s_precacheVtblIdx >= 0,
                           "offset (vtable index) key absent from gamedata");
```

(c) After the whole gamedata block in `Load()` (after the offsets `{ … }` closes), attempt the install:

```cpp
        // Sound slice: install the precache hook now if the factory list is already populated;
        // otherwise Hook_StartupServer retries each map start (idempotent).
        TryInstallPrecacheHook();
```

- [ ] **Step 7: The `Hook_StartupServer` lazy retry.** In `Hook_StartupServer` (`:2969`), before the `s2script_core_dispatch_map_start(...)` call:

```cpp
    // Sound slice: lazy precache-hook retry — the CGameRulesGameSystem factory/instance may not
    // exist at Load; each map startup is another chance (no-op once installed/poisoned). Timing
    // note: whether THIS map's manifest build follows StartupServer is engine-ordering; if the
    // Load-time install missed, the hook may only catch the NEXT map change — an accepted degrade.
    TryInstallPrecacheHook();
```

- [ ] **Step 8: Unload removal.** Beside the `StartupServer` `SH_REMOVE_HOOK` (`:2823`):

```cpp
    if (m_precacheHookInstalled && s_pGameRulesGameSystem) {
        SH_REMOVE_MANUALHOOK(GameRules_OnPrecacheResource, s_pGameRulesGameSystem,
                             SH_MEMBER(this, &S2ScriptPlugin::Hook_OnPrecacheResource), false);
        m_precacheHookInstalled = false;
        s_pGameRulesGameSystem = nullptr;
    }
```

- [ ] **Step 9: Gates.** `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` → green (`CGameRulesGameSystem`/`IResourceManifest` names appear only in shim + gamedata, never `core/src`). `cd core && cargo test` → all green (regression sanity).

- [ ] **Step 10: Commit**
```bash
git add gamedata/core.gamedata.jsonc shim/src/s2script_mm.cpp shim/src/s2script_mm.h
git commit -F - <<'EOF'
feat(sound): precache hook — factory-list resolve + manual OnPrecacheResource hook

The shim's first MANUAL SourceHook: CGameRulesGameSystem::OnPrecacheResource
(vtable index from gamedata, .text-validated before hooking) on the instance
found by walking the sig-resolved game-system factory list (sm_pFirst storage,
lea-disp; node layout disasm-confirmed offline). The hook stashes the live
IResourceManifest* around s2script_core_dispatch_precache() (block-scoped);
sound_precache_add calls AddResource (vtable slot 0, .text-guarded). Idempotent
installer at Load + a StartupServer retry; a bad vtable index poisons the
installer (hook off, no per-map retry, never a crash); factory-name miss dumps
the registered names once for a data-only fix.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 6: CS2 sugar — `pawn.emitSound` + `Sounds`

**Files:**
- Modify: `games/cs2/js/pawn.js` (`Pawn.prototype.emitSound` below `giveNamedItem` `:193`; `Sounds` above the tail export; the tail `Object.assign` export line)
- Modify: `packages/cs2/index.d.ts` (`emitSound` in `interface Pawn` `:24-76`; `export declare const Sounds` after the `TriggerZone` export block `:221`)

**Interfaces:**
- Consumes: `globalThis.__s2pkg_sound.Sound` (Task 4), `this.ref` (`EntityRef {index, serial}` on `Pawn`).
- Produces: `Pawn.prototype.emitSound(name: string, opts?: { recipients?: number[]; volume?: number }): number`; `Sounds` (a curated map of built-in CS2 soundevent names).

- [ ] **Step 1: pawn.js.** Below `Pawn.prototype.giveNamedItem` (`:193-198`):

```js
  // pawn.emitSound(name, opts?) — play a named CS2 SoundEvent from this pawn (its serial-gated
  // EntityRef is the source entity; a stale ref emits nothing -> 0). opts = { recipients?: slots[],
  // volume?: [0,1] } — same as Sound.emit minus entity. Returns the engine sound GUID or 0.
  Pawn.prototype.emitSound = function (name, opts) {
    var pkg = globalThis.__s2pkg_sound;
    if (!pkg || !pkg.Sound) return 0;
    var o = opts || {};
    return pkg.Sound.emit(name, { entity: this.ref, recipients: o.recipients, volume: o.volume });
  };
```

Then, directly before the tail export (`globalThis.__s2pkg_cs2 = Object.assign(...)`):

```js
  // A small curated set of known-good BUILT-IN CS2 soundevents (convenience + the sound-demo).
  // CS2 soundevent names live exclusively HERE (the game layer), never in core/src. The audible
  // verify is a human-client test (bots have no audio) — tune/extend these names at that gate.
  var Sounds = {
    Ping:       "UI.PlayerPing",
    PingUrgent: "UI.PlayerPingUrgent",
    Ak47Shot:   "Weapon_AK47.Single",
    DeagleShot: "Weapon_DEagle.Single",
  };
```

And add `Sounds: Sounds` into the tail export's `Object.assign` object literal (the line currently ending `TriggerZone: TriggerZone });` becomes `… TriggerZone: TriggerZone, Sounds: Sounds });`).

- [ ] **Step 2: packages/cs2 types.** In `packages/cs2/index.d.ts`, inside `export interface Pawn` (after `aimTrace` `:75`):

```ts
  /** Play a named CS2 SoundEvent from this pawn (the serial-gated source entity; a stale ref emits
   *  nothing). Returns the engine sound GUID (nonzero) or 0. Bot recipients are always skipped. */
  emitSound(name: string, opts?: { recipients?: number[]; volume?: number }): number;
```

and after the `TriggerZone` export block (`:221+`):

```ts
/** Curated built-in CS2 soundevent names (see @s2script/sound `Sound.emit` / `Pawn.emitSound`). */
export declare const Sounds: {
  readonly Ping: string;
  readonly PingUrgent: string;
  readonly Ak47Shot: string;
  readonly DeagleShot: string;
};
```

- [ ] **Step 3: Verify.** `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` → green (the soundevent strings are in the game layer). `bash scripts/check-plugins-typecheck.sh` → green (no plugin consumes the new surface yet — regression sanity).

- [ ] **Step 4: Commit**
```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts
git commit -F - <<'EOF'
feat(cs2): pawn.emitSound + the curated Sounds constants

pawn.emitSound(name, opts) emits from the pawn's serial-gated EntityRef via
@s2script/sound; Sounds holds a small set of known-good built-in soundevent
names (CS2 strings stay in the game layer, never core/src; audible tuning is
the deferred human-client gate).

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 7: Demo plugin + full typecheck + changeset

**Files:**
- Create: `examples/sound-demo/package.json`, `examples/sound-demo/tsconfig.json`, `examples/sound-demo/src/plugin.ts`
- Create: `.changeset/sound-slice.md`

**Interfaces:**
- Consumes: `Sound`/`PrecacheContext` (`@s2script/sound`), `Pawn`/`Sounds` (`@s2script/cs2`), `Commands.register` (`@s2script/commands` — `ctx = { callerSlot, args, argString, reply }`, `args` excludes the command name).

- [ ] **Step 1: Scaffold** (mirror `examples/beam-demo`). `examples/sound-demo/package.json`:

```json
{
  "name": "@demo/sound-demo",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x" }
}
```

`examples/sound-demo/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/globals/globals.d.ts"]
}
```

- [ ] **Step 2: `examples/sound-demo/src/plugin.ts`:**

```ts
import { Commands } from "@s2script/commands";
import { Sound } from "@s2script/sound";
import { Pawn, Sounds } from "@s2script/cs2";

export function onLoad(): void {
  // Precache: fires at map load / mapchange. add() -> true proves the live manifest AddResource
  // path end-to-end (the file itself need not exist for the gate — the engine tolerates a
  // missing resource; a REAL custom .vsndevts playing is the deferred human-client test).
  Sound.onPrecache((ctx) => {
    const ok = ctx.add("soundevents/soundevents_s2script_demo.vsndevts");
    console.log(`[sound-demo] onPrecache fired — add() -> ${ok}`);
  });

  // sm_playsound [name] [slot]: with a slot — emit from that slot's pawn to that slot only
  // (exercises the serial-gated source + explicit recipients; a bot slot is skipped shim-side but
  // the engine is still CALLED with an empty filter -> a real EmitSound to nobody, the shim logs
  // "EmitSound ... recipients=0 -> guid=G"). Without — a worldspawn global broadcast to all valid
  // clients (on a bots server the default enumeration is the bot slots -> also all bot-skipped ->
  // still a real engine call).
  Commands.register("sm_playsound", (ctx) => {
    const name = ctx.args[0] || Sounds.Ping;
    if (ctx.args.length > 1) {
      const slot = parseInt(ctx.args[1], 10);
      const pawn = Pawn.forSlot(Number.isNaN(slot) ? -1 : slot);
      if (!pawn) {
        ctx.reply(`[sound-demo] no pawn at slot ${ctx.args[1]}`);
        return;
      }
      const guid = pawn.emitSound(name, { recipients: [slot] });
      ctx.reply(`[sound-demo] emitSound('${name}') from slot ${slot} -> guid=${guid}`);
    } else {
      const guid = Sound.emit(name);
      ctx.reply(`[sound-demo] Sound.emit('${name}') broadcast -> guid=${guid}`);
    }
  });

  console.log("[sound-demo] onLoad — sm_playsound registered");
}
```

- [ ] **Step 3: Build the demo** (the CLI is local — `npx s2script` 404s): `( cd packages/cli && node build.mjs ) && node packages/cli/dist/cli.js build examples/sound-demo` → produces the `.s2sp` (the full-strict typecheck gate must pass — this validates the two new `.d.ts` surfaces).

- [ ] **Step 4: Typecheck all** — `bash scripts/check-plugins-typecheck.sh` → all plugins + examples green.

- [ ] **Step 5: Changeset** — `.changeset/sound-slice.md`:

```md
---
"@s2script/sound": minor
"@s2script/cs2": minor
---

Sound slice: new `@s2script/sound` module — `Sound.emit(name, { entity?, recipients?, volume? })`
plays a named CS2 SoundEvent (engine GUID or 0; serial-gated source, bot recipients skipped) and
`Sound.onPrecache(ctx => ctx.add(path))` registers custom resources into the session manifest at
map load. CS2 sugar: `pawn.emitSound(name, opts)` + the curated `Sounds` constants.
```

- [ ] **Step 6: Commit**
```bash
git add examples/sound-demo .changeset/sound-slice.md
git commit -F - <<'EOF'
feat(demo): sound-demo (sm_playsound + onPrecache log) + changeset

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Build, live gate, and merge (after the Workflow — the human-in-the-loop section)

Not a Workflow task. **Coordinate the shared `s2script-cs2` server first — do NOT `docker compose restart cs2` while another gate is mid-run.**

- [ ] **Rebase** — `git fetch origin && gh pr view 23 --json state,mergedAt`; rebase `feat/sound` onto current `origin/main` (or PR #23's branch if still open and the ABI tail conflicts); re-verify the two sound ops sit after the FINAL tail across all four ABI touchpoints; `cd core && cargo test` green post-rebase.
- [ ] **Core tests + gates** — `cd core && cargo test` (serial) → all green (expect the 6 new sound tests in the count); both boundary gates + `scripts/check-plugins-typecheck.sh` green.
- [ ] **Submodule + sniper rebuild** — `git submodule update --init`, then `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh` (core `.so` + shim `.so` — the ops, the natives/prelude, the FFI export, and the shim hook all need it). Verify GLIBC floors unchanged (core ≤ 2.30, shim ≤ 2.14).
- [ ] **Re-deploy** — `scripts/package-addon.sh` (pawn.js in dist is a CONCAT — schema/nav/activity/csitem/pawn — never a raw `cp`); recreate `dist/addons/s2script/configs` (and `data/`) as gkh if the build wiped them; copy the active plugins' `.s2sp` incl. `sound-demo`; copy the updated `gamedata/core.gamedata.jsonc`.
- [ ] **Restart** — `cd docker && docker compose restart cs2` (NOT `--force-recreate`); if `gameinfo.gi` was reset (a game update), `docker exec s2script-cs2 /patch-gameinfo.sh` first.
- [ ] **Live gate** (de_inferno, `bot_quota 2`, `scripts/rcon.py`) — bots-provable:
  - **Boot:** `[s2script] EmitSound resolved @…`; `[s2script] GameSystemFactoryList resolved @…`; `[s2script] precache hook installed (CGameRulesGameSystem @…, vtbl idx 7)` (if the Load-time install missed, this line appears at the first `map start:` instead — the StartupServer retry); `=== GAMEDATA VALIDATION: (prior+3) ok, 0 FAILED ===` (EmitSound + GameSystemFactoryList + the OnPrecacheResource offsets entry); `[sound-demo] onLoad — sm_playsound registered`; `RestartCount=0`.
  - **Precache:** rcon `changelevel de_dust2` → `[sound-demo] onPrecache fired — add() -> true` (the definitive line: hook → FFI → mux → handler → ctx.add → live AddResource). If the factory-name walk missed, the boot log's `factory[i] = …` dump gives the correct name — fix the string, redeploy the shim, re-gate.
  - **Emit (the resolved fn IS exercised at the bots gate):** rcon `sm_playsound` → the default recipients = all valid clients = the 2 bot slots → the shim bot-skips both → the engine is CALLED anyway with an empty filter → the shim logs `[s2script] EmitSound 'UI.PlayerPing' recipients=0 -> guid=G` (the resolved fn FIRED with 0 live recipients — an all-bot-skipped filter is a real engine call — and returned a clean guid → sig + 24-byte sret ABI + prototype + no-crash all PROVEN live), and the demo replies `Sound.emit('UI.PlayerPing') broadcast -> guid=G`; `sm_playsound UI.PlayerPing 0` → the pawn resolves, slot 0 is bot-skipped, the engine is still called → the same `EmitSound … recipients=0 -> guid=G` log + demo reply; `sm_playsound "" junk-args` variants → replies, no crash. Server ticking throughout, 0 panics. (The `guid=G` may be nonzero — the engine returns a real GUID for a play-to-nobody call.)
  - **NOTE the honest ceiling:** the resolved `s_pEmitSound` call, its ABI/prototype, and no-crash ARE proven at the BOTS gate (the `EmitSound … recipients=0 -> guid=G` line). Only AUDIBILITY on a real listener + volume-on-a-real-ear are human-deferred (bots have no audio).
- [ ] **Document the human-client deferrals** (per `[[deferred-live-tests]]`): a real client HEARS `sm_playsound <name> <humanslot>` (this also first-exercises the live `s_pEmitSound` call + the RE'd prototype + volume semantics); a custom precached `.vsndevts` sound actually plays; tune/extend the `Sounds` names.
- [ ] **Merge** — per `[[slice-workflow-cadence]]`: push `feat/sound`, open the PR (changeset included), body ends with `https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`; update CLAUDE.md + memory after merge.

---

## Self-Review notes (author)

- **Spec coverage:** emit op + native + module + CS2 sugar (Tasks 1/2/4/6), the member-vs-static prototype decision procedure with both call variants in full (Task 2 Steps 1/4/5), the bot-skip + serial-gate + worldspawn sentinel + `.text` guard (Task 2 Step 4), precache manual hook + factory-list resolve + `AddResource` slot-0 + block-scoped stash (Task 5), `PRECACHE_MUX`/`dispatch_precache`/FFI export (Task 3), `packages/sound` + `BUILTIN_MODULES` (Task 4), demo + changeset (Task 7), degrade rules and the in-isolate test list (spec §Testing: degrade ×2, module surface ×3, marshalling ×1, ABI parity in both literals), the live gate incl. the human-audible deferral — every spec section has a task.
- **No placeholders except the two RE-derived byte strings** (`EmitSound` trailing-byte confirmation, `GameSystemFactoryList` pattern) — these are genuinely underivable before the offline scan; each carries the concrete hint bytes, the exact scanner script, the disasm procedure, the uniqueness criterion ("re-scan until exactly `1 match(es)`"), and the instruction that the COMMITTED value is the validated one.
- **Type consistency:** `s2_sound_emit_fn` (C) ↔ `SoundEmitFn` (Rust) ↔ `__s2_sound_emit(name, idx, serial, slots[], volume)` ↔ `Sound.emit` opts ↔ `packages/sound` `.d.ts` ↔ `pawn.emitSound` — checked arg-for-arg. `sound_precache_add` 1/0 ↔ native boolean ↔ `ctx.add(): boolean`. The Task-1 mock's tuple order matches the op's arg order.
- **Deviations from the spec sketch (flagged):** (a) the fallback `EmitSound_t` defaults use CSSharp's ctor verbatim (`m_nSourceSoundscape 0`, `m_nPitch PITCH_NORM=100`) instead of the spec sketch's `-1`/`0` — the spec's port source IS CSSharp's `entity_manager.h`, and those are the live-proven values; (b) `SndOpEventGuid_t` keeps CSSharp's 24-byte shape (with pad) in BOTH variants so the compiler emits the proven sret convention — the spec's prose "16-byte return" reading is superseded by the RE-confirm step; (c) `Sounds` ships 4 names (spec: "a few") marked tune-at-human-gate.
- **Known judgment calls (flagged for review):** (a) an all-bot-skipped (post-filter empty) recipient set CALLS the engine anyway — `sound_emit` returns 0 only when the CALLER requested no recipients (`slotCount <= 0`) — because a PVS/PAS filter excluding everyone is a normal, safe engine path (plays to nobody, no netchannel), more correct than a "failed" 0 for a bot-only target, AND it exercises the resolved fn + its sret ABI/prototype on the bots-only gate (spec-aligned per commit b49d2f5); (b) the precache hook is PRE (ModSharp parity) — the manifest accepts adds either side of the original; (c) the bad-vtable arm "poisons" the installer (no per-map retry into a bad index); (d) the factory-name literal `"CGameRulesGameSystem"` is best-effort until the live gate — the one-time factory-name dump makes a mismatch a data-only fix; (e) `m_pInstance`@+24 is deref'd only after the offline layout confirm — the reviewer should treat Task 5 Step 1 sub-finding 3 as load-bearing (a wrong offset would deref garbage BEFORE the vtable `.text` guard can catch it).

---

## Workflow Orchestration

**Execution order (strictly sequential** — Tasks 1/3/4 all modify `core/src/v8host.rs`; Tasks 2/5 both modify `shim/src/s2script_mm.cpp` + `gamedata/core.gamedata.jsonc`; Task 2 forward-declares what Task 5 defines**):**

1. **Task 1 — ABI plumbing + natives** (deterministic append first; everything downstream consumes these signatures).
2. **Task 2 — emit shim + offline RE** (the slice's riskiest RE; doing it early leaves time to fall back to Variant B).
3. **Task 3 — precache core** (the FFI export Task 5's hook calls; the subscribe native Task 4 wraps).
4. **Task 4 — the `@s2script/sound` module + package** (consumes Tasks 1+3).
5. **Task 5 — precache shim + offline RE** (satisfies Task 2's forward decl; consumes Task 3's export).
6. **Task 6 — CS2 sugar** (consumes Task 4).
7. **Task 7 — demo + typecheck + changeset** (consumes all; validates the `.d.ts` surfaces under full strict).

**Per-task shape:** implement agent → adversarial-review agent → fix (the slice cadence). The final opus review runs after Task 7, before the sniper build + live gate.

**Adversarial-review priorities (where to spend reviewer depth — the shim is never compiled locally):**
- **Task 2 (HIGHEST):** the RE decision record (member vs static; the `const float*`-is-volume evidence; the sret convention) vs the committed typedef/call; `S2RecipientFilter`'s override signatures vs `public/irecipientfilter.h` exactly (`NetChannelBufType_t`, `const CPlayerBitVec&`, `CPlayerSlot`); the bot-skip `GetPlayerNetInfo` guard; the degrade set (return 0 WITHOUT calling ONLY on unresolved/`.text`-fail/`!soundName`/stale-ent/`slotCount<=0`) vs the CALL-ANYWAY-on-empty-filter invariant (an all-bot-skipped filter must still call `s_pEmitSound` — NO `Count()==0` early-out; confirm the post-call `EmitSound … recipients=%d` log); the serial-gate vs sentinel branch; volume NaN clamp; the `Load()` block's kFail paths; the forward-decl-only `s2_sound_precache_add` note.
- **Task 5 (HIGHEST):** the factory-walk safety chain — indirection depth (ONE deref of the resolved storage), the node offsets vs the disasm record, the `m_pInstance` confirm BEFORE any deref of it, the bounded walk, the `.text` check on `vtbl[idx]` BEFORE `SH_ADD_MANUALHOOK`, the poison-vs-retry split, PRE + `RETURN_META(MRES_IGNORED)`, stash set/clear bracketing the dispatch, the Unload removal guard, `SH_DECL_MANUALHOOK1_void`/`SH_MANUALHOOK_RECONFIGURE` macro usage (the shim's first manual hook — no in-tree precedent).
- **Task 1 (HIGH):** the 4-touchpoint ABI append in exact order after the (post-rebase-verified) tail; the two test literals; the slot-array marshalling vs `s2_user_message_send`; `set_uint32` on the guid.
- **Tasks 3/4 (MEDIUM):** `dispatch_precache` as a faithful `dispatch_map_start` mirror (snapshot-release, `try_borrow_mut`, `is_live`, TryCatch, zero-arg call); BOTH teardown sites (a missed one leaks handler Globals across reloads); the prelude module's placement after `__s2_MAX_CLIENTS`'s scope + the fresh-ctx-per-dispatch wrapper; `BUILTIN_MODULES`.
- **Tasks 6/7 (LOW):** standard game-layer/demo/typecheck review; `Sounds` strings only in the game layer.

**What the Workflow cannot verify (deferred to the sniper build + live gate):** shim compilation (incl. the manual-hook macros and `irecipientfilter.h` include chain), the factory name string, the OnPrecacheResource index on the live vtable, and audibility on a real listener (human-client). The EmitSound prototype + its 24-byte sret ABI + no-crash ARE proven at the BOTS gate — the resolved fn is called with an all-bot-skipped filter (the `EmitSound … recipients=0 -> guid=G` log line), so only actual playback is human-deferred.

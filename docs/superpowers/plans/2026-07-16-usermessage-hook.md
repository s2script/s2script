# UserMessage interception (`UserMessages.onPre`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Synchronous pre-hook interception of outbound user messages — `UserMessages.onPre(name, handler)` with a block-scoped `UserMessageView` (typed scalar + dotted-nested protobuf reads, read-only recipients, `debugString` fallback) and suppression via the standard HookResult collapse — closing the TTT parity gap for SilentAWP / PoisonShots / SuppressedRound (msg 452 `CMsgTEFireBullets`) and BombPlantSuppressor (msg 322 `CCSUsrMsg_RadioText`), with the D-09 fix (typed `player`/`item_def_index`/`origin.x` reads replace DebugString coordinate parsing).

**Architecture (spec: `docs/superpowers/specs/2026-07-16-usermessage-hook-design.md`):** One new SourceHook — `SH_DECL_HOOK8_void(IGameEventSystem, PostEventAbstract, ...)` on the already-held `s_pGameEventSystem` (the exact overload our send path calls at `s2script_mm.cpp:922/:1027`) — lazily installed on first subscribe, gated by a 2048-bit registered-id bitmap read via `pEvent->GetNetMessageInfo()->m_MessageId`, dispatching by `GetUnscopedName()` into a new name-keyed collapsing `USERMSG_MUX` (a copy of `OUTPUT_MUX`/`dispatch_output`); collapsed result >= Handled(2) → `RETURN_META(MRES_SUPERCEDE)`. Field reads are `Get*` mirrors of the shipping `s2_user_message_set_*` reflection ops with dotted `GetMessage()` path-walking and `is_repeated()` FATAL guards. **Doctrine:** the single borrowed layout fact (`NetMessageInfo_t::m_MessageId`, `inetworkserializer.h:53`) is validated fail-closed at subscribe time (FindNetworkMessagePartial + id range + name substring) with an observe-only first-**subscribed**-fire sanity banner; `m_MessageId` feeds only the self-consistent bitmap pre-filter while `GetUnscopedName()` is the authoritative dispatch key, so a drifted offset degrades to a wasted dispatch or a fail-closed subscribe — never a false suppression. Eight new ops appended at the `S2EngineOps` tail. **No sig-scan, no gamedata, no new interface, no game-package change.** *(The first-fire code in Task 2 below is pre-review; see [Post-review revisions](#post-review-revisions) for what actually shipped.)*

**Tech Stack:** Rust core (`core/src/{v8host,ffi,event_mux(unchanged),multiplexer(unchanged)}.rs`), C++ shim (`shim/src/s2script_mm.{cpp,h}`, `shim/include/s2script_core.h`), `packages/sdk/usermessages.d.ts`, `examples/usermsg-demo`. Graphite stack of 4 PRs.

## Graphite stack map

| PR | Branch (gt) | Content |
|---|---|---|
| PR1 | `usermsg/design-docs` | This plan + the design spec |
| PR2 | `usermsg/engine` | ABI (8 ops, every place) + `USERMSG_MUX` + `dispatch_usermsg` + ffi export + natives + `UserMessages` prelude + core tests; shim: SH_DECL + `Hook_PostEvent` + bitmap + reflection reads + validation + suppress + unload removal + ops wiring |
| PR3 | `usermsg/sdk-surface` | `packages/sdk/usermessages.d.ts` hook API + changeset + typecheck gate |
| PR4 | `usermsg/consumers` | `examples/usermsg-demo` (4 TTT consumer shapes + D-09 probe) + full host gate run |

Worktree `/home/gkh/projects/s2script-usermsg` is on `feat/usermsg`. Create each task's branch with `gt create -am "<msg>"` (stacked; `gt submit --stack --no-interactive` at the end). In a worktree the branch may start untracked: `gt track -p main` first.

## Global Constraints

- **ABI-append discipline — every place, and a LIVE collision hazard.** The 8 new ops append after `transmit_stats` (the origin/main tail) in: (1) C typedefs after `s2_transmit_stats_fn` (`shim/include/s2script_core.h:259`), (2) struct members after `s2_transmit_stats_fn transmit_stats;` (`:386`, before `} S2EngineOps;`), (3) Rust type aliases + (4) struct field after `pub transmit_stats: Option<TransmitStatsFn>,` (`core/src/v8host.rs:372`), (5) EVERY test op-struct (`grep -n "transmit_stats" core/src/v8host.rs` → the `None` block ~:11233 and both `mock_transmit_stats` blocks ~:11716/:11874), (6) shim wiring after `ops.transmit_stats = &s2_transmit_stats;` (`s2script_mm.cpp:3589`). Order MUST match everywhere. **COLLISION:** the unmerged round-control (#67) and voice (#71) stacks append at this same tail; `S2EngineOps` has no size/version handshake — before EACH merge of this stack, `git log origin/main` for either having landed and re-anchor the usermsg ops after the NEW tail in all places. A missed re-anchor is a silent function-pointer misdispatch, not a compile error.
- **Boundary — both gates stay green.** Core speaks only strings (message names, field paths), ints (ids, slots), floats. NO CS2 identifier (`CMsgTEFireBullets`, `RadioText`, 452, 322, `item_def_index`) in `core/src` or the shim (the shim compares ids it was handed; its only literals are the validation banner strings). Gates per PR: `make check-boundary` + `bash scripts/test-boundary-nameleak.sh`.
- **RE doctrine:** the ONLY un-proven engine fact is the `NetMessageInfo_t::m_MessageId` header offset. It is load-validated fail-closed (subscribe-time round-trip + observe-only first fire, spec §2) — never a bare borrowed constant. Everything else (PostEventAbstract slot, clients-mask bit=slot, `GAMEEVENTSYSTEM_INTERFACE_VERSION`, libprotobuf reflection) is transitively proven by the shipping send path. Cite this in the PR2 body.
- **Hot path:** zero subscribers → no hook installed; non-subscribed message → one virtual call + one bit test, then `MRES_IGNORED` — no strcmp, no protobuf, no FFI, no allocation, no logging after first-fire. Reflection only after a bitmap hit AND only when a handler reads a field.
- **protobuf FATAL guard:** every reflection read checks `is_repeated()` (and `CPPTYPE_MESSAGE` for path segments) before any `Get*` — a scalar accessor on a repeated field aborts the whole process. Mirror the shipping guards in `s2_user_message_set_*`.
- **Block-scoped view:** `s_hookMsg`/`s_hookClients`/`s_hookClientCount` set at dispatch entry, nulled at dispatch exit; all ops null-guard (post-`await` reads → `null`). SEPARATE statics from the send builder's `s_umInfo`/`s_umData`/`s_umMsg` (`s2script_mm.cpp:977-979`) — a mid-hook send must not retarget the view.
- **Re-entrancy:** core `dispatch_usermsg` uses the `try_borrow_mut` graceful-skip (Continue); shim `s_inUserMsgDispatch` flag returns `MRES_IGNORED` before the FFI. Natives `catch_unwind`.
- **Degrade-never-crash:** unresolvable name / degraded descriptor → `onPre` throws at plugin load (that plugin fails loudly, framework runs); validation failure → named banner, hook inert, send path untouched.
- **Worktree gotcha:** `third_party/` submodules may be empty — `git submodule update --init --recursive` before any shim build.
- **Core tests are single-threaded** (`.cargo/config.toml`): `cargo test -p s2script-core` — never pass `--test-threads`.
- **Sniper build + live gate are the MAIN LOOP's job — out of plan scope.** This plan's exit criteria are host builds + the gate suite; the live-gate checklist is spec §11.
- **Naming:** PascalCase types (`UserMessageView`), camelCase members (`onPre`, `readInt`). Plugins are pure ESM.

## File Structure

- `docs/superpowers/specs/2026-07-16-usermessage-hook-design.md` + this file. *(Task 1.)*
- `shim/include/s2script_core.h` — +8 op typedefs + 8 tail members. *(Task 2.)*
- `core/src/v8host.rs` — 8 aliases + 8 fields + test op-structs; `USERMSG_MUX` + `USERMSG_IDS` map + `dispatch_usermsg`; 8 natives + registration; prelude `UserMessages` + view factory in `__s2pkg_usermessages`; tests. *(Task 2.)*
- `core/src/ffi.rs` — `s2script_core_dispatch_usermsg` export. *(Task 2.)*
- `shim/src/s2script_mm.h` — `Hook_PostEvent` member decl. *(Task 2.)*
- `shim/src/s2script_mm.cpp` — SH_DECL, statics + bitmap, 8 ops, validation, hook body, unload removal, ops wiring. *(Task 2.)*
- `packages/sdk/usermessages.d.ts` — `UserMessageView` + `UserMessages`. `packages/sdk/globals.d.ts` unchanged (hook modules never touch it — usercmd/damage precedent). *(Task 3.)*
- `.changeset/usermsg-hook.md` — minor `@s2script/sdk`. *(Task 3.)*
- `examples/usermsg-demo/{package.json,tsconfig.json,src/plugin.ts}` (new). *(Task 4.)*

---

## Task 1 (PR1): Design docs

**Files:** create `docs/superpowers/specs/2026-07-16-usermessage-hook-design.md`, `docs/superpowers/plans/2026-07-16-usermessage-hook.md` (already written by the design phase).

### Steps

- [ ] **Step 1:** `gt track -p main` if untracked, then:

```bash
git add docs/superpowers/specs/2026-07-16-usermessage-hook-design.md docs/superpowers/plans/2026-07-16-usermessage-hook.md
gt create -am "docs(usermsg): UserMessage-interception design spec + implementation plan

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 2 (PR2): Engine — ABI + core mux/dispatch/natives/prelude + shim hook + reflection reads

**Files:** modify `shim/include/s2script_core.h`, `core/src/v8host.rs`, `core/src/ffi.rs`, `shim/src/s2script_mm.h`, `shim/src/s2script_mm.cpp`.

**Interfaces:**
- C ops at the struct tail (shim fills all 8):
  - `int (*usermsg_hook_sub)(const char* name, char* canonicalOut, int canonicalLen)` — resolve + validate + lazy hook install + set bitmap bit; returns id `>= 0`, or `-1` (unknown name / `GetNetMessageInfo()` null / id out of `[0,2048)` / degraded — named reason logged shim-side).
  - `int (*usermsg_hook_unsub)(int id)` — clear the bitmap bit; 1 ok / 0 bad id.
  - `int (*usermsg_hook_read_int)(const char* path, long long* out)` — int32/uint32/fixed32/enum/bool via reflection `Get*`; dotted nested paths; 1 ok / 0 fail (no msg, no field, repeated, wrong type).
  - `int (*usermsg_hook_read_float)(const char* path, double* out)` — float/double; same contract.
  - `int (*usermsg_hook_read_string)(const char* path, char* buf, int buflen)` — string fields; returns byte length or `-1`.
  - `int (*usermsg_hook_has_field)(const char* path)` — 1 present / 0 absent-or-invalid / -1 no current message.
  - `int (*usermsg_hook_recipients)(unsigned long long* outMask)` — 1 ok (broadcast pre-expanded to all valid slots) / 0 no current message.
  - `int (*usermsg_hook_debug)(char* buf, int buflen)` — TextFormat dump; returns byte length (truncated to buflen) or `-1`.
- ffi export: `pub extern "C" fn s2script_core_dispatch_usermsg(name: *const c_char, id: c_int) -> c_int` — collapsed HookResult 0..3, fail-open 0 (`catch_unwind`), mirroring `s2script_core_dispatch_output` (`ffi.rs:185`).
- JS-facing (prelude, consumed by Task 3's `.d.ts`): `__s2pkg_usermessages.UserMessages.onPre(name, handler)` / `.off(name)`, view per spec §6.

### Steps

- [ ] **Step 1 (RED): core in-isolate tests first.** In the `#[cfg(test)]` module of `core/src/v8host.rs`, next to the UserMessage-builder degrade test (~:11456). Capture ops:

```rust
    thread_local! {
        static USERMSG_SUB_CAPTURE:   std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        static USERMSG_UNSUB_CAPTURE: std::cell::RefCell<Vec<i32>>    = std::cell::RefCell::new(Vec::new());
    }
    /// Mock resolver: canonicalizes "FireBullets" -> "TE_FireBullets_Canonical" id 452 (partial-match
    /// shape); "SubFail" -> -1 (degraded/unknown). Records every sub call.
    extern "C" fn mock_usermsg_sub(name: *const c_char, out: *mut c_char, len: c_int) -> c_int {
        let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().to_string();
        USERMSG_SUB_CAPTURE.with(|v| v.borrow_mut().push(n.clone()));
        if n == "SubFail" { return -1; }
        let canonical = if n.contains("FireBullets") { "TE_FireBullets_Canonical" } else { return -1 };
        let bytes = canonical.as_bytes();
        assert!((bytes.len() as c_int) < len);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, bytes.len());
                 *out.add(bytes.len()) = 0; }
        452
    }
    extern "C" fn mock_usermsg_unsub(id: c_int) -> c_int {
        USERMSG_UNSUB_CAPTURE.with(|v| v.borrow_mut().push(id)); 1
    }
    extern "C" fn mock_usermsg_read_int(path: *const c_char, out: *mut i64) -> c_int {
        let p = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().to_string();
        let v = match p.as_str() { "item_def_index" => 9, "player" => 6390016, "origin.x" => 0, _ => return 0 };
        unsafe { *out = v; } 1
    }
    extern "C" fn mock_usermsg_read_float(path: *const c_char, out: *mut f64) -> c_int {
        let p = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().to_string();
        if p == "origin.x" { unsafe { *out = 128.5; } 1 } else { 0 }
    }
    extern "C" fn mock_usermsg_recipients(out: *mut u64) -> c_int { unsafe { *out = 0b1010; } 1 }
```

The five tests (names are the contract — write them, watch them FAIL to compile/run, then implement):

```rust
    /// onPre resolves the name through usermsg_hook_sub (canonicalized), and a dispatched message
    /// delivers a view with the canonical name + id; a non-subscribed name does not cross-fire.
    #[test]
    fn usermsg_on_pre_subscribes_and_dispatch_delivers_view() { /*
        set_engine_ops with mock_usermsg_sub/read ops (..mock_event_ops());
        load_plugin_js "pum": UserMessages.onPre("FireBullets", function (m) {
            globalThis.__m_ran = (globalThis.__m_ran||0)+1; globalThis.__m_name = m.name;
            globalThis.__m_id = m.id; globalThis.__m_idx = m.readInt("item_def_index"); });
        assert USERMSG_SUB_CAPTURE contains "FireBullets";
        assert dispatch_usermsg("TE_FireBullets_Canonical", 452) == 0 (Continue);
        assert __m_ran==1, __m_name=="TE_FireBullets_Canonical", __m_id==452, __m_idx==9;
        dispatch_usermsg("SomethingElse", 7); assert __m_ran==1 still. */ }

    /// The HookResult collapse: Handled (2) from any handler suppresses (dispatch returns 2);
    /// Continue returns 0. Max-by-precedence across two handlers.
    #[test]
    fn usermsg_dispatch_collapses_hook_results() { /*
        two plugins onPre the same name, one returns HookResult.Handled, one Continue;
        assert dispatch_usermsg(...) == 2. Single Continue-only plugin -> 0. */ }

    /// Degrade: usermsg_hook_sub returning -1 makes onPre THROW at subscribe (plugin-load-loud);
    /// with NO engine ops at all it also throws (never a silent dead subscription).
    #[test]
    fn usermsg_on_pre_throws_on_unresolvable_or_degraded() { /*
        eval "try { UserMessages.onPre('SubFail', function(){}) ; 'no-throw' } catch(e) { 'threw' }"
        == "threw"; repeat with set_engine_ops(None). */ }

    /// View reads route through the read ops; a missing field reads null (not 0/undefined-crash);
    /// readFloat("origin.x") proves dotted paths cross the ABI verbatim.
    #[test]
    fn usermsg_view_reads_route_through_ops_and_null_on_miss() { /*
        in-handler: m.readFloat("origin.x")===128.5, m.readInt("nope")===null,
        m.recipients deep-equals [1,3] (mask 0b1010), m.readInt("player")===6390016. */ }

    /// Teardown is the ledger's job: unloading the only subscriber plugin calls usermsg_hook_unsub
    /// with the id from sub time; a second subscriber keeps the bit (no unsub until last-empty).
    #[test]
    fn usermsg_unload_unsubscribes_on_last_empty() { /*
        pluginA + pluginB both onPre("FireBullets"); unload_plugin(A) -> UNSUB_CAPTURE empty;
        unload_plugin(B) -> UNSUB_CAPTURE == [452]. */ }
```

Write the test bodies fully (mirror the voice-plan test style: `init(dummy_logger())`, `set_engine_ops(Some(S2EngineOps { usermsg_hook_sub: Some(mock_usermsg_sub), ..mock_event_ops() }))`, `load_plugin_js`, `eval_in_context_string`, `read_i32_global_in`, `shutdown()`).

- [ ] **Step 2: run — confirm RED.** `cargo test -p s2script-core usermsg_` → compile errors (missing fields/fns) count as the failing state.

- [ ] **Step 3: C ABI.** In `shim/include/s2script_core.h` after `s2_transmit_stats_fn` (`:259`):

```c
/* UserMessage-interception slice — APPENDED after transmit_stats; order is the ABI.
 * usermsg_hook_sub: resolve an unscoped message name (FindNetworkMessagePartial, the live-proven
 * SayText2 path), VALIDATE the m_MessageId extraction fail-closed (non-null NetMessageInfo, id in
 * [0,2048), requested name a substring of GetUnscopedName), lazily SH_ADD_HOOK PostEventAbstract on
 * the first-ever sub, set the id's bitmap bit, write the canonical unscoped name into canonicalOut.
 * Returns the id, or -1 with a named USERMSG reason logged. All read ops target the BLOCK-SCOPED
 * current intercepted message (null-guarded; valid only during a dispatch). */
typedef int (*s2_usermsg_hook_sub_fn)(const char* name, char* canonicalOut, int canonicalLen);
typedef int (*s2_usermsg_hook_unsub_fn)(int id);
typedef int (*s2_usermsg_hook_read_int_fn)(const char* path, long long* out);
typedef int (*s2_usermsg_hook_read_float_fn)(const char* path, double* out);
typedef int (*s2_usermsg_hook_read_string_fn)(const char* path, char* buf, int buflen);
typedef int (*s2_usermsg_hook_has_field_fn)(const char* path);
typedef int (*s2_usermsg_hook_recipients_fn)(unsigned long long* outMask);
typedef int (*s2_usermsg_hook_debug_fn)(char* buf, int buflen);
```

Struct members after `s2_transmit_stats_fn transmit_stats;` (`:386`), before `} S2EngineOps;`:

```c
    /* UserMessage-interception slice — APPENDED after transmit_stats; order is the ABI. */
    s2_usermsg_hook_sub_fn         usermsg_hook_sub;
    s2_usermsg_hook_unsub_fn       usermsg_hook_unsub;
    s2_usermsg_hook_read_int_fn    usermsg_hook_read_int;
    s2_usermsg_hook_read_float_fn  usermsg_hook_read_float;
    s2_usermsg_hook_read_string_fn usermsg_hook_read_string;
    s2_usermsg_hook_has_field_fn   usermsg_hook_has_field;
    s2_usermsg_hook_recipients_fn  usermsg_hook_recipients;
    s2_usermsg_hook_debug_fn       usermsg_hook_debug;
```

- [ ] **Step 4: Rust ABI mirror.** In `core/src/v8host.rs` next to `TransmitStatsFn` (grep `type TransmitStatsFn`):

```rust
// UserMessage-interception slice (APPENDED after transmit_stats; order is the ABI).
type UsermsgHookSubFn        = extern "C" fn(*const std::os::raw::c_char, *mut std::os::raw::c_char, c_int) -> c_int;
type UsermsgHookUnsubFn      = extern "C" fn(c_int) -> c_int;
type UsermsgHookReadIntFn    = extern "C" fn(*const std::os::raw::c_char, *mut i64) -> c_int;
type UsermsgHookReadFloatFn  = extern "C" fn(*const std::os::raw::c_char, *mut f64) -> c_int;
type UsermsgHookReadStringFn = extern "C" fn(*const std::os::raw::c_char, *mut std::os::raw::c_char, c_int) -> c_int;
type UsermsgHookHasFieldFn   = extern "C" fn(*const std::os::raw::c_char) -> c_int;
type UsermsgHookRecipientsFn = extern "C" fn(*mut u64) -> c_int;
type UsermsgHookDebugFn      = extern "C" fn(*mut std::os::raw::c_char, c_int) -> c_int;
```

Fields after `pub transmit_stats: Option<TransmitStatsFn>,` (`:372`); then `grep -n "transmit_stats" core/src/v8host.rs` and add `usermsg_*: None` (or the mocks in tests) to EVERY op-struct literal (~:11233, ~:11716, ~:11874) — struct-update `..` literals need nothing, full literals need all 8.

- [ ] **Step 5: `USERMSG_MUX` + name→id map + `dispatch_usermsg`.** Next to `OUTPUT_MUX` (`:624`):

```rust
    /// UserMessage-interception slice: pre-hook handlers keyed by CANONICAL unscoped message name.
    /// Collapsing dispatch (max-by-precedence, multiplexer.rs rule) — >= Handled(2) tells the shim to
    /// MRES_SUPERCEDE the send. Lazy shim hook install on first-ever sub (usercmd precedent); on a
    /// name's last-empty (off or unload remove_by_owner) the shim's bitmap bit is cleared via
    /// usermsg_hook_unsub. `remove_by_owner` on unload; reset on shutdown.
    static USERMSG_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>> = ...;
    /// canonical name -> engine id (from usermsg_hook_sub) so teardown can clear bitmap bits.
    static USERMSG_IDS: std::cell::RefCell<std::collections::HashMap<String, i32>> = ...;
```

`dispatch_usermsg(name: &str, id: i32) -> i32` is a copy of `dispatch_output` (~:6608): snapshot subscribers for `name` with the mux borrow released → `HOST.try_borrow_mut()` graceful-skip (`Ok` else return 0 Continue) → `run_chain` calling each handler with args `(nameString, idNumber)` → return `outcome.result as i32`. In `unload_plugin`, next to the other mux sweeps (grep `remove_by_owner`): `USERMSG_MUX remove_by_owner` → for each name that became empty, look up `USERMSG_IDS` and call `ops.usermsg_hook_unsub(id)`.

In `core/src/ffi.rs` after `s2script_core_dispatch_output` (`:185`):

```rust
/// UserMessage-interception: shim -> core on a bitmap-hit PostEventAbstract. Returns the collapsed
/// HookResult (0..3); >= 2 = the shim supersedes the send. Fail-open Continue on panic/nul.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_usermsg(name: *const c_char, id: c_int) -> c_int {
    catch_unwind(|| {
        let Ok(name_str) = (unsafe { CStr::from_ptr(name) }).to_str() else { return 0 };
        v8host::dispatch_usermsg(name_str, id)
    }).unwrap_or(0)
}
```

- [ ] **Step 6: natives.** Next to the UserMessage-builder flush native (grep `__s2_user_message`), all `catch_unwind`-wrapped:

  - `__s2_usermsg_on(name, wrappedHandler) -> canonical|null`: call `ops.usermsg_hook_sub` (256-byte canonical buffer); on `-1`/no-op return null; else record `USERMSG_IDS[canonical] = id`, `USERMSG_MUX.subscribe(canonical, owner, generation, handler)` (owner = current plugin, the `s2_output_on` pattern at ~:5966), return the canonical string.
  - `__s2_usermsg_off(name)`: resolve canonical via `ops.usermsg_hook_sub` again (idempotent bit-set), `remove_by_owner_on(canonical, owner)`; if the name is now empty across all owners → `ops.usermsg_hook_unsub(USERMSG_IDS[canonical])`.
  - `__s2_usermsg_read_int(path) -> number|null`, `__s2_usermsg_read_float(path) -> number|null` (out-param ops, rc 0 → null), `__s2_usermsg_read_string(path) -> string|null` (4096-byte buffer), `__s2_usermsg_has_field(path) -> i32`, `__s2_usermsg_recipients() -> number[]` (decode the u64 mask bits to a JS array of slots; rc 0 → empty array), `__s2_usermsg_debug() -> string` (8192-byte buffer, "" on rc -1).

  Register all 8 in `install_natives` next to `__s2_transmit_stats` (`:7127`).

- [ ] **Step 7: prelude.** In the `__s2pkg_usermessages` block (before `:1309`):

```js
  // --- UserMessage interception (usermsg-hook slice). The view is BLOCK-SCOPED: the shim's current-
  // message statics are nulled when the synchronous dispatch returns, so reads after an await (or on
  // a stashed view) return null/[]/"" — never a dangling pointer. Suppression is the HookResult
  // return (>= Handled supersedes the send for EVERY recipient). Plugin-originated sends from inside
  // any JS dispatch do NOT re-trigger these hooks (recursion guard; documented v1 limitation).
  function __s2_umView(name, id) {
    return {
      name: name, id: id,
      get recipients() { return __s2_usermsg_recipients(); },
      get debugString() { return __s2_usermsg_debug(); },
      hasField:   function (p) { return __s2_usermsg_has_field(String(p)) === 1; },
      readInt:    function (p) { return __s2_usermsg_read_int(String(p)); },
      readFloat:  function (p) { return __s2_usermsg_read_float(String(p)); },
      readBool:   function (p) { var v = __s2_usermsg_read_int(String(p)); return v === null ? null : v !== 0; },
      readString: function (p) { return __s2_usermsg_read_string(String(p)); }
    };
  }
  var UserMessages = {
    onPre: function (name, handler) {
      var canonical = __s2_usermsg_on(String(name), function (n, id) { return handler(__s2_umView(n, id)); });
      if (!canonical)
        throw new Error("UserMessages.onPre: cannot resolve message '" + name +
                        "' (unknown name, or the intercept descriptor is degraded — see server log)");
    },
    off: function (name) { __s2_usermsg_off(String(name)); }
  };
```

and extend the export: `globalThis.__s2pkg_usermessages = { UserMessage: UserMessage, UserMessages: UserMessages };`

- [ ] **Step 8: run — core GREEN.** `cargo test -p s2script-core usermsg_` → 5 (+ the pre-existing builder test) PASS; `cargo test -p s2script-core` → full suite green.

- [ ] **Step 9: shim — SH_DECL + member decl.** `git submodule update --init --recursive` first. In `shim/src/s2script_mm.cpp` next to the other SH_DECLs (grep `SH_DECL_HOOK.*FireEvent`):

```cpp
// UserMessage-interception slice. The 8-arg PostEventAbstract overload — the EXACT method our send
// path calls live (s2_client_print :922 / s2_user_message_send :1027), so the vendored-header vtable
// slot is transitively proven. Param 7 is `unsigned long` exactly (ABI). SourceHook disambiguates
// from the 6-arg IRecipientFilter overload by the parameter type list — no numeric index anywhere.
SH_DECL_HOOK8_void(IGameEventSystem, PostEventAbstract, SH_NOATTRIB, 0,
    CSplitScreenSlot, bool, int, const uint64*,
    INetworkMessageInternal*, const CNetMessage*, unsigned long, NetChannelBufType_t);
```

In `shim/src/s2script_mm.h` next to the lifecycle hook decls:

```cpp
    // UserMessage-interception: bitmap-gated pre-hook on every outbound PostEventAbstract.
    void Hook_PostEvent(CSplitScreenSlot nSlot, bool bLocalOnly, int nClientCount, const uint64* clients,
                        INetworkMessageInternal* pEvent, const CNetMessage* pData,
                        unsigned long nSize, NetChannelBufType_t bufType);
```

- [ ] **Step 10: shim — statics, validation, ops.** After the send-builder statics/ops block (`s2_user_message_send`, ~:1027+):

```cpp
// ---------------------------------------------------------------------------
// UserMessage-interception slice. Doctrine: the ONE borrowed layout fact is
// NetMessageInfo_t::m_MessageId (inetworkserializer.h:53 — never exercised by the send path);
// validated fail-closed at subscribe (round-trip below) and on an observe-only first fire.
// Hot path: bitmap test on the id, MRES_IGNORED on miss before ANY reflection.
// Block-scoped view statics are SEPARATE from the send builder's s_umInfo/s_umData/s_umMsg.
// ---------------------------------------------------------------------------
static constexpr int kUserMsgMaxId = 2048;
static uint64_t s_userMsgSubBits[kUserMsgMaxId / 64] = {0};
static bool     s_userMsgHookInstalled = false;   // lazy SH_ADD_HOOK on first-ever sub
static bool     s_userMsgFirstFireDone = false;   // observe-only first-fire validation ran
static bool     s_userMsgDegraded = false;        // NAMED degrade: hook inert, subs refused
static bool     s_inUserMsgDispatch = false;      // recursion guard (mid-hook sends re-enter)
static google::protobuf::Message* s_hookMsg = nullptr;      // current intercepted message
static const uint64_t*            s_hookClients = nullptr;  // its recipient mask (may be null=broadcast)
static int                        s_hookClientCount = 0;

static inline bool s2_usermsg_bit(int id) {
    return id >= 0 && id < kUserMsgMaxId && (s_userMsgSubBits[id >> 6] & (1ull << (id & 63)));
}

// Dotted-path walk: returns the leaf's parent message + writes the leaf field name. Every hop is
// guarded (CPPTYPE_MESSAGE, !is_repeated) — a scalar Get* on a repeated field is a protobuf FATAL
// that aborts the process (the shipping s2_user_message_set_* guards, mirrored).
static const google::protobuf::Message* s2_usermsg_walk(const google::protobuf::Message* m,
                                                        const char* path, std::string& leaf);
```

`usermsg_hook_sub` (the subscribe-time validation — spec §2.1):

```cpp
static int s2_usermsg_hook_sub(const char* name, char* canonicalOut, int canonicalLen) {
    if (s_userMsgDegraded || !name || !s_pNetworkMessages || !s_pGameEventSystem) return -1;
    INetworkMessageInternal* info = s_pNetworkMessages->FindNetworkMessagePartial(name);
    if (!info) { META_CONPRINTF("[s2script] USERMSG sub FAILED: no message matches '%s'\n", name); return -1; }
    const NetMessageInfo_t* mi = info->GetNetMessageInfo();
    if (!mi) { META_CONPRINTF("[s2script] USERMSG descriptor 'message-id-extract' FAILED: "
                              "GetNetMessageInfo null for '%s'\n", name); return -1; }
    int id = (int)mi->m_MessageId;
    const char* canonical = info->GetUnscopedName();
    if (id < 0 || id >= kUserMsgMaxId || !canonical || !*canonical || !strstr(canonical, name)) {
        META_CONPRINTF("[s2script] USERMSG descriptor 'message-id-extract' FAILED: '%s' -> id=%d "
                       "canonical='%s' (out of range or name mismatch — header layout drift?)\n",
                       name, id, canonical ? canonical : "(null)");
        return -1;
    }
    snprintf(canonicalOut, (size_t)canonicalLen, "%s", canonical);
    if (!s_userMsgHookInstalled) {   // lazy install, idempotent (m_eventHookInstalled pattern)
        SH_ADD_HOOK(IGameEventSystem, PostEventAbstract, s_pGameEventSystem,
                    SH_MEMBER(this_plugin(), &S2ScriptPlugin::Hook_PostEvent), false);   // PRE
        s_userMsgHookInstalled = true;
        META_CONPRINTF("[s2script] usermsg: PostEventAbstract hook installed (lazy, first subscribe)\n");
    }
    s_userMsgSubBits[id >> 6] |= (1ull << (id & 63));
    return id;
}
static int s2_usermsg_hook_unsub(int id) {
    if (id < 0 || id >= kUserMsgMaxId) return 0;
    s_userMsgSubBits[id >> 6] &= ~(1ull << (id & 63));
    return 1;
}
```

(Use the file's existing idiom for reaching the plugin instance from a static op — grep how `Shim_UsercmdHookInstall` reaches `SH_ADD_HOOK`/the plugin `this`; mirror it instead of `this_plugin()` if it differs.)

Read ops: `s2_usermsg_hook_read_int/float/string/has_field` all begin `if (!s_hookMsg || !path) return 0;` (has_field returns -1), walk the dotted path, then the `cpp_type()` switch of `Get*` mirrors of `s2_user_message_set_*` (~:994-1040): `GetInt32/GetUInt32/GetInt64/GetUInt64/GetEnumValue/GetBool` → `read_int`; `GetFloat/GetDouble` → `read_float`; `GetString` → `read_string`. `s2_usermsg_hook_recipients`: no msg → 0; `s_hookClients` non-null → `*outMask = *s_hookClients`; null (broadcast, `nClientCount==-1`) → build the mask from the existing client-validity helper (grep `s2_client_valid`). `s2_usermsg_hook_debug`: `s_hookMsg->DebugString()` into the buffer.

- [ ] **Step 11: shim — the hook body.**

```cpp
// UserMessage-interception choke point: every outbound event/message posts through here. Order:
// recursion guard -> degraded guard -> observe-only FIRST-FIRE validation (never suppresses) ->
// bitmap gate on m_MessageId (one virtual + one bit test; MRES_IGNORED on miss BEFORE any
// reflection/strcmp/FFI) -> name-keyed core dispatch with block-scoped statics -> collapsed
// HookResult >= Handled(2) => MRES_SUPERCEDE (dropped for every recipient + any local listener —
// the live-gate watches for server-side fallout; fallback = recall-with-modified-mask).
void S2ScriptPlugin::Hook_PostEvent(CSplitScreenSlot nSlot, bool bLocalOnly, int nClientCount,
                                    const uint64* clients, INetworkMessageInternal* pEvent,
                                    const CNetMessage* pData, unsigned long nSize,
                                    NetChannelBufType_t bufType) {
    if (s_inUserMsgDispatch || s_userMsgDegraded) RETURN_META(MRES_IGNORED);
    if (!s_userMsgFirstFireDone) {
        s_userMsgFirstFireDone = true;
        const NetMessageInfo_t* mi = pEvent ? pEvent->GetNetMessageInfo() : nullptr;
        const char* nm = pEvent ? pEvent->GetUnscopedName() : nullptr;
        INetworkMessageInternal* rt = (nm && *nm && s_pNetworkMessages)
                                        ? s_pNetworkMessages->FindNetworkMessagePartial(nm) : nullptr;
        google::protobuf::Message* pb = pData
            ? reinterpret_cast<google::protobuf::Message*>(const_cast<CNetMessage*>(pData)->AsProto()) : nullptr;
        if (!mi || (int)mi->m_MessageId < 0 || (int)mi->m_MessageId >= 32768 || !nm || !*nm
            || !rt || !rt->GetNetMessageInfo() || rt->GetNetMessageInfo()->m_MessageId != mi->m_MessageId
            || !pb || !pb->GetDescriptor() || !pb->GetReflection()) {
            s_userMsgDegraded = true;
            META_CONPRINTF("[s2script] USERMSG VALIDATION FAILED: first-fire id/name/reflection "
                           "round-trip mismatch (name='%s') — NetMessageInfo_t layout drift? "
                           "intercept DISABLED (send path unaffected)\n", nm ? nm : "(null)");
            RETURN_META(MRES_IGNORED);
        }
        META_CONPRINTF("[s2script] USERMSG intercept validated (first fire: id=%d name=%s)\n",
                       (int)mi->m_MessageId, nm);
    }
    const NetMessageInfo_t* mi = pEvent ? pEvent->GetNetMessageInfo() : nullptr;
    if (!mi || !s2_usermsg_bit((int)mi->m_MessageId)) RETURN_META(MRES_IGNORED);   // the cheap gate
    google::protobuf::Message* pb = pData
        ? reinterpret_cast<google::protobuf::Message*>(const_cast<CNetMessage*>(pData)->AsProto()) : nullptr;
    if (!pb) RETURN_META(MRES_IGNORED);
    s_hookMsg = pb; s_hookClients = clients; s_hookClientCount = nClientCount;
    s_inUserMsgDispatch = true;
    int result = s2script_core_dispatch_usermsg(pEvent->GetUnscopedName(), (int)mi->m_MessageId);
    s_inUserMsgDispatch = false;
    s_hookMsg = nullptr; s_hookClients = nullptr; s_hookClientCount = 0;   // block-scope ends here
    if (result >= 2 /* HookResult.Handled */) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
```

- [ ] **Step 12: shim — unload removal + ops wiring.** In `Unload()` next to the FireEvent `SH_REMOVE_HOOK` (grep `m_eventHookInstalled`):

```cpp
    // UserMessage-interception: remove the lazy PostEventAbstract hook (ledger/teardown authority).
    if (s_userMsgHookInstalled && s_pGameEventSystem) {
        SH_REMOVE_HOOK(IGameEventSystem, PostEventAbstract, s_pGameEventSystem,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_PostEvent), false);
        s_userMsgHookInstalled = false;
    }
```

Ops wiring after `ops.transmit_stats = &s2_transmit_stats;` (`:3589`):

```cpp
    // UserMessage-interception slice — APPENDED after transmit_stats; order MUST match S2EngineOps.
    ops.usermsg_hook_sub         = &s2_usermsg_hook_sub;
    ops.usermsg_hook_unsub       = &s2_usermsg_hook_unsub;
    ops.usermsg_hook_read_int    = &s2_usermsg_hook_read_int;
    ops.usermsg_hook_read_float  = &s2_usermsg_hook_read_float;
    ops.usermsg_hook_read_string = &s2_usermsg_hook_read_string;
    ops.usermsg_hook_has_field   = &s2_usermsg_hook_has_field;
    ops.usermsg_hook_recipients  = &s2_usermsg_hook_recipients;
    ops.usermsg_hook_debug       = &s2_usermsg_hook_debug;
```

- [ ] **Step 13: build + gates (PR2 exit).**

```bash
make core                                    # expected: release build OK
make shim                                    # expected: shim compiles + links (host build — dev/CI proof only)
cargo test -p s2script-core                  # expected: full suite green (prev count + 5)
make check-boundary                          # expected: PASS (no games/* import in core)
bash scripts/test-boundary-nameleak.sh       # expected: PASS (no CS2 name in core/shim)
```

- [ ] **Step 14: three-way ABI tail re-check + commit.** Verify field order: `s2script_core.h` struct == `v8host.rs` struct == the `ops.*` assignment block, and that `transmit_stats` is STILL the preceding tail on current `origin/main` (re-anchor if #67/#71 merged). Then:

```bash
git add shim/include/s2script_core.h core/src/v8host.rs core/src/ffi.rs shim/src/s2script_mm.h shim/src/s2script_mm.cpp
gt create -am "feat(usermsg): PostEventAbstract intercept — mux/dispatch, reflection reads, fail-closed validation

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 3 (PR3): SDK surface — `.d.ts` + changeset

**Files:** modify `packages/sdk/usermessages.d.ts`; create `.changeset/usermsg-hook.md`. (`packages/sdk/package.json` already exports `./usermessages` at `:112-114` — no exports change; `globals.d.ts` untouched, matching the usercmd/damage hook-module precedent.)

**Interfaces:** exactly the spec §6 surface (consumed by Task 4's demo and every future TTT consumer).

### Steps

- [ ] **Step 1 (RED): typecheck probe.** Append a throwaway probe to any existing example (or `npx tsc --noEmit` a scratch file in the scratchpad importing `UserMessages` from `@s2script/sdk/usermessages`) → FAILS: `UserMessages` not exported.

- [ ] **Step 2: implement.** Append to `packages/sdk/usermessages.d.ts` (below the existing `UserMessage` class) the spec §6 block verbatim: the `import type { HookResultValue } from "./events";` line, `export interface UserMessageView { ... }`, `export declare const UserMessages: { onPre(...); off(...); }` — including the block-scoped/throws/suppression doc comments.

- [ ] **Step 3: run — GREEN + gates.**

```bash
( cd packages/cli && node build.mjs )
bash scripts/check-plugins-typecheck.sh      # expected: PASS (every plugin + example vs the new .d.ts)
make check-boundary && bash scripts/test-boundary-nameleak.sh   # expected: PASS
```

- [ ] **Step 4: changeset + commit.** `.changeset/usermsg-hook.md`:

```md
---
"@s2script/sdk": minor
---

UserMessage interception: `UserMessages.onPre(name, handler)` / `UserMessages.off(name)` with a
block-scoped `UserMessageView` (typed scalar reads with dotted nested paths, read-only recipients,
`debugString` fallback). Returning >= `HookResult.Handled` suppresses the send for every recipient.
Fail-closed: an unresolvable name (or a degraded intercept descriptor) throws at subscribe time.
```

```bash
git add packages/sdk/usermessages.d.ts .changeset/usermsg-hook.md
gt create -am "feat(usermsg): @s2script/sdk usermessages hook API (UserMessages.onPre + UserMessageView)

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 4 (PR4): Consumers — demo plugin + full host gate run

**Files:** create `examples/usermsg-demo/{package.json,tsconfig.json,src/plugin.ts}`.

**Interfaces:**
- Consumes: `UserMessages`/`UserMessageView` (`@s2script/sdk/usermessages`), `HookResult` (`@s2script/sdk/events`), `Commands.register` (`@s2script/sdk/commands`). CS2 knowledge (message names, field names, handle decode) lives HERE — the correct side of the boundary.

### Steps

- [ ] **Step 1: scaffold.** `examples/usermsg-demo/package.json`:

```json
{
  "name": "@demo/usermsg-demo",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x" }
}
```

`cp examples/entity-listeners-demo/tsconfig.json examples/usermsg-demo/tsconfig.json`.

- [ ] **Step 2: `examples/usermsg-demo/src/plugin.ts`** — the four TTT consumer shapes + the D-09 probe:

```ts
// Live-gate demo for the usermsg-hook slice — the four TTT consumer shapes:
//   BombPlantSuppressor: onPre RadioText -> Handled (blanket radio-text block, toggleable)
//   SilentAWP/PoisonShots/SuppressedRound: onPre TEFireBullets -> typed reads + conditional suppress
// D-09 probe: readInt("player") is the shooter as a packed fixed32 entity handle — logged raw AND
// bit-split both ways (14/15-bit index) so the live gate settles the packing + pawn-vs-controller
// questions before any TTT consumer ships on it. All message/field names are CS2 knowledge and live
// HERE, never in core/shim (vendored proto: third_party/hl2sdk/game/shared/cs/cs_gameevents.proto).
import { UserMessages } from "@s2script/sdk/usermessages";
import { HookResult, type HookResultValue } from "@s2script/sdk/events";
import { Commands } from "@s2script/sdk/commands";

let blockRadio = false;   // BombPlantSuppressor shape (TTT blanket-blocks ALL radio text)
let blockShots = false;   // SuppressedRound shape

UserMessages.onPre("CCSUsrMsg_RadioText", (m): HookResultValue | void => {
  console.log("[usermsg-demo] RadioText id=" + m.id + " msg_name=" + m.readString("msg_name") +
              " client=" + m.readInt("client") + " recipients=[" + m.recipients.join(",") + "]");
  if (blockRadio) return HookResult.Handled;      // TTT: Recipients.Clear()+Handled -> just Handled
});

UserMessages.onPre("CMsgTEFireBullets", (m): HookResultValue | void => {
  const item = m.readInt("item_def_index");       // TTT's weapon filter (the only typed read it used)
  const player = m.readInt("player");             // D-09 fix: the shooter handle, top-level fixed32
  const ox = m.readFloat("origin.x");             // dotted nested read — the capability CSSharp lacks
  console.log("[usermsg-demo] FireBullets item_def_index=" + item + " player=" + player +
              " origin.x=" + ox +
              (player !== null && player !== 16777215
                 ? " idx14=" + (player & 0x3fff) + "/ser" + (player >>> 14) +
                   " idx15=" + (player & 0x7fff) + "/ser" + (player >>> 15)
                 : " (no shooter)"));
  if (blockShots) return HookResult.Handled;      // SilentAWP/SuppressedRound suppress path
});

Commands.register("sm_umtest", (ctx) => {
  const what = ctx.arg(0), on = ctx.arg(1) !== "0";
  if (what === "radio") blockRadio = on;
  else if (what === "shots") blockShots = on;
  else { ctx.reply("[usermsg-demo] usage: sm_umtest <radio|shots> <0|1>"); return; }
  ctx.reply("[usermsg-demo] block " + what + " = " + on);
});

export function onLoad(): void {
  console.log("[usermsg-demo] onLoad — RadioText + TEFireBullets hooks armed; sm_umtest registered");
}
export function onUnload(): void {}   // no manual off — teardown is the ledger's job (proves it)
```

(If subscribe-time resolution can run at module scope vs `onLoad` differs in this codebase, follow the sibling demos — `grep -rn "onPre" examples/` — and move the two `onPre` calls into `onLoad` if that is the house pattern.)

- [ ] **Step 3 (RED→GREEN): build the demo.** `node packages/cli/dist/cli.js build examples/usermsg-demo` — first run RED if any `.d.ts` mismatch exists; fix until it emits `dist/*.s2sp` with the strict typecheck PASS.

- [ ] **Step 4: full host gate run (PR4 exit — the whole-stack proof).**

```bash
make core && make shim
cargo test -p s2script-core
make check-boundary
bash scripts/test-boundary-nameleak.sh
bash scripts/check-plugins-typecheck.sh
# all expected: PASS. Sniper build + Docker live gate (spec §11 checklist: validation banner,
# RadioText suppression + server-side-fallout check, FireBullets typed reads + player-handle
# decode, no protobuf FATAL, RestartCount=0) are the MAIN LOOP's job — out of plan scope.
```

- [ ] **Step 5: commit + submit the stack.**

```bash
git add examples/usermsg-demo
gt create -am "feat(usermsg): demo plugin — TTT consumer shapes + D-09 shooter-handle probe

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
gt submit --stack --no-interactive
```

PR bodies (Write tool + `gh pr edit N --body-file`, never a heredoc) must include **Stack Context** (UserMessage interception for the TTT port) and **Why**, plus: the pure-reuse verdict (zero new sig-scans; the one validated header offset), the fail-closed validation design, the MRES_SUPERCEDE local-listener trade-off flagged for the live gate, the ABI-tail collision note vs #67/#71, and `https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3`.

---

## Self-Review

**1. Spec coverage.** §2 mechanism (SH_DECL_HOOK8 on the live-proven overload, lazy install, param-7 `unsigned long`) → Task 2 Steps 9-11. §2 doctrine validation (subscribe round-trip + observe-only first fire, named fail-closed degrade) → Steps 10-11; refusal surfaces as the `onPre` throw tested in Step 1 test 3. §3 suppress/collapse (max-by-precedence, >=Handled → SUPERCEDE, read-only recipients incl. broadcast expansion) → Steps 1 (tests 2, 4), 5, 10, 11. §4 block-scope + re-entrancy (separate statics, `s_inUserMsgDispatch`, `try_borrow_mut`) → Steps 5, 10, 11 + Global Constraints. §5 cheap gate (bitmap, MRES_IGNORED-on-miss before reflection) → Steps 10-11 + Global Constraints. §6 API → Task 3 (verbatim) + prelude Step 7. §7 architecture/ABI tail + #67/#71 collision → Steps 3-4, 12, 14 + Global Constraints. §8 boundary → Global Constraints + gate runs in every task. §9 deviations (D-09 typed reads incl. dotted `origin.x`, Handled-not-Clear, name-keyed, getters-only, pre-only) → Task 4 demo + spec cross-reference. §10 deferred → not built anywhere (verified: no setter, no removeRecipient, no post-mode, no 64-bit read in any step). §11 live gate → explicitly out of scope, checklist referenced in Task 4 Step 4.

**2. Placeholder scan.** No TBD/`<fill in>`. Two intentionally house-idiom-dependent points carry explicit grep instructions instead of guesses: the static-op→plugin-instance idiom for the lazy `SH_ADD_HOOK` (Task 2 Step 10, mirror `Shim_UsercmdHookInstall`) and module-scope-vs-`onLoad` subscription in the demo (Task 4 Step 2). Test bodies in Step 1 are contracts with exact assertions; tests 1, 3, and the capture ops are fully coded, 2/4/5 specify exact inputs/outputs.

**3. Type consistency.** C `long long*`/`double*` out-params ↔ Rust `*mut i64`/`*mut f64`; `const char*` ↔ `*const c_char`; mask `unsigned long long*` ↔ `*mut u64`. `usermsg_hook_sub` returns the id consumed by `USERMSG_IDS` and `usermsg_hook_unsub(int)`. Dispatch `(name: &str, id: i32) -> i32` ↔ ffi `(*const c_char, c_int) -> c_int` ↔ shim call with `GetUnscopedName()` + `(int)mi->m_MessageId`; suppress threshold `>= 2` matches `HookResult.Handled: 2` (`events.d.ts:25`). Prelude view methods ↔ natives ↔ `.d.ts` (`number|null` via rc-0→null everywhere; `recipients: number[]`; `readBool` null-propagating). Anchors verified against the worktree: `transmit_stats` at `s2script_core.h:259/:386`, `v8host.rs:372` + test structs `:11233/:11716/:11874`, wiring `s2script_mm.cpp:3589`; send statics `s_umInfo/s_umData/s_umMsg` `:977-979`; `OUTPUT_MUX` `:624`; `dispatch_output` export `ffi.rs:185`; `__s2pkg_usermessages` `v8host.rs:1309`; sdk exports `package.json:112-114`.

---

## Post-review revisions

The engine was implemented per Task 2, then a 4-lens adversarial review (doctrine, hot-path/memory, ABI/degrade, reuse/consumers) + an independent ABI audit ran; the ABI came back byte-consistent, hot-path and reuse/consumers clean, and four correctness items were fixed before the stack was cut. These are what actually shipped — the illustrative code in Task 2 Steps 10–11 predates them:

1. **First fire is truly observe-only, and gated on the first *subscribed* message.** The Step-11 code validated then fell through — it could `MRES_SUPERCEDE` the first message, contradicting the spec's "never suppresses/dispatches" guarantee. Fixed: the first-fire block now runs *after* the bitmap gate (so it only ever sees a message a plugin subscribed to, never the arbitrary first engine post) and unconditionally `RETURN_META(MRES_IGNORED)` after logging the banner. (spec §2.2, §11.1)
2. **Removed the self-comparing round-trip and the global-degrade latch.** `rt = FindNetworkMessagePartial(GetUnscopedName())` returns the same engine singleton as `pEvent`, so `rt->…m_MessageId != mi->m_MessageId` compared the field to itself — vacuous when it matched, and a **false global disable** whenever a partial-name match resolved `rt != pEvent`. `s_userMsgDegraded` is gone; a reflection failure on the first subscribed fire skips *that one fire* (per-descriptor), never the whole feature. `m_MessageId` is documented honestly as a range-checked offset feeding only the self-consistent bitmap pre-filter, with `GetUnscopedName()` as the authoritative dispatch key. (spec §2.2)
3. **`readInt` refuses 64-bit fields (returns `null`) instead of truncating.** The shim `read_int` handled `CPPTYPE_INT64/UINT64`, but the native marshals through `f64` — a value > 2^53 corrupts silently, violating the decimal-string 64-bit doctrine. The `INT64`/`UINT64` cases were removed (→ `default: 0` → `null`); 64-bit reads stay deferred (spec §10). No TTT consumer reads 64-bit.
4. **`UserMessages.off(name)` no longer leaks a permanent hook.** `off` re-resolved the name through the *installing* `usermsg_hook_sub` op, so `off()` on a never-subscribed name armed the hook + set a bitmap bit forever. Added a subscribe-time `USERMSG_RESOLVE` (raw-name → canonical) cache; `off` resolves from it and no-ops on a miss — no `sub` call. Plus: the `Unload` hook-removal now resets `s_userMsgFirstFireDone` + clears the bitmap so a re-arm re-validates.

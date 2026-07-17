# Crash Reporter — Capture Client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build sub-project 1 of the crash reporter (spec `docs/superpowers/specs/2026-07-17-crash-reporter-design.md`): the in-server capture client — breadcrumb tracker, the three capture paths (native fault via Breakpad, Rust panic, fatal JS error), the frozen incident-envelope wire contract, and the spool + upload-on-next-boot transport.

**Architecture:** A fixed-size `#[repr(C)]` breadcrumb POD in core static memory is stamped near-free on every dispatch and read by the crash paths with plain memory loads. The Breakpad `ExceptionHandler` lives shim-side (armed in `Load()` after `s2script_core_init`) and, on a hard fault, writes a minidump plus a `.s2meta` sidecar (the raw POD bytes, one `write()`) to a crash-spool dir; the Rust panic hook and the V8 error callbacks render JSON envelopes directly (normal context) into the same spool. A boot-time + periodic sweep renders/uploads spooled incidents over the existing tokio+reqwest engine.

**Tech Stack:** Rust (core: `serde`/`serde_json`, existing `tokio`+`reqwest`, new `uuid`; `reqwest` gains the `multipart` feature), C++17 (shim: vendored Google Breakpad Linux client + linux-syscall-support), the established `S2EngineOps` append-only C ABI, TypeScript example plugin for the harness.

## Global Constraints

- **Opt-in:** `crashreporter.json` key `enabled` defaults to **false**. `enabled=false` disables the uploader/sweep entirely; local spool files are still written (decision D-3 below).
- **Engine-generic core:** core must **not** import `games/*` (gate: `make check-boundary`). CS2's build number is pushed **into** core by the `@s2script/cs2` prelude via an engine-generic native; no CS2 identifier appears in `core/src` (the name-leak gate also scans for this).
- **Sniper build:** the only deployable binaries build inside `rust:bullseye` (Steam Runtime 3, Debian bullseye, **glibc 2.31**) via `scripts/build-sniper.sh`. Breakpad must compile and link there with `-Wl,--gc-sections` and `_GLIBCXX_USE_CXX11_ABI=0`.
- **Envelope contract is frozen at `schema_version: 1`** (bump to evolve). The §6.5 schema, reproduced verbatim:

```jsonc
{
  "schema_version": 1,
  "incident_id": "<uuid, generated in normal context by the uploader>",
  "kind": "native | js | panic",
  "occurred_at": "<ISO-8601 crash time — stamped directly for js/panic (normal context); for native, reconstructed by the uploader from the spool file's mtime, since native uploads on next boot>",
  "s2script": { "version": "...", "api_version": "..." },
  "gamedata": { "fingerprint": "...", "generated_at": "...", "hl2sdk": "...",
                "schema_build": "...", "stale": false },
  "game":     { "name": "cs2", "build_number": 0, "map": "...", "players": 0, "uptime": 0 },
  "host":     { "server_id": "<stable, hashed, non-PII>", "os": "..." },
  "breadcrumb": { "plugin": "...", "dispatch": "...", "engine_op": "...",
                  "js_location": "file:line", "ring": [ { "tick": 0, "plugin": "...", "dispatch": "..." } ] },
  "plugins":  [ { "id": "...", "version": "..." } ],
  "detail": {
    // native: { "minidump_ref": "<spool filename>", "faulting_module": "<optional, best-effort>" }
    // js:     { "stack": "...", "message": "...", "file": "...", "line": 0 }
    // panic:  { "message": "...", "backtrace": "..." }
  }
}
```

- **Upload-on-next-boot:** the signal handler only **writes files**; nothing network-touching or allocating runs in signal context. JS/panic kinds also spool first and are picked up by the periodic sweep (process still alive).
- **The golden rule:** *the handler must never cause or worsen a crash — async-signal-safe + bounded + chain to the previous handler; fail-off never fail-loud.* Everything reachable from the Breakpad callback is async-signal-safe (`open`/`write`/`close`/`memcpy` on fixed buffers); the callback returns `false` so previously installed handlers still run and the process dies as it otherwise would. Any init failure (Breakpad, spool dir, config parse) logs one WARN and leaves the runtime running normally with crash reporting off.
- **Tests:** `cargo test -p s2script-core` is forced single-threaded via `.cargo/config.toml` (`RUST_TEST_THREADS=1`) — **never** pass `--test-threads`.
- **Stack, not a branch:** one PR per task below, branch names `crash-reporter/<terse-change>`, gate suite run **per PR** (`make check-boundary`, `./scripts/check-plugins-typecheck.sh`, `./scripts/check-schema-generated.sh`, `./scripts/check-nav-generated.sh`, `./scripts/check-events-generated.sh`, `./scripts/check-csitem-generated.sh`, `./scripts/test-boundary-nameleak.sh`, `cargo test -p s2script-core`).

## Spec §13 open questions — resolved here

- **D-1 `server_id` derivation:** a random 128-bit hex id generated on first boot and persisted at `<spool>/server_id` (uuid v4, hyphens stripped). Rationale: any derivation from IP/hostname/steam account risks PII and instability across hosting moves; a persisted random id is stable, collision-safe, and carries zero information. If the file cannot be written, `server_id` degrades to `"unknown"` (fail-off).
- **D-2 fatal-JS definition + dedup vs degrade-per-descriptor:** "fatal JS error" = (a) an exception that escapes a plugin handler into one of the existing per-handler `TryCatch`es (`dispatch_onframe`, `dispatch_game_event`, plus `load_plugin_js`'s eval/onLoad branches — the multiplexer's `Err(())` path), and (b) an unhandled promise rejection via `Isolate::set_promise_reject_callback` (`PromiseRejectWithNoHandler`, cancelled by a later `PromiseHandlerAddedAfterReject`, reported at end of `frame_async_drain`). Dedup is by FNV-1a signature of `(plugin, message, top stack frame)`: first occurrence always reports, then ≥60 s between reports per signature, cap 5 per signature and 100 total per boot. Reporting is **orthogonal** to degrade-per-descriptor: the existing `error_count`/auto-disable machinery is untouched; once a handler auto-disables (10 errors) it stops dispatching, so reporting stops naturally — the rate limiter just keeps the first 10 from spamming.
- **D-3 capture vs upload when disabled:** capture paths always write local spool files; `enabled` gates only the sweep/upload. Rationale: local files carry no more privacy exposure than the core dump the OS already leaves on the operator's own disk, and a crash occurring before opt-in stays diagnosable.
- **D-4 native "artifacts written" notification:** not needed. The boot sweep is file-discovery-based (`.dmp` + `.dmp.s2meta` pairs in the spool dir); the crashing process is dead and the next boot needs no in-band signal. The C ABI therefore only adds the breadcrumb pointer/size getters and the identity push.
- **D-5 `detail.faulting_module`:** omitted in sub-project 1 (the field is optional in the schema). The capture client cannot parse minidumps; sub-project 2's symbolication derives it server-side.
- **D-6 `schema_build`:** FNV-1a 64 hex of the registered game-package JS bytes (the deployed `pawn.js` concat embeds the generated schema accessors), computed shim-side at Load and pushed with the identity block. No new codegen stamp needed.
- **D-7 CS2 build number source:** `IVEngineServer2::GetBuildVersion()` (verified: `third_party/hl2sdk/public/eiface.h:311`, on the `s_pEngine` interface the shim already acquires at Load). It is a Source 2 engine virtual — engine-generic — exposed as the appended op `server_build_number` and the native `__s2_server_build()`; the cs2 prelude pushes `__s2_crash_set_game("cs2", __s2_server_build())` so the *game name* still comes only from the game package.

---

## File Structure

**Created:**

| File | Responsibility |
|---|---|
| `core/src/crash/mod.rs` | Module root: spool-dir cell, `report_js_error`, re-exports; the one public surface other core modules call |
| `core/src/crash/breadcrumb.rs` | The `#[repr(C)]` POD + main-thread writer API (identity/game/map/players/tick/engine-op setters, `DispatchGuard`, ring, plugin table, snapshot) |
| `core/src/crash/envelope.rs` | The frozen `schema_version: 1` serde envelope, `Detail`, `Scrub`, POD→envelope `render`, `iso8601_utc` |
| `core/src/crash/panic_hook.rs` | `std::panic::set_hook` install (chains the previous hook), panic → envelope → spool |
| `core/src/crash/spool.rs` | Spool dir write/scan/mark-sent, bounded (50 pending / 50 sent) |
| `core/src/crash/config.rs` | `CrashConfig` (`enabled`/`endpoint`/`api_key`/`include_minidump`/`scrub_map`/`scrub_players`/`dev_test`), JSONC-tolerant parse, fail-off defaults |
| `core/src/crash/uploader.rs` | Boot + periodic sweep, `server_id` read-or-create, envelope finalize (patch `server_id`), multipart upload with retry via the shared tokio runtime |
| `core/src/crash/dedup.rs` | FNV-1a signatures + `RateLimiter` (per-sig interval + caps) |
| `shim/src/crash_handler.h` | `S2CrashArm`/`S2CrashDisarm` declarations |
| `shim/src/crash_handler.cpp` | Breakpad `ExceptionHandler` arming (sigaltstack, chaining) + the async-signal-safe `.s2meta` sidecar writer |
| `shim/src/crash_selftest.cpp` | Standalone fork-and-crash test executable asserting `.dmp` + `.s2meta` + SIGSEGV chaining |
| `third_party/breakpad` (submodule) | Vendored Google Breakpad (pinned) |
| `third_party/breakpad/src/third_party/lss` (submodule) | linux-syscall-support header Breakpad requires |
| `examples/crash-test/package.json`, `examples/crash-test/src/plugin.ts`, `examples/crash-test/tsconfig.json` | Deliberate-crash harness plugin (`sm_crashtest <kind>`), dev-only |

**Modified:**

| File | Change |
|---|---|
| `core/src/lib.rs` | `pub(crate) mod crash;` |
| `core/src/ffi.rs` | New C-ABI exports (`s2script_core_crash_breadcrumb`/`_size`/`_set_identity`), panic-hook install in `s2script_core_init` |
| `core/src/v8host.rs` | `S2EngineOps` tail append (`server_build_number`, later `crash_test_native`), dispatch stamping (`DispatchGuard`), natives `__s2_crash_set_game`/`__s2_server_build`/`__s2_crash_test`, `note_engine_op` at the four `s2_ent_ref_{read,write,read_chain,write_chain}` natives, promise-reject callback, TryCatch instrumentation, periodic-sweep call in `frame_async_drain`, plugin-table clears in `shutdown` |
| `core/src/loader.rs` | `crash::plugin_loaded/plugin_unloaded` at the load/unload call sites |
| `core/src/config.rs` | `strip_line_comments` visibility → `pub(crate)` (reused by crash config) |
| `core/Cargo.toml` | `uuid` dep; `reqwest` `multipart` feature |
| `shim/include/s2script_core.h` | Ops-struct tail append + the three new core exports |
| `shim/src/s2script_mm.cpp` | `s2_server_build_number` op, identity push (gamedata fingerprint/mtime, hl2sdk build, schema hash, spool dir) after core init, `S2CrashArm`/`S2CrashDisarm`, `s2_crash_test_native` op |
| `shim/CMakeLists.txt` | `breakpad_client` static lib + link, `crash_handler.cpp`, `crash_selftest` target, `S2_HL2SDK_BUILD` define |
| `games/cs2/js/pawn.js` | `__s2_crash_set_game("cs2", __s2_server_build())` push |
| `docs/PROGRESS.md` | Finished-slice entry (Task 6) |

---

### Task 1: Breadcrumb POD + tracker (+ the cs2 build-number setter)

**PR boundary:** branch `crash-reporter/breadcrumb` — atomic: POD + stamping + ABI append + shim op + cs2 push land together (the ops-struct append must land with both sides).

**Files:**
- Create: `core/src/crash/mod.rs`, `core/src/crash/breadcrumb.rs`
- Modify: `core/src/lib.rs`, `core/src/ffi.rs`, `core/src/v8host.rs`, `core/src/loader.rs:84-93` and `core/src/loader.rs:242,258`, `shim/include/s2script_core.h`, `shim/src/s2script_mm.cpp`, `games/cs2/js/pawn.js`
- Test: in-module `#[cfg(test)]` in `core/src/crash/breadcrumb.rs`, plus integration tests appended to the `#[cfg(test)]` mod in `core/src/ffi.rs`

**Interfaces:**
- Consumes: `v8host` thread-locals (`FRAME_COUNTER`, dispatch fns), `crate::loader::HOST_API_VERSION_MAJOR`, shim's `s_pEngine` (`IVEngineServer2`).
- Produces (later tasks rely on these exact names/types):
  - `crash::breadcrumb::{CrashBreadcrumb, RingEntry, PluginSlot, BREADCRUMB_MAGIC: u32, BREADCRUMB_VERSION: u32, RING_LEN: usize = 16, PLUGIN_TABLE_LEN: usize = 64}`
  - `crash::breadcrumb::breadcrumb_ptr() -> *const u8`, `breadcrumb_size() -> u32`, `snapshot() -> CrashBreadcrumb`
  - `crash::breadcrumb::set_identity(fingerprint: &str, generated_at: &str, hl2sdk: &str, schema_build: &str, stale: bool)`, `set_game(name: &str, build: u32)`, `set_map(map: &str)`, `set_players(n: i32)`, `note_tick(tick: u64, uptime_secs: u32)`, `note_engine_op(op: &str)`, `note_js_location(owner: &str, line: u32)`, `plugin_loaded(id: &str, version: &str)`, `plugin_unloaded(id: &str)`, `clear_plugins()`
  - `crash::breadcrumb::enter_dispatch(plugin: &str, dispatch: &str) -> DispatchGuard` (RAII: stamps + pushes a ring entry; `Drop` restores the previous plugin/dispatch)
  - helpers `crash::breadcrumb::copy_cstr(dst: &mut [u8], src: &str)` and `read_cstr(src: &[u8]) -> String`
  - C ABI: `s2script_core_crash_breadcrumb() -> *const u8`, `s2script_core_crash_breadcrumb_size() -> u32`, `s2script_core_crash_set_identity(fp, gen_at, hl2sdk, schema_build, gd_fail_count: c_int, spool_dir)` (spool-dir storage itself is a one-line cell in `crash/mod.rs`, consumed by Task 2)
  - `S2EngineOps.server_build_number: Option<ServerBuildNumberFn>` with `pub type ServerBuildNumberFn = extern "C" fn() -> c_int;`
  - JS natives: `__s2_crash_set_game(name: string, build: number)`, `__s2_server_build(): number`

- [ ] **Step 1: Write the failing unit tests**

Create `core/src/crash/breadcrumb.rs` containing ONLY the test mod for now (the impl comes in Step 3), and wire the module tree so it compiles as far as name resolution:

`core/src/lib.rs` — add after `mod net;`:

```rust
pub(crate) mod crash;
```

`core/src/crash/mod.rs`:

```rust
//! Crash-reporter capture client (engine-generic). Sub-project 1 of the crash-reporter spec.
//! No V8 types cross into this module; no game names ever appear here.
pub mod breadcrumb;
```

`core/src/crash/breadcrumb.rs`:

```rust
//! The breadcrumb: a fixed-size, pre-allocated #[repr(C)] POD in static memory that a signal
//! handler can read with plain memory loads. Written ONLY by the main thread (dispatch stamps);
//! torn reads are tolerated by design — the minidump is the source of truth for native stacks.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_guard_stamps_and_restores() {
        clear_plugins();
        {
            let _g = enter_dispatch("pluginA", "OnGameFrame:pre");
            let s = snapshot();
            assert_eq!(read_cstr(&s.plugin), "pluginA");
            assert_eq!(read_cstr(&s.dispatch), "OnGameFrame:pre");
            {
                let _inner = enter_dispatch("pluginB", "event:round_start");
                let s2 = snapshot();
                assert_eq!(read_cstr(&s2.plugin), "pluginB");
            }
            // inner drop restores the outer stamp
            let s3 = snapshot();
            assert_eq!(read_cstr(&s3.plugin), "pluginA");
        }
        let s4 = snapshot();
        assert_eq!(read_cstr(&s4.plugin), "core"); // guard drop restores the idle stamp
    }

    #[test]
    fn ring_records_last_16_in_order() {
        clear_plugins();
        for i in 0..20u64 {
            note_tick(i, 0);
            let _g = enter_dispatch(&format!("p{}", i), "d");
        }
        let s = snapshot();
        // head points at the next write slot; the 16 entries are ticks 4..=19
        let mut ticks: Vec<u64> = Vec::new();
        for k in 0..RING_LEN {
            let idx = (s.ring_head as usize + k) % RING_LEN;
            ticks.push(s.ring[idx].tick);
        }
        assert_eq!(ticks, (4..20).collect::<Vec<u64>>());
        assert_eq!(read_cstr(&s.ring[(s.ring_head as usize + RING_LEN - 1) % RING_LEN].plugin), "p19");
    }

    #[test]
    fn plugin_table_add_remove_and_overflow() {
        clear_plugins();
        plugin_loaded("a", "1.0.0");
        plugin_loaded("b", "2.0.0");
        plugin_loaded("a", "1.0.1"); // reload updates in place, no duplicate
        let s = snapshot();
        assert_eq!(s.plugin_count, 2);
        let ids: Vec<(String, String)> = (0..s.plugin_count as usize)
            .map(|i| (read_cstr(&s.plugins[i].id), read_cstr(&s.plugins[i].version)))
            .collect();
        assert!(ids.contains(&("a".into(), "1.0.1".into())));
        plugin_unloaded("a");
        assert_eq!(snapshot().plugin_count, 1);
        // overflow: table is fixed-size; extra loads are dropped, never grow/realloc
        for i in 0..100 {
            plugin_loaded(&format!("p{}", i), "0.0.1");
        }
        assert_eq!(snapshot().plugin_count as usize, PLUGIN_TABLE_LEN);
    }

    #[test]
    fn identity_game_map_players_stamp() {
        set_identity("fp123", "1752710400", "hl2sdk-abc", "schema-def", true);
        set_game("cs2", 14099);
        set_map("de_dust2");
        set_players(7);
        note_engine_op("ent_ref_read");
        note_js_location("myplugin", 42);
        let s = snapshot();
        assert_eq!(s.magic, BREADCRUMB_MAGIC);
        assert_eq!(s.version, BREADCRUMB_VERSION);
        assert_eq!(read_cstr(&s.gamedata_fingerprint), "fp123");
        assert_eq!(s.gamedata_stale, 1);
        assert_eq!(read_cstr(&s.game_name), "cs2");
        assert_eq!(s.game_build, 14099);
        assert_eq!(read_cstr(&s.map), "de_dust2");
        assert_eq!(s.players, 7);
        assert_eq!(read_cstr(&s.engine_op), "ent_ref_read");
        assert_eq!(read_cstr(&s.js_location), "myplugin:42");
        assert_eq!(read_cstr(&s.s2_version), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn copy_cstr_truncates_and_terminates() {
        let mut buf = [0u8; 8];
        copy_cstr(&mut buf, "12345678901234");
        assert_eq!(read_cstr(&buf), "1234567"); // 7 chars + NUL
        copy_cstr(&mut buf, "ab");
        assert_eq!(read_cstr(&buf), "ab");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p s2script-core crash::breadcrumb`
Expected: **compile error** — `cannot find function \`clear_plugins\` in this scope` (etc.). That is the failing state.

- [ ] **Step 3: Write the breadcrumb implementation**

Prepend to `core/src/crash/breadcrumb.rs` (above the test mod):

```rust
use std::cell::UnsafeCell;

pub const BREADCRUMB_MAGIC: u32 = 0x5332_4352; // "S2CR" (LE bytes: 52 43 32 53)
pub const BREADCRUMB_VERSION: u32 = 1;
pub const RING_LEN: usize = 16;
pub const PLUGIN_TABLE_LEN: usize = 64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RingEntry {
    pub tick: u64,
    pub plugin: [u8; 32],
    pub dispatch: [u8; 48],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginSlot {
    pub id: [u8; 48],
    pub version: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CrashBreadcrumb {
    pub magic: u32,
    pub version: u32,
    // --- identity / treadmill ---
    pub s2_version: [u8; 32],
    pub api_version: u32,
    pub gamedata_fingerprint: [u8; 40],
    pub gamedata_generated_at: [u8; 32],
    pub hl2sdk_build: [u8; 32],
    pub schema_build: [u8; 40],
    pub gamedata_stale: u32, // 0/1
    pub game_name: [u8; 16],
    pub game_build: u32,
    pub map: [u8; 64],
    pub players: i32,
    // --- current context ---
    pub plugin: [u8; 32],
    pub dispatch: [u8; 48],
    pub engine_op: [u8; 32],
    pub js_location: [u8; 96],
    pub tick: u64,
    pub uptime_secs: u32,
    // --- ring buffer ---
    pub ring_head: u32, // next write index (mod RING_LEN)
    pub ring: [RingEntry; RING_LEN],
    // --- plugin table ---
    pub plugin_count: u32,
    pub plugins: [PluginSlot; PLUGIN_TABLE_LEN],
}

/// Static storage. Writers are main-thread-only (dispatch/engine ops run on the game thread);
/// the signal handler / panic hook read a best-effort snapshot and TOLERATE torn writes (spec
/// §6.1 threading note), so no lock is taken on either side.
struct BreadcrumbCell(UnsafeCell<CrashBreadcrumb>);
unsafe impl Sync for BreadcrumbCell {}

static BREADCRUMB: BreadcrumbCell = BreadcrumbCell(UnsafeCell::new(CrashBreadcrumb {
    magic: BREADCRUMB_MAGIC,
    version: BREADCRUMB_VERSION,
    s2_version: [0; 32],
    api_version: 0,
    gamedata_fingerprint: [0; 40],
    gamedata_generated_at: [0; 32],
    hl2sdk_build: [0; 32],
    schema_build: [0; 40],
    gamedata_stale: 0,
    game_name: [0; 16],
    game_build: 0,
    map: [0; 64],
    players: 0,
    plugin: [0; 32],
    dispatch: [0; 48],
    engine_op: [0; 32],
    js_location: [0; 96],
    tick: 0,
    uptime_secs: 0,
    ring_head: 0,
    ring: [RingEntry { tick: 0, plugin: [0; 32], dispatch: [0; 48] }; RING_LEN],
    plugin_count: 0,
    plugins: [PluginSlot { id: [0; 48], version: [0; 16] }; PLUGIN_TABLE_LEN],
}));

#[inline]
fn bc() -> &'static mut CrashBreadcrumb {
    // SAFETY: single-writer (main thread) by construction; readers tolerate tears.
    unsafe { &mut *BREADCRUMB.0.get() }
}

pub fn breadcrumb_ptr() -> *const u8 {
    BREADCRUMB.0.get() as *const u8
}

pub fn breadcrumb_size() -> u32 {
    std::mem::size_of::<CrashBreadcrumb>() as u32
}

/// Main-thread copy of the whole POD (used by the panic hook + JS-error path renderers).
pub fn snapshot() -> CrashBreadcrumb {
    unsafe { std::ptr::read(BREADCRUMB.0.get()) }
}

/// Bounded, NUL-terminated copy. Truncates to dst.len()-1 bytes; never allocates.
pub(crate) fn copy_cstr(dst: &mut [u8], src: &str) {
    let n = src.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&src.as_bytes()[..n]);
    dst[n..].iter_mut().for_each(|b| *b = 0);
}

/// Read a NUL-terminated fixed buffer back into a String (lossy).
pub(crate) fn read_cstr(src: &[u8]) -> String {
    let end = src.iter().position(|&b| b == 0).unwrap_or(src.len());
    String::from_utf8_lossy(&src[..end]).into_owned()
}

pub fn set_identity(fingerprint: &str, generated_at: &str, hl2sdk: &str, schema_build: &str, stale: bool) {
    let b = bc();
    copy_cstr(&mut b.s2_version, env!("CARGO_PKG_VERSION"));
    b.api_version = crate::loader::HOST_API_VERSION_MAJOR;
    copy_cstr(&mut b.gamedata_fingerprint, fingerprint);
    copy_cstr(&mut b.gamedata_generated_at, generated_at);
    copy_cstr(&mut b.hl2sdk_build, hl2sdk);
    copy_cstr(&mut b.schema_build, schema_build);
    b.gamedata_stale = if stale { 1 } else { 0 };
}

pub fn set_game(name: &str, build: u32) {
    let b = bc();
    copy_cstr(&mut b.game_name, name);
    b.game_build = build;
}

pub fn set_map(map: &str) { copy_cstr(&mut bc().map, map); }
pub fn set_players(n: i32) { bc().players = n.max(0); }
pub fn note_tick(tick: u64, uptime_secs: u32) { let b = bc(); b.tick = tick; b.uptime_secs = uptime_secs; }
pub fn note_engine_op(op: &str) { copy_cstr(&mut bc().engine_op, op); }

/// "owner:line" without allocation-heavy formatting (a handful of byte stores per dispatch).
pub fn note_js_location(owner: &str, line: u32) {
    let b = bc();
    let mut buf = [0u8; 96];
    let n = owner.len().min(84);
    buf[..n].copy_from_slice(&owner.as_bytes()[..n]);
    buf[n] = b':';
    let mut digits = [0u8; 10];
    let mut v = line;
    let mut d = 0usize;
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        v /= 10;
        d += 1;
        if v == 0 { break; }
    }
    for k in 0..d { buf[n + 1 + k] = digits[d - 1 - k]; }
    b.js_location = buf;
}

pub fn plugin_loaded(id: &str, version: &str) {
    let b = bc();
    // Update in place on reload.
    for i in 0..b.plugin_count as usize {
        if read_cstr(&b.plugins[i].id) == id {
            copy_cstr(&mut b.plugins[i].version, version);
            return;
        }
    }
    if (b.plugin_count as usize) < PLUGIN_TABLE_LEN {
        let i = b.plugin_count as usize;
        copy_cstr(&mut b.plugins[i].id, id);
        copy_cstr(&mut b.plugins[i].version, version);
        b.plugin_count += 1;
    } // else: table full → drop (fixed-size, never grows)
}

pub fn plugin_unloaded(id: &str) {
    let b = bc();
    let count = b.plugin_count as usize;
    for i in 0..count {
        if read_cstr(&b.plugins[i].id) == id {
            b.plugins[i] = b.plugins[count - 1];
            b.plugins[count - 1] = PluginSlot { id: [0; 48], version: [0; 16] };
            b.plugin_count -= 1;
            return;
        }
    }
}

pub fn clear_plugins() {
    let b = bc();
    b.plugins = [PluginSlot { id: [0; 48], version: [0; 16] }; PLUGIN_TABLE_LEN];
    b.plugin_count = 0;
    copy_cstr(&mut b.plugin, "core");
    copy_cstr(&mut b.dispatch, "idle");
}

/// RAII dispatch stamp: sets plugin+dispatch, pushes a ring entry; Drop restores the previous
/// stamp (supports nesting — e.g. an event fired from inside a frame handler).
pub struct DispatchGuard {
    prev_plugin: [u8; 32],
    prev_dispatch: [u8; 48],
}

pub fn enter_dispatch(plugin: &str, dispatch: &str) -> DispatchGuard {
    let b = bc();
    let g = DispatchGuard { prev_plugin: b.plugin, prev_dispatch: b.dispatch };
    copy_cstr(&mut b.plugin, plugin);
    copy_cstr(&mut b.dispatch, dispatch);
    let idx = (b.ring_head as usize) % RING_LEN;
    b.ring[idx].tick = b.tick;
    b.ring[idx].plugin = b.plugin;
    b.ring[idx].dispatch = b.dispatch;
    b.ring_head = ((idx + 1) % RING_LEN) as u32;
    g
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        let b = bc();
        b.plugin = self.prev_plugin;
        b.dispatch = self.prev_dispatch;
    }
}
```

Note: `enter_dispatch` with `"core"`/`"idle"` defaults means the *idle* stamp is only meaningful after `clear_plugins()` has run once; `v8host::init` will call it (Step 5).

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p s2script-core crash::breadcrumb`
Expected: `test result: ok. 5 passed` (test names: `dispatch_guard_stamps_and_restores`, `ring_records_last_16_in_order`, `plugin_table_add_remove_and_overflow`, `identity_game_map_players_stamp`, `copy_cstr_truncates_and_terminates`).

- [ ] **Step 5: Commit, then write the failing integration test for stamping + FFI**

```bash
git add core/src/crash core/src/lib.rs
git commit -m "crash-reporter: breadcrumb POD + tracker (ring, plugin table, dispatch guard)"
```

(If this is the first commit on the stack: `git add` then `gt create crash-reporter/breadcrumb -m "crash-reporter: breadcrumb POD + tracker"` instead; in a fresh worktree run `gt track -p main` first.)

Append to the `#[cfg(test)] mod tests` in `core/src/ffi.rs` (uses the existing `test_logger`):

```rust
    #[test]
    fn breadcrumb_ffi_exports_and_dispatch_stamping() {
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        // FFI exports: non-null pointer, size matches the POD, magic readable through the pointer.
        let ptr = s2script_core_crash_breadcrumb();
        assert!(!ptr.is_null());
        assert_eq!(
            s2script_core_crash_breadcrumb_size() as usize,
            std::mem::size_of::<crate::crash::breadcrumb::CrashBreadcrumb>()
        );
        let magic = unsafe { *(ptr as *const u32) };
        assert_eq!(magic, crate::crash::breadcrumb::BREADCRUMB_MAGIC);

        // Identity push (shim-side call simulated).
        let fp = std::ffi::CString::new("fp-1").unwrap();
        let gen = std::ffi::CString::new("1752710400").unwrap();
        let sdk = std::ffi::CString::new("dota-abc123").unwrap();
        let sb = std::ffi::CString::new("schema-77").unwrap();
        let dir = std::ffi::CString::new("/tmp/spool").unwrap();
        s2script_core_crash_set_identity(fp.as_ptr(), gen.as_ptr(), sdk.as_ptr(), sb.as_ptr(), 0, dir.as_ptr());
        let s = crate::crash::breadcrumb::snapshot();
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s.gamedata_fingerprint), "fp-1");
        assert_eq!(s.gamedata_stale, 0);

        // A frame dispatch stamps plugin+dispatch and pushes a ring entry.
        v8host::create_plugin_context("bc_test");
        v8host::eval_in_context(
            "bc_test",
            r#"
                const { OnGameFrame } = __s2require("@s2script/frame");
                globalThis._bcsub = OnGameFrame.subscribe(() => {});
            "#,
        )
        .unwrap();
        let head_before = crate::crash::breadcrumb::snapshot().ring_head;
        s2script_core_dispatch_game_frame(0, 1, 1, 0);
        let s2 = crate::crash::breadcrumb::snapshot();
        assert_ne!(s2.ring_head, head_before, "dispatch must push a ring entry");
        let last = (s2.ring_head as usize + crate::crash::breadcrumb::RING_LEN - 1)
            % crate::crash::breadcrumb::RING_LEN;
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s2.ring[last].plugin), "bc_test");
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s2.ring[last].dispatch), "OnGameFrame:pre");
        // After the dispatch returns, the current stamp is restored to core/idle.
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s2.plugin), "core");

        // The cs2-package setter native is installed in plugin contexts.
        v8host::eval_in_context("bc_test", "__s2_crash_set_game('cs2', 14099);").unwrap();
        let s3 = crate::crash::breadcrumb::snapshot();
        assert_eq!(crate::crash::breadcrumb::read_cstr(&s3.game_name), "cs2");
        assert_eq!(s3.game_build, 14099);
        s2script_core_shutdown();
    }
```

Run: `cargo test -p s2script-core breadcrumb_ffi`
Expected: **compile error** — `cannot find function \`s2script_core_crash_breadcrumb\``.

- [ ] **Step 6: Implement the FFI exports + core wiring**

`core/src/ffi.rs` — add after `s2script_core_set_plugins_dir`:

```rust
/// Crash reporter: the breadcrumb POD base pointer. The shim's Breakpad callback reads
/// `s2script_core_crash_breadcrumb_size()` raw bytes from here with a single write() —
/// no JSON, no allocation (signal-safe by construction). The pointer targets static
/// memory in this cdylib (linked -z nodelete), so it stays valid for the process lifetime.
#[no_mangle]
pub extern "C" fn s2script_core_crash_breadcrumb() -> *const u8 {
    crate::crash::breadcrumb::breadcrumb_ptr()
}

#[no_mangle]
pub extern "C" fn s2script_core_crash_breadcrumb_size() -> u32 {
    crate::crash::breadcrumb::breadcrumb_size()
}

/// Crash reporter: the shim pushes the treadmill identity block + the crash-spool dir at Load
/// (after `s2script_core_init`). `gd_fail_count > 0` marks the gamedata as stale in every
/// envelope. Null pointers degrade to "" (never crash). Also records the spool dir for the
/// capture paths (Task 2) and schedules the boot sweep (Task 3).
#[no_mangle]
pub extern "C" fn s2script_core_crash_set_identity(
    fingerprint: *const c_char,
    generated_at: *const c_char,
    hl2sdk: *const c_char,
    schema_build: *const c_char,
    gd_fail_count: c_int,
    spool_dir: *const c_char,
) {
    let _ = catch_unwind(|| {
        fn s(p: *const c_char) -> String {
            if p.is_null() { return String::new(); }
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
        crate::crash::breadcrumb::set_identity(
            &s(fingerprint), &s(generated_at), &s(hl2sdk), &s(schema_build), gd_fail_count > 0,
        );
        crate::crash::set_spool_dir(&s(spool_dir));
    });
}
```

`core/src/crash/mod.rs` — add the spool-dir cell (consumed by Tasks 2/3):

```rust
use std::path::PathBuf;
use std::sync::Mutex;

static SPOOL_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Record the crash-spool directory (pushed by the shim with the identity block). Empty → None.
pub fn set_spool_dir(path: &str) {
    let mut g = match SPOOL_DIR.lock() { Ok(g) => g, Err(p) => p.into_inner() };
    *g = if path.is_empty() { None } else { Some(PathBuf::from(path)) };
}

pub fn spool_dir() -> Option<PathBuf> {
    match SPOOL_DIR.lock() { Ok(g) => g.clone(), Err(p) => p.into_inner().clone() }
}
```

`core/src/v8host.rs` — five edits:

(a) `S2EngineOps` tail append (after `voice_get_muted`, keeping the ABI order comment style):

```rust
    // --- Crash-reporter slice — APPENDED after voice_get_muted; order is the ABI; do not reorder above. ---
    pub server_build_number: Option<ServerBuildNumberFn>,
```

and with the other fn-pointer type aliases:

```rust
// --- Crash-reporter slice: engine build number (C-ABI; the C header must match exactly) ---
pub type ServerBuildNumberFn = extern "C" fn() -> c_int;
```

Every existing test that constructs an `S2EngineOps` literal (`core/src/v8host.rs:11321`, `:12189`, and siblings — find them with `grep -n "config_read: None" core/src/v8host.rs`) gains `server_build_number: None,` at the tail.

(b) Natives — add above `install_natives`:

```rust
/// Native `__s2_crash_set_game(name, build)` — the engine-generic setter the GAME PACKAGE calls to
/// stamp the game identity into the crash breadcrumb (core never knows which game; spec §5).
fn s2_crash_set_game(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        let build = args.get(1).uint32_value(scope).unwrap_or(0);
        crate::crash::breadcrumb::set_game(&name, build);
    }));
}

/// Native `__s2_server_build() -> number` — the engine's build number via the appended
/// `server_build_number` op (IVEngineServer2::GetBuildVersion; engine-generic). 0 = unavailable.
fn s2_server_build(
    _scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = ENGINE_OPS
            .with(|o| o.get())
            .and_then(|ops| ops.server_build_number)
            .map(|f| f())
            .unwrap_or(0);
        rv.set_int32(n);
    }));
}
```

In `install_natives` (next to the `__s2_current_plugin` registration at `core/src/v8host.rs:6981`):

```rust
    set_native(scope, global_obj, "__s2_crash_set_game", s2_crash_set_game);
    set_native(scope, global_obj, "__s2_server_build", s2_server_build);
```

(c) Dispatch stamping. In `dispatch_onframe` (`core/src/v8host.rs:8758`), inside the `run_chain` closure right after the owner's context Global is cloned (after the `else { return Ok(HookResult::Continue); };` block at `:8789-8792`):

```rust
            let _crash_guard = crate::crash::breadcrumb::enter_dispatch(
                owner,
                if phase == Phase::Pre { "OnGameFrame:pre" } else { "OnGameFrame:post" },
            );
```

and just before `let func = v8::Local::new(tc, &jh.func);` (`:8824`), the cheap JS-location stamp:

```rust
            crate::crash::breadcrumb::note_js_location(
                owner,
                v8::Local::new(tc, &jh.func).get_script_line_number().map(|l| l + 1).unwrap_or(0),
            );
```

At the TOP of `dispatch_onframe` (before the Phase 1 snapshot), the per-tick counters:

```rust
    if phase == Phase::Pre {
        crate::crash::breadcrumb::note_tick(
            FRAME_COUNTER.with(|c| c.get()),
            UPTIME_START.with(|t| t.get().map(|s| s.elapsed().as_secs() as u32).unwrap_or(0)),
        );
    }
```

with a new thread-local next to `FRAME_COUNTER` (`core/src/v8host.rs:472`):

```rust
    /// Boot instant for the breadcrumb's uptime field (set once in `init`).
    static UPTIME_START: std::cell::Cell<Option<Instant>> = std::cell::Cell::new(None);
```

and in `pub fn init` (after `crate::http::init();` at `:8646`):

```rust
    UPTIME_START.with(|t| if t.get().is_none() { t.set(Some(Instant::now())) });
    crate::crash::breadcrumb::clear_plugins(); // establishes the "core"/"idle" idle stamp
```

In `dispatch_game_event` (`:6554`), after the `g_ctx` clone (`:6571`):

```rust
            let _crash_guard = crate::crash::breadcrumb::enter_dispatch(owner, &format!("event:{}", name));
```

In `dispatch_damage` (`:6621`), after its `g_ctx` clone (`:6631`):

```rust
            let _crash_guard = crate::crash::breadcrumb::enter_dispatch(owner, "damage:onPre");
```

In `dispatch_map_start` (`:3933`), first line of the function body:

```rust
    crate::crash::breadcrumb::set_map(map);
```

In `dispatch_client_event` — locate `pub(crate) fn dispatch_client_event(name: &str, slot: i32)` and add as the first lines of the body (players tracking, engine-generic):

```rust
    {
        let s = crate::crash::breadcrumb::snapshot();
        match name {
            "putinserver" => crate::crash::breadcrumb::set_players(s.players + 1),
            "disconnect" => crate::crash::breadcrumb::set_players(s.players - 1),
            _ => {}
        }
    }
```

In `shutdown` (`:8865`), with the other bulk clears:

```rust
    crate::crash::breadcrumb::clear_plugins();
```

(d) Engine-op stamping at the four raw-memory natives — one line at the top of the `catch_unwind` body of each of `s2_ent_ref_read` (`:3380`), `s2_ent_ref_write` (`:3413`), `s2_ent_ref_read_chain` (`:3550`), `s2_ent_ref_write_chain` (`:3592`):

```rust
        crate::crash::breadcrumb::note_engine_op("ent_ref_read");        // (read)
        crate::crash::breadcrumb::note_engine_op("ent_ref_write");       // (write)
        crate::crash::breadcrumb::note_engine_op("ent_ref_read_chain");  // (read_chain)
        crate::crash::breadcrumb::note_engine_op("ent_ref_write_chain"); // (write_chain)
```

(Further ops adopt the same one-liner opportunistically in later slices; the field is best-effort.)

(e) `core/src/loader.rs` — plugin-table maintenance. In `load_and_reconcile` (`:84`), first line:

```rust
    crate::crash::breadcrumb::plugin_loaded(&manifest.id, &manifest.version);
```

(this also removes the `#[allow(dead_code)]` on `Manifest.version` — delete that attribute at `core/src/loader.rs:36`). At both `unload_plugin` call sites in the poll loop (`:242` and `:258`), immediately before each `crate::v8host::unload_plugin(&id);`:

```rust
                    crate::crash::breadcrumb::plugin_unloaded(&id);
```

- [ ] **Step 7: Run the integration test**

Run: `cargo test -p s2script-core breadcrumb_ffi`
Expected: `test ffi::tests::breadcrumb_ffi_exports_and_dispatch_stamping ... ok`

Then the full suite: `cargo test -p s2script-core`
Expected: all pre-existing tests still pass (the `S2EngineOps` literals updated in Step 6(a) compile).

- [ ] **Step 8: Commit, then the shim + cs2 side**

```bash
git add core/src
git commit -m "crash-reporter: FFI exports, dispatch stamping, server_build_number op (core side)"
```

`shim/include/s2script_core.h` — append after `s2_voice_get_muted_fn`'s typedef:

```c
/* Crash-reporter slice — APPENDED after voice_get_muted; order is the ABI.
 * server_build_number: the engine build via IVEngineServer2::GetBuildVersion(); 0 if the
 * interface is unavailable. Engine-generic (a Source 2 engine virtual, not a game name). */
typedef int (*s2_server_build_number_fn)(void);
```

in the `S2EngineOps` struct, after `s2_voice_get_muted_fn  voice_get_muted;`:

```c
    /* Crash-reporter slice — APPENDED after voice_get_muted; order is the ABI. */
    s2_server_build_number_fn server_build_number;
```

and with the other core exports (after `s2script_core_set_plugins_dir`):

```c
/* Crash reporter: the breadcrumb POD base pointer + byte size. The shim's crash callback
 * dumps exactly this many raw bytes with a single write() (signal-safe; no field access). */
const uint8_t* s2script_core_crash_breadcrumb(void);
uint32_t       s2script_core_crash_breadcrumb_size(void);
/* Crash reporter: push the treadmill identity block + the crash-spool dir (called once in
 * Load, after s2script_core_init). gd_fail_count > 0 marks gamedata stale. */
void s2script_core_crash_set_identity(const char* gamedata_fingerprint,
                                      const char* gamedata_generated_at,
                                      const char* hl2sdk_build,
                                      const char* schema_build,
                                      int gamedata_fail_count,
                                      const char* spool_dir);
```

(`#include <stdint.h>` is already at the top of the header.)

`shim/src/s2script_mm.cpp` — three edits:

(a) The op, next to the other `s2_*` op functions (e.g. after the `s2_server_game_time` implementation — find it with `grep -n "s2_server_game_time" shim/src/s2script_mm.cpp`):

```cpp
// Crash-reporter slice: the engine build number (IVEngineServer2::GetBuildVersion — a typed SDK
// virtual on the already-acquired s_pEngine; engine-generic). 0 = interface unavailable (degrade).
static int s2_server_build_number(void) {
    return s_pEngine ? s_pEngine->GetBuildVersion() : 0;
}
```

(b) The ops-table assignment at the tail of the `S2EngineOps ops = {};` block (`shim/src/s2script_mm.cpp:3583`, after `ops.gamerules_terminate_round` / the voice ops):

```cpp
    // Crash-reporter slice — APPENDED after voice_get_muted; order MUST match S2EngineOps.
    ops.server_build_number = &s2_server_build_number;
```

(c) The identity push — insert immediately after the `s2script_core_init` success check (`shim/src/s2script_mm.cpp:3707-3710`), BEFORE the `@s2script/cs2` registration block:

```cpp
    // --- Crash reporter: identity + spool-dir push (fail-off: any miss degrades to "") ---
    {
        // FNV-1a 64 over a file's bytes; also reused for the registered game-package JS below.
        auto fnv64hex = [](const std::string& bytes) -> std::string {
            uint64_t h = 0xcbf29ce484222325ULL;
            for (unsigned char c : bytes) { h ^= c; h *= 0x100000001b3ULL; }
            char out[17];
            snprintf(out, sizeof out, "%016llx", (unsigned long long)h);
            return std::string(out);
        };
        auto slurp = [](const std::string& path) -> std::string {
            FILE* f = fopen(path.c_str(), "rb");
            if (!f) return std::string();
            fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
            std::string s(sz > 0 ? (size_t)sz : 0, '\0');
            if (sz > 0 && fread(&s[0], 1, (size_t)sz, f) != (size_t)sz) s.clear();
            fclose(f);
            return s;
        };
        std::string gdPath = GamedataPath();
        std::string gdBytes = slurp(gdPath);
        std::string gdFp = gdBytes.empty() ? "" : fnv64hex(gdBytes);
        char gdMtime[32] = "";
        struct stat st{};
        if (stat(gdPath.c_str(), &st) == 0)
            snprintf(gdMtime, sizeof gdMtime, "%lld", (long long)st.st_mtime);
        std::string schemaHash;
        {
            std::string js = slurp(Cs2JsPath());   // the deployed pawn.js concat carries the
            if (!js.empty()) schemaHash = fnv64hex(js);  // generated schema accessors (D-6)
        }
        std::string spool = CrashSpoolDir();
#ifndef S2_HL2SDK_BUILD
#define S2_HL2SDK_BUILD "unknown"
#endif
        s2script_core_crash_set_identity(gdFp.c_str(), gdMtime, S2_HL2SDK_BUILD,
                                         schemaHash.c_str(), s_gdFail, spool.c_str());
        META_CONPRINTF("[s2script] crash identity pushed (gamedata %s, spool %s)\n",
                       gdFp.empty() ? "<none>" : gdFp.c_str(), spool.c_str());
    }
```

with the `CrashSpoolDir` helper next to `PluginsDir()` (`shim/src/s2script_mm.cpp:1815`), mirroring its dladdr walk (addon root = dirname ×3 from the .so):

```cpp
// CrashSpoolDir: addons/s2script/data/crashes, resolved relative to the plugin .so via dladdr
// (mirrors PluginsDir). Created (mkdir -p equivalent, two levels) if absent; "" on any failure
// (fail-off — crash reporting then stays disarmed).
static std::string CrashSpoolDir() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&CrashSpoolDir), &info) && info.dli_fname) {
        std::string dir(info.dli_fname);
        for (int i = 0; i < 1; i++) dir = dir.substr(0, dir.find_last_of('/'));
        std::string data = dir + "/data";
        std::string spool = data + "/crashes";
        mkdir(data.c_str(), 0755);            // EEXIST is fine
        if (mkdir(spool.c_str(), 0755) == 0 || errno == EEXIST) return spool;
    }
    return "";
}
```

(Match the dirname-walk depth to `PluginsDir()`'s actual body when editing — read it first; `s2script.so` sits in the addon root, so one `dirname` reaches `addons/s2script/`. Add `#include <sys/stat.h>` and `#include <errno.h>` to the include block at the top of the file if not already present.)

`games/cs2/js/pawn.js` — add at the very end of the IIFE body (before the closing `})();`):

```js
  // Crash reporter: push the game identity into the engine-generic breadcrumb (spec §5 — the
  // game package supplies the value IN; core never knows the game). Best-effort: absent natives
  // (an older core) degrade silently.
  if (typeof __s2_crash_set_game === "function" && typeof __s2_server_build === "function") {
    __s2_crash_set_game("cs2", __s2_server_build());
  }
```

- [ ] **Step 9: Build both sides + run the gate suite**

```bash
make core && make shim
cargo test -p s2script-core
make check-boundary
./scripts/check-plugins-typecheck.sh
./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh && ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh
./scripts/test-boundary-nameleak.sh
```

Expected: all green. (`check-boundary` proves the cs2 push kept core engine-generic.)

- [ ] **Step 10: Commit + submit the PR**

```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp games/cs2/js/pawn.js
git commit -m "crash-reporter: shim identity push + server_build_number op + cs2 game-identity setter"
gt submit --no-interactive
```

PR body: Stack Context (capture client for the crash reporter, 6-PR stack) + Why (the breadcrumb is the crux every capture path reads; no capture yet). Write with the Write tool + `gh pr edit N --body-file`.

---

### Task 2: Rust panic path → envelope → spool

**PR boundary:** branch `crash-reporter/panic-path` — the smallest capture path; proves envelope + spool end-to-end with no C++.

**Files:**
- Create: `core/src/crash/envelope.rs`, `core/src/crash/spool.rs`, `core/src/crash/panic_hook.rs`
- Modify: `core/src/crash/mod.rs`, `core/src/ffi.rs` (hook install), `core/Cargo.toml` (uuid)
- Test: in-module `#[cfg(test)]` in each new file

**Interfaces:**
- Consumes: `crash::breadcrumb::{snapshot, CrashBreadcrumb, read_cstr, RING_LEN}`, `crash::spool_dir()`.
- Produces:
  - `crash::envelope::SCHEMA_VERSION: u32 = 1`
  - `crash::envelope::{Envelope, S2Block, GamedataBlock, GameBlock, HostBlock, BreadcrumbBlock, RingJson, PluginJson, Detail, Scrub}` (serde Serialize+Deserialize; `Detail` is `#[serde(untagged)]` with variants `Native { minidump_ref: String }`, `Js { stack: String, message: String, file: String, line: u32 }`, `Panic { message: String, backtrace: String }`)
  - `crash::envelope::render(bc: &CrashBreadcrumb, kind: &str, detail: Detail, occurred_at: Option<String>, server_id: &str, scrub: &Scrub) -> Envelope`
  - `crash::envelope::iso8601_utc(unix_secs: i64) -> String`
  - `crash::spool::write_incident(dir: &Path, envelope_json: &str) -> Option<PathBuf>` (writes `<uuid>.json`; refuses when ≥ `MAX_SPOOL = 50` pending)
  - `crash::spool::{SpoolItem, scan(dir: &Path) -> Vec<SpoolItem>, mark_sent(dir: &Path, files: &[PathBuf])}` with `enum SpoolItem { Envelope(PathBuf), Native { meta: PathBuf, dump: PathBuf } }` and `MAX_SENT = 50`
  - `crash::panic_hook::install()` (idempotent, chains the previous hook)

- [ ] **Step 1: Write the failing envelope tests**

`core/src/crash/envelope.rs` starts as tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash::breadcrumb;

    #[test]
    fn iso8601_epoch_and_known_date() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_752_710_400), "2025-07-17T00:00:00Z");
        assert_eq!(iso8601_utc(1_752_710_400 + 3661), "2025-07-17T01:01:01Z");
    }

    #[test]
    fn render_panic_envelope_matches_schema_v1() {
        breadcrumb::clear_plugins();
        breadcrumb::set_identity("fp-x", "1752710400", "sdk-a", "sch-b", false);
        breadcrumb::set_game("cs2", 14099);
        breadcrumb::set_map("de_inferno");
        breadcrumb::set_players(3);
        breadcrumb::plugin_loaded("myplugin", "1.2.3");
        let _g = breadcrumb::enter_dispatch("myplugin", "OnGameFrame:pre");
        let bc = breadcrumb::snapshot();
        let env = render(
            &bc,
            "panic",
            Detail::Panic { message: "boom".into(), backtrace: "bt".into() },
            Some(iso8601_utc(1_752_710_400)),
            "srv-1",
            &Scrub { map: false, players: false },
        );
        assert_eq!(env.schema_version, 1);
        assert_eq!(env.kind, "panic");
        assert_eq!(env.occurred_at.as_deref(), Some("2025-07-17T00:00:00Z"));
        assert_eq!(env.s2script.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(env.s2script.api_version, "1");
        assert_eq!(env.gamedata.fingerprint, "fp-x");
        assert!(!env.gamedata.stale);
        assert_eq!(env.game.name, "cs2");
        assert_eq!(env.game.build_number, 14099);
        assert_eq!(env.game.map, "de_inferno");
        assert_eq!(env.game.players, 3);
        assert_eq!(env.host.server_id, "srv-1");
        assert_eq!(env.host.os, std::env::consts::OS);
        assert_eq!(env.breadcrumb.plugin, "myplugin");
        assert_eq!(env.breadcrumb.dispatch, "OnGameFrame:pre");
        assert!(env.breadcrumb.ring.len() <= breadcrumb::RING_LEN);
        assert_eq!(env.plugins, vec![PluginJson { id: "myplugin".into(), version: "1.2.3".into() }]);
        assert!(!env.incident_id.is_empty());
        // Round-trip: the wire contract survives serialize → deserialize.
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, env);
        match back.detail {
            Detail::Panic { message, .. } => assert_eq!(message, "boom"),
            other => panic!("wrong detail variant: {:?}", other),
        }
    }

    #[test]
    fn scrub_toggles_blank_map_and_players() {
        breadcrumb::set_map("de_nuke");
        breadcrumb::set_players(9);
        let bc = breadcrumb::snapshot();
        let env = render(&bc, "js",
            Detail::Js { stack: "s".into(), message: "m".into(), file: "f".into(), line: 1 },
            None, "srv", &Scrub { map: true, players: true });
        assert_eq!(env.game.map, "");
        assert_eq!(env.game.players, 0);
        assert!(env.occurred_at.is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core crash::envelope`
Expected: compile error — `cannot find function \`iso8601_utc\``. (Add `pub mod envelope;` to `core/src/crash/mod.rs` first so the module is reached.)

- [ ] **Step 3: Implement the envelope**

Add `uuid` to `core/Cargo.toml` under `[dependencies]`:

```toml
# Crash reporter: incident ids + the persisted server id (v4 = random; getrandom-backed).
uuid = { version = "1", features = ["v4"] }
```

Prepend to `core/src/crash/envelope.rs`:

```rust
//! The incident envelope — the FROZEN schema_version 1 wire contract between the capture client
//! and the central backend (spec §6.5). Evolving any field requires bumping SCHEMA_VERSION.
use crate::crash::breadcrumb::{read_cstr, CrashBreadcrumb, RING_LEN};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct S2Block { pub version: String, pub api_version: String }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct GamedataBlock {
    pub fingerprint: String, pub generated_at: String, pub hl2sdk: String,
    pub schema_build: String, pub stale: bool,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct GameBlock {
    pub name: String, pub build_number: u32, pub map: String, pub players: i32, pub uptime: u32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct HostBlock { pub server_id: String, pub os: String }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct RingJson { pub tick: u64, pub plugin: String, pub dispatch: String }

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct BreadcrumbBlock {
    pub plugin: String, pub dispatch: String, pub engine_op: String,
    pub js_location: String, pub ring: Vec<RingJson>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct PluginJson { pub id: String, pub version: String }

/// detail differs per kind; untagged = the plain objects of §6.5 (each variant has a
/// distinguishing required field set, so untagged deserialization is unambiguous).
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(untagged)]
pub enum Detail {
    Native { minidump_ref: String },
    Js { stack: String, message: String, file: String, line: u32 },
    Panic { message: String, backtrace: String },
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Envelope {
    pub schema_version: u32,
    pub incident_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<String>,
    pub s2script: S2Block,
    pub gamedata: GamedataBlock,
    pub game: GameBlock,
    pub host: HostBlock,
    pub breadcrumb: BreadcrumbBlock,
    pub plugins: Vec<PluginJson>,
    pub detail: Detail,
}

/// Privacy scrub toggles (from crashreporter.json; Task 3 maps config → this).
pub struct Scrub { pub map: bool, pub players: bool }

/// ISO-8601 UTC from unix seconds (no chrono dep; Howard Hinnant's civil_from_days).
pub fn iso8601_utc(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, secs / 3600, (secs / 60) % 60, secs % 60)
}

/// Render a breadcrumb snapshot + detail into the wire envelope. Pure (no I/O); the caller
/// supplies occurred_at (None for native at capture — the uploader reconstructs from mtime).
pub fn render(
    bc: &CrashBreadcrumb,
    kind: &str,
    detail: Detail,
    occurred_at: Option<String>,
    server_id: &str,
    scrub: &Scrub,
) -> Envelope {
    let mut ring = Vec::new();
    for k in 0..RING_LEN {
        let idx = (bc.ring_head as usize + k) % RING_LEN;
        let e = &bc.ring[idx];
        if e.plugin[0] == 0 && e.tick == 0 { continue; } // never-written slot
        ring.push(RingJson { tick: e.tick, plugin: read_cstr(&e.plugin), dispatch: read_cstr(&e.dispatch) });
    }
    let plugins = (0..bc.plugin_count as usize)
        .map(|i| PluginJson { id: read_cstr(&bc.plugins[i].id), version: read_cstr(&bc.plugins[i].version) })
        .collect();
    Envelope {
        schema_version: SCHEMA_VERSION,
        incident_id: uuid::Uuid::new_v4().to_string(),
        kind: kind.to_string(),
        occurred_at,
        s2script: S2Block {
            version: read_cstr(&bc.s2_version),
            api_version: bc.api_version.to_string(),
        },
        gamedata: GamedataBlock {
            fingerprint: read_cstr(&bc.gamedata_fingerprint),
            generated_at: read_cstr(&bc.gamedata_generated_at),
            hl2sdk: read_cstr(&bc.hl2sdk_build),
            schema_build: read_cstr(&bc.schema_build),
            stale: bc.gamedata_stale != 0,
        },
        game: GameBlock {
            name: read_cstr(&bc.game_name),
            build_number: bc.game_build,
            map: if scrub.map { String::new() } else { read_cstr(&bc.map) },
            players: if scrub.players { 0 } else { bc.players },
            uptime: bc.uptime_secs,
        },
        host: HostBlock { server_id: server_id.to_string(), os: std::env::consts::OS.to_string() },
        breadcrumb: BreadcrumbBlock {
            plugin: read_cstr(&bc.plugin),
            dispatch: read_cstr(&bc.dispatch),
            engine_op: read_cstr(&bc.engine_op),
            js_location: read_cstr(&bc.js_location),
            ring,
        },
        plugins,
        detail,
    }
}
```

Note `render_panic_envelope_matches_schema_v1` asserts `s.s2_version` set by `set_identity` — the test calls `set_identity` first, matching the runtime order (identity is pushed at Load before any capture).

- [ ] **Step 4: Run the envelope tests**

Run: `cargo test -p s2script-core crash::envelope`
Expected: `3 passed`.

- [ ] **Step 5: Commit, then the failing spool tests**

```bash
git add core/src/crash/envelope.rs core/src/crash/mod.rs core/Cargo.toml Cargo.lock
gt create crash-reporter/panic-path -m "crash-reporter: schema_version 1 incident envelope (frozen wire contract)"
```

`core/src/crash/spool.rs`:

```rust
//! Crash spool: the on-disk handoff between capture (any context) and upload (next boot /
//! periodic sweep). Bounded both directions; every failure is a silent skip (fail-off).
use std::path::{Path, PathBuf};

pub const MAX_SPOOL: usize = 50;
pub const MAX_SENT: usize = 50;

#[derive(Debug, PartialEq)]
pub enum SpoolItem {
    /// A rendered envelope (<uuid>.json) — js/panic kinds.
    Envelope(PathBuf),
    /// A Breakpad pair: <stem>.dmp + <stem>.dmp.s2meta — native kind.
    Native { meta: PathBuf, dump: PathBuf },
}

/// Write one rendered envelope as <uuid>.json. None (skip) when the dir is missing/unwritable
/// or already holds MAX_SPOOL pending incidents (bounded disk).
pub fn write_incident(dir: &Path, envelope_json: &str) -> Option<PathBuf> {
    if scan(dir).len() >= MAX_SPOOL { return None; }
    let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&path, envelope_json).ok()?;
    Some(path)
}

/// Enumerate pending incidents: every *.json, plus every *.dmp that has a *.dmp.s2meta sidecar.
/// (A .dmp without a sidecar is still uploaded — breadcrumbless; a sidecar without a .dmp is
/// treated as an orphan and reported envelope-only by the uploader.)
pub fn scan(dir: &Path) -> Vec<SpoolItem> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if name.ends_with(".json") {
            out.push(SpoolItem::Envelope(p));
        } else if name.ends_with(".dmp") {
            let meta = PathBuf::from(format!("{}.s2meta", p.display()));
            out.push(SpoolItem::Native { meta, dump: p });
        }
    }
    out.sort_by_key(|i| match i {
        SpoolItem::Envelope(p) => p.clone(),
        SpoolItem::Native { dump, .. } => dump.clone(),
    });
    out
}

/// Move uploaded files into <dir>/sent/, pruning sent/ down to MAX_SENT (oldest first by mtime).
pub fn mark_sent(dir: &Path, files: &[PathBuf]) {
    let sent = dir.join("sent");
    let _ = std::fs::create_dir_all(&sent);
    for f in files {
        if let Some(name) = f.file_name() {
            let _ = std::fs::rename(f, sent.join(name));
        }
    }
    // Prune sent/ (oldest first).
    let Ok(entries) = std::fs::read_dir(&sent) else { return };
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let t = e.metadata().ok()?.modified().ok()?;
            Some((t, p))
        })
        .collect();
    if all.len() <= MAX_SENT { return; }
    all.sort_by_key(|(t, _)| *t);
    for (_, p) in all.iter().take(all.len() - MAX_SENT) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("s2crash-spool-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_scan_mark_sent_roundtrip() {
        let d = tmpdir("rt");
        let p = write_incident(&d, r#"{"schema_version":1}"#).expect("write");
        assert!(p.exists());
        // A native pair is discovered as one item.
        std::fs::write(d.join("aaaa.dmp"), b"MDMP").unwrap();
        std::fs::write(d.join("aaaa.dmp.s2meta"), b"meta").unwrap();
        let items = scan(&d);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|i| matches!(i, SpoolItem::Native { .. })));
        mark_sent(&d, &[p.clone(), d.join("aaaa.dmp"), d.join("aaaa.dmp.s2meta")]);
        assert!(scan(&d).is_empty());
        assert!(d.join("sent").join(p.file_name().unwrap()).exists());
    }

    #[test]
    fn spool_is_bounded_at_max() {
        let d = tmpdir("cap");
        for _ in 0..MAX_SPOOL {
            assert!(write_incident(&d, "{}").is_some());
        }
        assert!(write_incident(&d, "{}").is_none(), "51st incident must be dropped");
    }
}
```

Add `pub mod spool;` to `core/src/crash/mod.rs`. Run: `cargo test -p s2script-core crash::spool`
Expected: `2 passed` (this module is written test-with-impl in one step because the tests ARE the spec of trivial fs plumbing; the failing-first cycle was demonstrated by writing + running the test file before `mark_sent` existed if split — if you prefer strict TDD, comment out the impl fns, observe the compile failure, then restore).

- [ ] **Step 6: Commit, then the failing panic-hook test**

```bash
git add core/src/crash/spool.rs core/src/crash/mod.rs
git commit -m "crash-reporter: bounded crash spool (write/scan/mark-sent)"
```

`core/src/crash/panic_hook.rs`:

```rust
//! Rust panic → envelope(kind=panic) → spool. The existing catch_unwind in ffi.rs still keeps
//! the panic from crossing FFI (the process survives); this hook makes it REPORTED instead of
//! silently swallowed (spec §6.4). The hook chains the previous hook and must itself never panic.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Once;

static INSTALL: Once = Once::new();
/// Per-boot cap so a per-frame panicking descriptor cannot fill the spool (Task 5 adds
/// signature-level dedup on top of this).
static REPORTED: AtomicU32 = AtomicU32::new(0);
const MAX_PANICS_PER_BOOT: u32 = 32;

pub fn install() {
    INSTALL.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Everything best-effort; a failure here must never obscure the panic itself.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| report(info)));
            prev(info);
        }));
    });
}

fn report(info: &std::panic::PanicHookInfo) {
    if REPORTED.fetch_add(1, Ordering::Relaxed) >= MAX_PANICS_PER_BOOT { return; }
    let Some(dir) = crate::crash::spool_dir() else { return }; // identity not pushed yet → fail-off
    let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "panic (non-string payload)".to_string()
    };
    let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let bc = crate::crash::breadcrumb::snapshot();
    let env = crate::crash::envelope::render(
        &bc,
        "panic",
        crate::crash::envelope::Detail::Panic {
            message: if loc.is_empty() { msg } else { format!("{} ({})", msg, loc) },
            backtrace,
        },
        Some(crate::crash::envelope::iso8601_utc(now)),
        "", // server_id is patched in by the uploader at upload time (D-1 / Task 3)
        &crate::crash::envelope::Scrub { map: false, players: false }, // Task 3 threads config
    );
    if let Ok(json) = serde_json::to_string(&env) {
        let _ = crate::crash::spool::write_incident(&dir, &json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swallowed_panic_writes_a_panic_envelope_to_spool() {
        let d = std::env::temp_dir().join(format!("s2crash-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        crate::crash::set_spool_dir(d.to_str().unwrap());
        install();
        crate::crash::breadcrumb::set_identity("fp-p", "0", "sdk", "sch", false);
        // The ffi.rs pattern: catch_unwind swallows the panic — but the hook has already reported.
        let r = std::panic::catch_unwind(|| panic!("test-panic-boom"));
        assert!(r.is_err());
        let items = crate::crash::spool::scan(&d);
        assert_eq!(items.len(), 1);
        let crate::crash::spool::SpoolItem::Envelope(p) = &items[0] else { panic!("expected envelope") };
        let json = std::fs::read_to_string(p).unwrap();
        let env: crate::crash::envelope::Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env.kind, "panic");
        assert_eq!(env.schema_version, 1);
        match env.detail {
            crate::crash::envelope::Detail::Panic { message, backtrace } => {
                assert!(message.contains("test-panic-boom"));
                assert!(!backtrace.is_empty());
            }
            other => panic!("wrong detail: {:?}", other),
        }
        crate::crash::set_spool_dir(""); // don't leak the dir into other tests
    }
}
```

Add `pub mod panic_hook;` to `core/src/crash/mod.rs`.

Run: `cargo test -p s2script-core crash::panic_hook`
Expected first run: PASS only after the impl above is present — to observe the failing state, run after writing only the test mod: compile error `cannot find function \`install\``.

- [ ] **Step 7: Install the hook at core init**

`core/src/ffi.rs` — in `s2script_core_init`, first line inside the `catch_unwind`:

```rust
        crate::crash::panic_hook::install();
```

- [ ] **Step 8: Full suite + commit**

Run: `cargo test -p s2script-core`
Expected: all green (the panic test's stderr will show the chained default hook's panic output — expected, the test still passes).

```bash
git add core/src/crash core/src/ffi.rs
git commit -m "crash-reporter: panic hook -> envelope(kind=panic) -> spool (chains previous hook)"
```

Run the gate suite (all seven commands from Global Constraints), then `gt submit --no-interactive`.

---

### Task 3: Spool sweep + uploader + config

**PR boundary:** branch `crash-reporter/spool-uploader` — transport proven against a mock endpoint before the backend exists.

**Files:**
- Create: `core/src/crash/config.rs`, `core/src/crash/uploader.rs`
- Modify: `core/src/crash/mod.rs`, `core/src/config.rs:41` (`strip_line_comments` → `pub(crate)`), `core/src/v8host.rs` (periodic sweep in `frame_async_drain`, `read_engine_config` helper), `core/src/ffi.rs` (`set_identity` triggers the boot sweep), `core/src/crash/panic_hook.rs` (thread the config's scrub into `render`), `core/Cargo.toml` (`reqwest` `multipart` feature)
- Test: in-module `#[cfg(test)]` in `config.rs` and `uploader.rs`

**Interfaces:**
- Consumes: `crash::spool::{scan, SpoolItem, mark_sent}`, `crash::envelope::{render, Envelope, Detail, Scrub, iso8601_utc}`, `crate::http::{init, spawn}`, `crate::config::strip_line_comments`, Task 1's `crash::spool_dir()`.
- Produces:
  - `crash::config::CrashConfig { enabled: bool, endpoint: String, api_key: String, include_minidump: bool, scrub_map: bool, scrub_players: bool, dev_test: bool }` (all serde-defaulted), `crash::config::parse(json: Option<&str>) -> CrashConfig`, `crash::config::DEFAULT_ENDPOINT: &str = "https://s2script.com/api/crash/v1/ingest"`, `crash::config::load() -> CrashConfig` (reads `configs/crashreporter.json` via the `config_read` engine op)
  - `crash::config::scrub(cfg: &CrashConfig) -> crash::envelope::Scrub`
  - `crash::uploader::server_id(dir: &Path) -> String` (read-or-create `<dir>/server_id`; `"unknown"` on failure)
  - `crash::uploader::finalize(envelope_json: &str, server_id: &str) -> Option<String>` (parse → patch `host.server_id` → re-serialize)
  - `crash::uploader::sweep_now(dir: &Path, cfg: &CrashConfig)` (test-callable synchronous scan that spawns the async uploads)
  - `crash::uploader::boot_sweep()` and `crash::uploader::periodic_sweep()` (throttled; called from `frame_async_drain`)
  - `v8host::read_engine_config(id: &str) -> Option<String>` (a thin `pub(crate)` wrapper over the existing `config_file_content` at `core/src/v8host.rs:7416`)

- [ ] **Step 1: Write the failing config tests**

`core/src/crash/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_opt_out_and_fail_off() {
        let c = parse(None);
        assert!(!c.enabled, "enabled MUST default to false (opt-in)");
        assert_eq!(c.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(c.api_key, "");
        assert!(c.include_minidump);
        assert!(!c.scrub_map);
        assert!(!c.scrub_players);
        assert!(!c.dev_test);
    }

    #[test]
    fn parse_overrides_and_tolerates_jsonc_comments() {
        let c = parse(Some(
            r#"{
                // operator opted in
                "enabled": true,
                "endpoint": "http://127.0.0.1:9/ingest",
                "api_key": "k-123",
                "include_minidump": false,
                "scrub_map": true
            }"#,
        ));
        assert!(c.enabled);
        assert_eq!(c.endpoint, "http://127.0.0.1:9/ingest");
        assert_eq!(c.api_key, "k-123");
        assert!(!c.include_minidump);
        assert!(c.scrub_map);
        assert!(!c.scrub_players); // unspecified key keeps its default
    }

    #[test]
    fn malformed_json_degrades_to_defaults() {
        let c = parse(Some("{ not json"));
        assert!(!c.enabled);
        assert_eq!(c.endpoint, DEFAULT_ENDPOINT);
    }
}
```

Add `pub mod config;` to `core/src/crash/mod.rs`.
Run: `cargo test -p s2script-core crash::config` — Expected: compile error (`parse` not found).

- [ ] **Step 2: Implement the config**

`core/src/config.rs:41` — change `fn strip_line_comments` to `pub(crate) fn strip_line_comments`.

Prepend to `core/src/crash/config.rs`:

```rust
//! crashreporter.json — operator config (runtime infra, NOT plugin-permissioned; spec §6.7).
//! Fail-off: absent/malformed file → all defaults, reporter effectively disabled.
use serde::Deserialize;

pub const DEFAULT_ENDPOINT: &str = "https://s2script.com/api/crash/v1/ingest";

fn default_endpoint() -> String { DEFAULT_ENDPOINT.to_string() }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CrashConfig {
    #[serde(default)]
    pub enabled: bool, // opt-in: default FALSE
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_true")]
    pub include_minidump: bool,
    #[serde(default)]
    pub scrub_map: bool,
    #[serde(default)]
    pub scrub_players: bool,
    /// Dev-only: arms the deliberate-crash harness natives (Task 6). Never enable in production.
    #[serde(default)]
    pub dev_test: bool,
}

impl Default for CrashConfig {
    fn default() -> Self {
        CrashConfig {
            enabled: false,
            endpoint: DEFAULT_ENDPOINT.to_string(),
            api_key: String::new(),
            include_minidump: true,
            scrub_map: false,
            scrub_players: false,
            dev_test: false,
        }
    }
}

pub fn parse(json: Option<&str>) -> CrashConfig {
    json.and_then(|s| serde_json::from_str(&crate::config::strip_line_comments(s)).ok())
        .unwrap_or_default()
}

/// Read + parse configs/crashreporter.json via the config_read engine op (shim file I/O).
pub fn load() -> CrashConfig {
    parse(crate::v8host::read_engine_config("crashreporter").as_deref())
}

pub fn scrub(cfg: &CrashConfig) -> crate::crash::envelope::Scrub {
    crate::crash::envelope::Scrub { map: cfg.scrub_map, players: cfg.scrub_players }
}
```

`core/src/v8host.rs` — next to `config_file_content` (`:7416`):

```rust
/// Crash reporter: read an arbitrary configs/<id>.json via the config_read op (the same shim
/// path plugins' configs use). pub(crate) so crash::config can reach it without touching ops.
pub(crate) fn read_engine_config(id: &str) -> Option<String> {
    config_file_content(id)
}
```

Run: `cargo test -p s2script-core crash::config` — Expected: `3 passed`.

- [ ] **Step 3: Commit, then the failing uploader test**

```bash
git add core/src/crash/config.rs core/src/crash/mod.rs core/src/config.rs core/src/v8host.rs
gt create crash-reporter/spool-uploader -m "crash-reporter: crashreporter.json config (opt-in default-false, fail-off parse)"
```

`core/src/crash/uploader.rs` — tests first (they drive the whole API):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("s2crash-up-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// One-shot mock ingest endpoint: accepts one HTTP request, returns 200, hands back the body.
    fn spawn_ingest() -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Read until the socket would block long enough — headers+small body fit easily.
                s.set_read_timeout(Some(std::time::Duration::from_millis(300))).unwrap();
                while let Ok(n) = s.read(&mut chunk) {
                    if n == 0 { break; }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > 64 * 1024 { break; }
                }
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
            }
        });
        (port, rx)
    }

    #[test]
    fn server_id_is_created_once_and_stable() {
        let d = tmpdir("sid");
        let a = server_id(&d);
        let b = server_id(&d);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32, "uuid v4 hex, hyphens stripped");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn finalize_patches_server_id() {
        let json = r#"{"schema_version":1,"incident_id":"i","kind":"panic","s2script":{"version":"0.0.0","api_version":"1"},"gamedata":{"fingerprint":"","generated_at":"","hl2sdk":"","schema_build":"","stale":false},"game":{"name":"","build_number":0,"map":"","players":0,"uptime":0},"host":{"server_id":"","os":"linux"},"breadcrumb":{"plugin":"","dispatch":"","engine_op":"","js_location":"","ring":[]},"plugins":[],"detail":{"message":"m","backtrace":"b"}}"#;
        let out = finalize(json, "srv-42").unwrap();
        let env: crate::crash::envelope::Envelope = serde_json::from_str(&out).unwrap();
        assert_eq!(env.host.server_id, "srv-42");
        assert!(finalize("{ not json", "x").is_none());
    }

    #[test]
    fn sweep_uploads_envelope_and_marks_sent() {
        crate::http::init();
        let d = tmpdir("sweep");
        let (port, rx) = spawn_ingest();
        let cfg = crate::crash::config::CrashConfig {
            enabled: true,
            endpoint: format!("http://127.0.0.1:{}/ingest", port),
            api_key: "key-abc".into(),
            ..Default::default()
        };
        let bc = crate::crash::breadcrumb::snapshot();
        let env = crate::crash::envelope::render(
            &bc, "panic",
            crate::crash::envelope::Detail::Panic { message: "m".into(), backtrace: "b".into() },
            None, "", &crate::crash::envelope::Scrub { map: false, players: false });
        let json = serde_json::to_string(&env).unwrap();
        crate::crash::spool::write_incident(&d, &json).unwrap();

        sweep_now(&d, &cfg);

        // The upload runs on the shared tokio runtime; poll for the sent/ move.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if crate::crash::spool::scan(&d).is_empty() { break; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(crate::crash::spool::scan(&d).is_empty(), "incident must be marked sent");
        let body = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(body.contains("authorization: Bearer key-abc") || body.contains("Authorization: Bearer key-abc"));
        assert!(body.contains("schema_version"));
        assert!(body.contains(&server_id(&d)), "server_id patched into the uploaded envelope");
    }

    #[test]
    fn sweep_disabled_uploads_nothing() {
        crate::http::init();
        let d = tmpdir("disabled");
        crate::crash::spool::write_incident(&d, r#"{"x":1}"#).unwrap();
        let cfg = crate::crash::config::CrashConfig::default(); // enabled=false
        sweep_now(&d, &cfg);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(crate::crash::spool::scan(&d).len(), 1, "opt-out must not upload or consume");
    }
}
```

Add `pub mod uploader;` to `core/src/crash/mod.rs`.
Run: `cargo test -p s2script-core crash::uploader` — Expected: compile error (`server_id` not found).

- [ ] **Step 4: Implement the uploader**

`core/Cargo.toml` — extend the reqwest features:

```toml
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "multipart"] }
```

Prepend to `core/src/crash/uploader.rs`:

```rust
//! Upload-on-next-boot sweep. The handler side only ever WRITES files; this module (normal
//! context, shared tokio runtime) renders + uploads them with retry, marking each sent.
//! Fail-off throughout: any error leaves the file in place for the next sweep.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::crash::config::CrashConfig;
use crate::crash::spool::{self, SpoolItem};

/// Files currently being uploaded (guards the periodic sweep double-starting one file).
static INFLIGHT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static BOOT_SWEEP_DONE: AtomicBool = AtomicBool::new(false);
/// Unix seconds of the last periodic sweep (0 = never).
static LAST_SWEEP: Mutex<u64> = Mutex::new(0);
const SWEEP_INTERVAL_SECS: u64 = 300;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// D-1: random 128-bit hex persisted at <dir>/server_id; "unknown" if unreadable+unwritable.
pub fn server_id(dir: &Path) -> String {
    let path = dir.join("server_id");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let t = s.trim().to_string();
        if t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()) { return t; }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    match std::fs::write(&path, &id) {
        Ok(()) => id,
        Err(_) => "unknown".to_string(),
    }
}

/// Patch host.server_id into a spooled envelope (envelopes are spooled with it empty).
pub fn finalize(envelope_json: &str, server_id: &str) -> Option<String> {
    let mut env: crate::crash::envelope::Envelope = serde_json::from_str(envelope_json).ok()?;
    env.host.server_id = server_id.to_string();
    serde_json::to_string(&env).ok()
}

/// Boot-time sweep: triggered by the FIRST identity push (the spool dir arrives there), so it
/// runs once per process, after the ops table + http engine exist.
pub fn boot_sweep() {
    if BOOT_SWEEP_DONE.swap(true, Ordering::SeqCst) { return; }
    let Some(dir) = crate::crash::spool_dir() else { return };
    let cfg = crate::crash::config::load();
    sweep_now(&dir, &cfg);
}

/// Periodic sweep for the still-alive kinds (js/panic): every SWEEP_INTERVAL_SECS, from
/// frame_async_drain. Cheap early-outs; never blocks the frame.
pub fn periodic_sweep() {
    let now = now_secs();
    {
        let mut last = match LAST_SWEEP.lock() { Ok(g) => g, Err(p) => p.into_inner() };
        if now.saturating_sub(*last) < SWEEP_INTERVAL_SECS { return; }
        *last = now;
    }
    let Some(dir) = crate::crash::spool_dir() else { return };
    let cfg = crate::crash::config::load();
    sweep_now(&dir, &cfg);
}

/// Scan the spool and spawn one upload task per pending incident. Synchronous scan (test seam);
/// the network work runs on the shared tokio runtime (http::spawn — never a second runtime).
pub fn sweep_now(dir: &Path, cfg: &CrashConfig) {
    if !cfg.enabled { return; }
    let sid = server_id(dir);
    for item in spool::scan(dir) {
        let files: Vec<PathBuf> = match &item {
            SpoolItem::Envelope(p) => vec![p.clone()],
            SpoolItem::Native { meta, dump } => vec![dump.clone(), meta.clone()],
        };
        {
            let mut inflight = match INFLIGHT.lock() { Ok(g) => g, Err(p) => p.into_inner() };
            if files.iter().any(|f| inflight.contains(f)) { continue; }
            inflight.extend(files.iter().cloned());
        }
        let dir = dir.to_path_buf();
        let cfg = cfg.clone();
        let sid = sid.clone();
        crate::http::spawn(async move {
            let ok = upload_item(&dir, &item, &cfg, &sid).await;
            if ok { spool::mark_sent(&dir, &files); }
            let mut inflight = match INFLIGHT.lock() { Ok(g) => g, Err(p) => p.into_inner() };
            inflight.retain(|f| !files.contains(f));
        });
    }
}

/// Render (native) / finalize (envelope) + POST with 3 attempts (1s/2s/4s backoff).
async fn upload_item(dir: &Path, item: &SpoolItem, cfg: &CrashConfig, sid: &str) -> bool {
    // Build the envelope JSON + optional minidump bytes OUTSIDE the retry loop.
    let (json, dump_bytes): (String, Option<Vec<u8>>) = match item {
        SpoolItem::Envelope(p) => {
            let Ok(raw) = std::fs::read_to_string(p) else { return false };
            let Some(fin) = finalize(&raw, sid) else {
                // Unparseable spool file: consume it (move to sent) rather than retry forever.
                spool::mark_sent(dir, &[p.clone()]);
                return false;
            };
            (fin, None)
        }
        SpoolItem::Native { meta, dump } => {
            let bc = read_meta(meta); // zeroed breadcrumb when the sidecar is missing/short
            let occurred = std::fs::metadata(dump)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| crate::crash::envelope::iso8601_utc(d.as_secs() as i64));
            let dump_name = dump.file_name().and_then(|n| n.to_str()).unwrap_or("crash.dmp").to_string();
            let env = crate::crash::envelope::render(
                &bc,
                "native",
                crate::crash::envelope::Detail::Native { minidump_ref: dump_name },
                occurred,
                sid,
                &crate::crash::config::scrub(cfg),
            );
            let Ok(json) = serde_json::to_string(&env) else { return false };
            let bytes = if cfg.include_minidump { std::fs::read(dump).ok() } else { None };
            (json, bytes)
        }
    };

    let client = reqwest::Client::new();
    for attempt in 0u32..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(1 << (attempt - 1))).await;
        }
        let mut req = client
            .post(&cfg.endpoint)
            .header("authorization", format!("Bearer {}", cfg.api_key))
            .timeout(std::time::Duration::from_secs(30));
        req = match &dump_bytes {
            Some(bytes) => {
                let form = reqwest::multipart::Form::new()
                    .part("envelope", reqwest::multipart::Part::text(json.clone())
                        .mime_str("application/json").unwrap_or_else(|_| reqwest::multipart::Part::text(json.clone())))
                    .part("minidump", reqwest::multipart::Part::bytes(bytes.clone()).file_name("crash.dmp"));
                req.multipart(form)
            }
            None => req.header("content-type", "application/json").body(json.clone()),
        };
        match req.send().await {
            Ok(resp) if resp.status().is_success() => return true,
            _ => continue,
        }
    }
    false // all attempts failed: leave the file for the next sweep
}

/// Read a .s2meta sidecar back into a CrashBreadcrumb (validate size + magic; else zeroed).
fn read_meta(meta: &Path) -> crate::crash::breadcrumb::CrashBreadcrumb {
    use crate::crash::breadcrumb::{CrashBreadcrumb, BREADCRUMB_MAGIC};
    let zeroed = || unsafe { std::mem::MaybeUninit::<CrashBreadcrumb>::zeroed().assume_init() };
    let Ok(bytes) = std::fs::read(meta) else { return zeroed() };
    if bytes.len() != std::mem::size_of::<CrashBreadcrumb>() { return zeroed(); }
    let bc: CrashBreadcrumb = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const CrashBreadcrumb) };
    if bc.magic != BREADCRUMB_MAGIC { return zeroed(); }
    bc
}
```

(`CrashBreadcrumb` is all integers/byte-arrays, so the zeroed `MaybeUninit` is sound.)

Wire the sweeps:

- `core/src/ffi.rs` — in `s2script_core_crash_set_identity`, after `crate::crash::set_spool_dir(...)`:

```rust
        crate::crash::uploader::boot_sweep();
```

- `core/src/v8host.rs` — in `frame_async_drain` (`:9077`), add as the LAST line of the function body:

```rust
    crate::crash::uploader::periodic_sweep();
```

- `core/src/crash/panic_hook.rs` — replace the hardcoded `Scrub { map: false, players: false }` in `report` with:

```rust
        &crate::crash::config::scrub(&crate::crash::config::load()),
```

(`load()` degrades to defaults when no ops table exists — e.g. in tests.)

- [ ] **Step 5: Run the uploader tests**

Run: `cargo test -p s2script-core crash::uploader`
Expected: `4 passed` (`server_id_is_created_once_and_stable`, `finalize_patches_server_id`, `sweep_uploads_envelope_and_marks_sent`, `sweep_disabled_uploads_nothing`).

Then: `cargo test -p s2script-core` — all green.

- [ ] **Step 6: Gate suite + commit + submit**

```bash
git add core/src/crash core/src/ffi.rs core/src/v8host.rs core/Cargo.toml Cargo.lock
git commit -m "crash-reporter: boot+periodic sweep, multipart uploader with retry, persisted server_id"
```

Run the full gate suite, then `gt submit --no-interactive`.

---

### Task 4: Native fault path (Breakpad)

**PR boundary:** branch `crash-reporter/breakpad` — the riskiest slice: vendoring + signal-handler arming. Recommended executor: the strongest available model; every line in `DumpCallback` is audited for async-signal-safety.

**Files:**
- Create: `third_party/breakpad` (submodule), `third_party/breakpad/src/third_party/lss` (submodule), `shim/src/crash_handler.h`, `shim/src/crash_handler.cpp`, `shim/src/crash_selftest.cpp`
- Modify: `shim/CMakeLists.txt`, `shim/src/s2script_mm.cpp` (arm in `Load` after the identity push; disarm in `Unload`)
- Test: the `crash_selftest` executable (fork-and-crash, host + bullseye container)

**Interfaces:**
- Consumes: Task 1's `s2script_core_crash_breadcrumb()`/`_size()` and `CrashSpoolDir()`.
- Produces: `bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize)`, `void S2CrashDisarm(void)`; on a fault, `<spool>/<uuid>.dmp` + `<spool>/<uuid>.dmp.s2meta` (exactly the file shapes Task 3's `spool::scan` consumes).

- [ ] **Step 1: Vendor Breakpad (pinned submodules)**

```bash
git submodule add https://chromium.googlesource.com/breakpad/breakpad third_party/breakpad
git submodule add https://chromium.googlesource.com/linux-syscall-support third_party/breakpad/src/third_party/lss
git add .gitmodules third_party/breakpad
git commit -m "crash-reporter: vendor breakpad + lss (pinned submodules)"
```

(Then `gt create crash-reporter/breakpad -m ...` folds this commit onto the new branch if created in that order — create the branch first if following the strict flow: `gt create crash-reporter/breakpad -m "crash-reporter: vendor breakpad + lss (pinned submodules)"` after `git add`.)

- [ ] **Step 2: Write the failing selftest (the TDD cycle for C++)**

`shim/src/crash_selftest.cpp`:

```cpp
// Standalone crash-handler selftest: fork a child that arms the handler and SIGSEGVs; assert
// (1) the child died by SIGSEGV (chaining preserved — the crash was NOT swallowed),
// (2) exactly one .dmp and one .dmp.s2meta appeared in the spool dir,
// (3) the .s2meta content is byte-identical to the breadcrumb buffer.
#include "crash_handler.h"
#include <dirent.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static uint8_t g_breadcrumb[128];

int main(int argc, char** argv) {
    const char* dir = argc > 1 ? argv[1] : "/tmp/s2-crash-selftest";
    mkdir(dir, 0755);
    // Clean previous artifacts.
    if (DIR* d = opendir(dir)) {
        while (dirent* e = readdir(d)) {
            if (e->d_name[0] == '.') continue;
            char p[1024];
            snprintf(p, sizeof p, "%s/%s", dir, e->d_name);
            unlink(p);
        }
        closedir(d);
    }
    for (size_t i = 0; i < sizeof g_breadcrumb; i++) g_breadcrumb[i] = (uint8_t)(i * 7 + 1);

    pid_t pid = fork();
    if (pid == 0) {
        if (!S2CrashArm(dir, g_breadcrumb, sizeof g_breadcrumb)) _exit(3);
        volatile int* p = nullptr;
        *p = 42; // SIGSEGV
        _exit(0); // unreachable
    }
    int status = 0;
    waitpid(pid, &status, 0);
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGSEGV) {
        fprintf(stderr, "FAIL: child did not die by SIGSEGV (chaining broken?) status=%d\n", status);
        return 1;
    }
    int dmp = 0, meta = 0;
    char metaPath[1024] = {0};
    if (DIR* d = opendir(dir)) {
        while (dirent* e = readdir(d)) {
            size_t n = strlen(e->d_name);
            if (n > 4 && strcmp(e->d_name + n - 4, ".dmp") == 0) dmp++;
            if (n > 11 && strcmp(e->d_name + n - 11, ".dmp.s2meta") == 0) {
                meta++;
                snprintf(metaPath, sizeof metaPath, "%s/%s", dir, e->d_name);
            }
        }
        closedir(d);
    }
    if (dmp != 1 || meta != 1) {
        fprintf(stderr, "FAIL: expected 1 .dmp + 1 .s2meta, got dmp=%d meta=%d\n", dmp, meta);
        return 1;
    }
    FILE* f = fopen(metaPath, "rb");
    uint8_t back[sizeof g_breadcrumb];
    size_t rd = f ? fread(back, 1, sizeof back, f) : 0;
    if (f) fclose(f);
    if (rd != sizeof g_breadcrumb || memcmp(back, g_breadcrumb, sizeof g_breadcrumb) != 0) {
        fprintf(stderr, "FAIL: .s2meta content mismatch (read %zu bytes)\n", rd);
        return 1;
    }
    printf("OK: SIGSEGV chained, minidump + byte-exact .s2meta written to %s\n", dir);
    return 0;
}
```

`shim/src/crash_handler.h`:

```cpp
#ifndef S2_CRASH_HANDLER_H
#define S2_CRASH_HANDLER_H
#include <stdint.h>
// Arm the Breakpad ExceptionHandler: minidumps into spoolDir; on fault also write the raw
// breadcrumb bytes as <dump>.s2meta. Idempotent (second call is a no-op returning false).
// Returns false on empty dir (fail-off).
bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize);
void S2CrashDisarm(void);
#endif
```

Build now fails (no `crash_handler.cpp`, no CMake target) — that is the failing state:

Run: `cmake --build shim/build --target crash_selftest`
Expected: `unknown target 'crash_selftest'` (or an equivalent configure error).

- [ ] **Step 3: Implement the handler + build wiring**

`shim/src/crash_handler.cpp`:

```cpp
// Breakpad arming + the .s2meta sidecar writer (crash-reporter spec §6.2).
//
// GOLDEN RULE (spec §9): nothing reachable from DumpCallback may allocate, lock, format, or
// call non-async-signal-safe libc. The callback uses only open/write/close (POSIX AS-safe) and
// byte copies into fixed buffers, then returns false so previously installed handlers run —
// the process still dies / core-dumps exactly as it would have without us.
#include "crash_handler.h"
#include "client/linux/handler/exception_handler.h"
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

static google_breakpad::ExceptionHandler* s_handler = nullptr;
static const uint8_t* s_breadcrumb = nullptr;
static uint32_t s_breadcrumbSize = 0;

static bool DumpCallback(const google_breakpad::MinidumpDescriptor& descriptor,
                         void* /*context*/, bool /*succeeded*/) {
    // ASYNC-SIGNAL-SAFE ONLY from here down.
    if (s_breadcrumb && s_breadcrumbSize) {
        char meta[512];
        const char* p = descriptor.path(); // "<spool>/<uuid>.dmp" (fixed buffer inside Breakpad)
        size_t n = 0;
        while (p[n] != '\0' && n < sizeof(meta) - 8) { meta[n] = p[n]; n++; }
        memcpy(meta + n, ".s2meta", 8); // 7 chars + NUL
        int fd = open(meta, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd >= 0) {
            uint32_t off = 0;
            while (off < s_breadcrumbSize) {
                ssize_t w = write(fd, s_breadcrumb + off, s_breadcrumbSize - off);
                if (w <= 0) break;
                off += (uint32_t)w;
            }
            close(fd);
        }
    }
    // false => Breakpad restores + re-raises to any previously installed handler: never swallow
    // the crash, never suppress core dumps (spec §6.2 chaining requirement).
    return false;
}

bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize) {
    if (s_handler || !spoolDir || !spoolDir[0]) return false;
    s_breadcrumb = breadcrumb;
    s_breadcrumbSize = breadcrumbSize;
    google_breakpad::MinidumpDescriptor descriptor(spoolDir);
    // install_handler=true: Breakpad installs SIGSEGV/SIGABRT/SIGBUS/SIGFPE/SIGILL/SIGTRAP
    // handlers on its own dedicated sigaltstack (stack-overflow faults stay catchable) and
    // SAVES the previous handlers for restore-and-re-raise. server_fd=-1: in-process dumping
    // (the Accelerator model; out-of-process is a documented future hardening).
    s_handler = new google_breakpad::ExceptionHandler(
        descriptor, /*filter=*/nullptr, DumpCallback, /*context=*/nullptr,
        /*install_handler=*/true, /*server_fd=*/-1);
    return true;
}

void S2CrashDisarm(void) {
    delete s_handler; // ~ExceptionHandler restores the previous signal handlers
    s_handler = nullptr;
    s_breadcrumb = nullptr;
    s_breadcrumbSize = 0;
}
```

`shim/CMakeLists.txt` — add after the `add_library(s2script SHARED ...)` block:

```cmake
# --- Crash reporter (Breakpad, vendored) ---------------------------------------------------
# The Linux client only: exception_handler + the in-process minidump writer. Compiled as a
# static lib with the SAME Valve ABI define as the shim; linked into s2script.so (gc-sections
# drops the unused microdump/core-dump paths). Verified to build under Steam Runtime 3
# (glibc 2.31) in the rust:bullseye sniper container.
enable_language(ASM)
set(BREAKPAD ${CMAKE_SOURCE_DIR}/../third_party/breakpad/src)
add_library(breakpad_client STATIC
    ${BREAKPAD}/client/linux/crash_generation/crash_generation_client.cc
    ${BREAKPAD}/client/linux/dump_writer_common/thread_info.cc
    ${BREAKPAD}/client/linux/dump_writer_common/ucontext_reader.cc
    ${BREAKPAD}/client/linux/handler/exception_handler.cc
    ${BREAKPAD}/client/linux/handler/minidump_descriptor.cc
    ${BREAKPAD}/client/linux/log/log.cc
    ${BREAKPAD}/client/linux/microdump_writer/microdump_writer.cc
    ${BREAKPAD}/client/linux/minidump_writer/linux_dumper.cc
    ${BREAKPAD}/client/linux/minidump_writer/linux_ptrace_dumper.cc
    ${BREAKPAD}/client/linux/minidump_writer/minidump_writer.cc
    ${BREAKPAD}/client/linux/minidump_writer/pe_file.cc
    ${BREAKPAD}/client/minidump_file_writer.cc
    ${BREAKPAD}/common/convert_UTF.cc
    ${BREAKPAD}/common/md5.cc
    ${BREAKPAD}/common/string_conversion.cc
    ${BREAKPAD}/common/linux/breakpad_getcontext.S
    ${BREAKPAD}/common/linux/elf_core_dump.cc
    ${BREAKPAD}/common/linux/elfutils.cc
    ${BREAKPAD}/common/linux/file_id.cc
    ${BREAKPAD}/common/linux/guid_creator.cc
    ${BREAKPAD}/common/linux/linux_libc_support.cc
    ${BREAKPAD}/common/linux/memory_mapped_file.cc
    ${BREAKPAD}/common/linux/safe_readlink.cc
)
target_include_directories(breakpad_client PUBLIC ${BREAKPAD})
set_target_properties(breakpad_client PROPERTIES POSITION_INDEPENDENT_CODE ON)
target_compile_definitions(breakpad_client PRIVATE _GLIBCXX_USE_CXX11_ABI=0)
target_compile_options(breakpad_client PRIVATE -ffunction-sections -fdata-sections)

# The standalone fork-and-crash selftest (not shipped; run manually + in the sniper container).
add_executable(crash_selftest src/crash_selftest.cpp src/crash_handler.cpp)
target_include_directories(crash_selftest PRIVATE src)
target_link_libraries(crash_selftest PRIVATE breakpad_client)
target_compile_definitions(crash_selftest PRIVATE _GLIBCXX_USE_CXX11_ABI=0)
```

and add `src/crash_handler.cpp` to the `add_library(s2script SHARED ...)` source list (after `src/ekv.cpp`), plus link it:

```cmake
target_link_libraries(s2script PRIVATE breakpad_client)
```

(placed after the existing `target_link_libraries(s2script PRIVATE ...)` block).

- [ ] **Step 4: Run the selftest — host, then the sniper container**

```bash
make shim   # or: cmake -S shim -B shim/build && cmake --build shim/build -j
./shim/build/crash_selftest /tmp/s2-crash-selftest
```
Expected: `OK: SIGSEGV chained, minidump + byte-exact .s2meta written to /tmp/s2-crash-selftest`

```bash
docker run --rm -v "$PWD:/repo" -w /repo rust:bullseye bash -c \
  "apt-get update -qq && apt-get install -y -qq cmake g++ >/dev/null && \
   cmake -S shim -B /tmp/shim-bullseye -DCMAKE_BUILD_TYPE=Release && \
   cmake --build /tmp/shim-bullseye --target crash_selftest -j && \
   /tmp/shim-bullseye/crash_selftest /tmp/spool"
```
Expected: the same `OK:` line — proves Breakpad compiles + links + runs under glibc 2.31.

- [ ] **Step 5: Signal-safety audit (documented in the PR body)**

Enumerate every call reachable from `DumpCallback` and confirm async-signal-safe: `memcpy` (pure), `open`/`write`/`close` (POSIX AS-safe list), `descriptor.path()` (returns a pre-built fixed buffer — Breakpad builds the path at arm time, not in the handler). Breakpad's own writer uses `sys_*` syscall wrappers (lss) and a page allocator by design. Confirm NO: `malloc`, `printf`, `std::string`, locks, V8. Record the list in the PR body under "Signal-safety audit".

- [ ] **Step 6: Arm in Load / disarm in Unload**

`shim/src/s2script_mm.cpp` — add `#include "crash_handler.h"` to the include block. At the END of the crash-identity block added in Task 1 Step 8(c) (still inside `Load`, after `s2script_core_crash_set_identity(...)`):

```cpp
        // Arm Breakpad AFTER core init (spec §6.2) — boot-time crashes from here on are caught.
        // Fail-off: an empty spool dir leaves the reporter disarmed and the server running.
        if (!spool.empty() &&
            S2CrashArm(spool.c_str(), s2script_core_crash_breadcrumb(),
                       s2script_core_crash_breadcrumb_size())) {
            META_CONPRINTF("[s2script] crash handler armed (spool %s)\n", spool.c_str());
        } else {
            META_CONPRINTF("[s2script] WARN: crash handler NOT armed (spool dir unavailable)\n");
        }
```

In `S2ScriptPlugin::Unload` (`shim/src/s2script_mm.cpp:3758`), before the core shutdown call:

```cpp
    S2CrashDisarm();   // restore previous signal handlers before the core is torn down
```

- [ ] **Step 7: Full builds + gate suite + commit + submit**

```bash
make core && make shim
cargo test -p s2script-core
make check-boundary && ./scripts/check-plugins-typecheck.sh && ./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh && ./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh && ./scripts/test-boundary-nameleak.sh
git add shim third_party .gitmodules
git commit -m "crash-reporter: Breakpad native fault path (sigaltstack, chaining, .s2meta sidecar) + selftest"
gt submit --no-interactive
```

Also verify the sniper build end-to-end once: `docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh` — expected: builds green (record in the PR body).

---

### Task 5: Fatal JS error path (V8 callbacks → envelope, dedup/rate-limit)

**PR boundary:** branch `crash-reporter/js-errors` — V8-callback wiring is subtle (CallbackScope in a raw `extern "C"` callback); recommended executor: the strongest available model.

**Files:**
- Create: `core/src/crash/dedup.rs`
- Modify: `core/src/crash/mod.rs` (`report_js_error` + the pending-rejects drain hook), `core/src/v8host.rs` (promise-reject callback registration + `PENDING_REJECTS` + TryCatch instrumentation at `dispatch_onframe`, `dispatch_game_event`, `load_plugin_js`), `core/src/crash/panic_hook.rs` (adopt signature dedup)
- Test: in-module in `dedup.rs`; integration tests appended to the `#[cfg(test)]` mod in `core/src/ffi.rs`

**Interfaces:**
- Consumes: Task 2's `envelope::{render, Detail, Scrub}` + `spool::write_incident`, Task 3's `config::{load, scrub}`, Task 1's `breadcrumb::snapshot`.
- Produces:
  - `crash::dedup::fnv1a64(parts: &[&str]) -> u64`
  - `crash::dedup::RateLimiter` with `RateLimiter::new()`, `should_report(&mut self, sig: u64, now_secs: u64) -> bool`, consts `MIN_INTERVAL_SECS: u64 = 60`, `PER_SIG_CAP: u32 = 5`, `TOTAL_CAP: u32 = 100`
  - `crash::report_js_error(plugin: &str, dispatch: &str, message: &str, stack: &str)` (dedup → envelope kind=js → spool; parses `file:line` from the stack's first frame)
  - `crash::parse_top_frame(stack: &str) -> (String, u32)` (pub(crate), unit-tested)
  - `v8host` internal: `promise_reject_cb` (registered via `isolate.set_promise_reject_callback` in `init` and `create_plugin_context`'s isolate — one isolate, so once in `init`), `PENDING_REJECTS` thread-local, drained at the end of `frame_async_drain`

- [ ] **Step 1: Write the failing dedup tests**

`core/src/crash/dedup.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable_and_part_sensitive() {
        let a = fnv1a64(&["p", "msg", "frame"]);
        assert_eq!(a, fnv1a64(&["p", "msg", "frame"]));
        assert_ne!(a, fnv1a64(&["p", "msg", "other"]));
        assert_ne!(fnv1a64(&["ab", "c"]), fnv1a64(&["a", "bc"]), "part boundaries must matter");
    }

    #[test]
    fn rate_limiter_first_then_interval_then_caps() {
        let mut rl = RateLimiter::new();
        let sig = 42u64;
        assert!(rl.should_report(sig, 1000), "first occurrence always reports");
        assert!(!rl.should_report(sig, 1010), "within 60s: suppressed");
        assert!(rl.should_report(sig, 1061), "after 60s: reports again");
        assert!(rl.should_report(sig, 1200));
        assert!(rl.should_report(sig, 1300));
        assert!(rl.should_report(sig, 1400)); // 5th report
        assert!(!rl.should_report(sig, 2000), "PER_SIG_CAP=5 reached");
        // Different signature unaffected.
        assert!(rl.should_report(7, 2000));
    }

    #[test]
    fn rate_limiter_total_cap() {
        let mut rl = RateLimiter::new();
        let mut reported = 0;
        for sig in 0..200u64 {
            if rl.should_report(sig, 5000) { reported += 1; }
        }
        assert_eq!(reported, RateLimiter::TOTAL_CAP as usize);
    }
}
```

Add `pub mod dedup;` to `core/src/crash/mod.rs`.
Run: `cargo test -p s2script-core crash::dedup` — Expected: compile error.

- [ ] **Step 2: Implement dedup**

Prepend to `core/src/crash/dedup.rs`:

```rust
//! Stack-signature dedup + rate limiting (spec §6.3): a per-frame thrower must not spam the
//! pipeline. Orthogonal to degrade-per-descriptor — error_count/auto-disable is untouched (D-2).
use std::collections::HashMap;

pub fn fnv1a64(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        // Part separator so ["ab","c"] != ["a","bc"].
        h ^= 0x1f;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

pub struct RateLimiter {
    last_report: HashMap<u64, u64>, // sig → unix secs of last report
    per_sig: HashMap<u64, u32>,
    total: u32,
}

impl RateLimiter {
    pub const MIN_INTERVAL_SECS: u64 = 60;
    pub const PER_SIG_CAP: u32 = 5;
    pub const TOTAL_CAP: u32 = 100;

    pub fn new() -> Self {
        RateLimiter { last_report: HashMap::new(), per_sig: HashMap::new(), total: 0 }
    }

    pub fn should_report(&mut self, sig: u64, now_secs: u64) -> bool {
        if self.total >= Self::TOTAL_CAP { return false; }
        let count = *self.per_sig.get(&sig).unwrap_or(&0);
        if count >= Self::PER_SIG_CAP { return false; }
        if let Some(&last) = self.last_report.get(&sig) {
            if now_secs.saturating_sub(last) < Self::MIN_INTERVAL_SECS { return false; }
        }
        self.last_report.insert(sig, now_secs);
        self.per_sig.insert(sig, count + 1);
        self.total += 1;
        true
    }
}

impl Default for RateLimiter { fn default() -> Self { Self::new() } }
```

Run: `cargo test -p s2script-core crash::dedup` — Expected: `3 passed`.

- [ ] **Step 3: Commit, then `report_js_error` + frame parsing**

```bash
git add core/src/crash/dedup.rs core/src/crash/mod.rs
gt create crash-reporter/js-errors -m "crash-reporter: stack-signature dedup + rate limiter"
```

Add to `core/src/crash/mod.rs`:

```rust
/// Best-effort "file:line" from a V8 stack's first frame:
///   "    at fn (plugin.js:12:5)"  /  "    at plugin.js:12:5"
pub(crate) fn parse_top_frame(stack: &str) -> (String, u32) {
    for line in stack.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("at ") else { continue };
        let loc = rest.rsplit_once('(').map(|(_, l)| l.trim_end_matches(')')).unwrap_or(rest);
        // loc = "file:line:col" — split from the right.
        let mut it = loc.rsplitn(3, ':');
        let _col = it.next();
        let line_no = it.next().and_then(|l| l.parse::<u32>().ok()).unwrap_or(0);
        let file = it.next().unwrap_or("").to_string();
        if !file.is_empty() { return (file, line_no); }
    }
    (String::new(), 0)
}

use std::sync::Mutex as StdMutex;
static JS_LIMITER: StdMutex<Option<dedup::RateLimiter>> = StdMutex::new(None);

/// The fatal-JS capture entry (D-2): dedup by (plugin, message, top frame), then envelope
/// (kind=js) → spool. Called from the per-handler TryCatch sites + the promise-reject drain.
pub fn report_js_error(plugin: &str, dispatch: &str, message: &str, stack: &str) {
    let Some(dir) = spool_dir() else { return };
    let (file, line) = parse_top_frame(stack);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sig = dedup::fnv1a64(&[plugin, message, &format!("{}:{}", file, line)]);
    {
        let mut g = match JS_LIMITER.lock() { Ok(g) => g, Err(p) => p.into_inner() };
        let rl = g.get_or_insert_with(dedup::RateLimiter::new);
        if !rl.should_report(sig, now) { return; }
    }
    let mut bc = breadcrumb::snapshot();
    // The current stamp may already have unwound (guard dropped) — restamp the culprit.
    breadcrumb::copy_cstr(&mut bc.plugin, plugin);
    breadcrumb::copy_cstr(&mut bc.dispatch, dispatch);
    let cfg = config::load();
    let env = envelope::render(
        &bc,
        "js",
        envelope::Detail::Js {
            stack: stack.to_string(),
            message: message.to_string(),
            file,
            line,
        },
        Some(envelope::iso8601_utc(now as i64)),
        "", // patched by the uploader (D-1)
        &config::scrub(&cfg),
    );
    if let Ok(json) = serde_json::to_string(&env) {
        let _ = spool::write_incident(&dir, &json);
    }
}
```

Add a unit test at the bottom of `core/src/crash/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn parse_top_frame_variants() {
        assert_eq!(
            super::parse_top_frame("Error: x\n    at doThing (myplugin.js:12:5)\n    at top (a.js:1:1)"),
            ("myplugin.js".to_string(), 12)
        );
        assert_eq!(
            super::parse_top_frame("Error: x\n    at myplugin.js:7:3"),
            ("myplugin.js".to_string(), 7)
        );
        assert_eq!(super::parse_top_frame("no frames here"), (String::new(), 0));
    }
}
```

Run: `cargo test -p s2script-core crash::tests::parse_top_frame_variants` — Expected: `1 passed`.

- [ ] **Step 4: Write the failing integration test (thrown handler → spooled js envelope)**

Append to `core/src/ffi.rs` tests:

```rust
    #[test]
    fn throwing_frame_handler_spools_a_js_incident_once() {
        let d = std::env::temp_dir().join(format!("s2crash-js-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        crate::crash::set_spool_dir(d.to_str().unwrap());
        v8host::create_plugin_context("thrower");
        v8host::eval_in_context(
            "thrower",
            r#"
                const { OnGameFrame } = __s2require("@s2script/frame");
                globalThis._t = OnGameFrame.subscribe(() => { throw new Error("js-boom"); });
            "#,
        )
        .unwrap();
        // Two frames: the second identical throw is deduped (same signature, <60s apart).
        s2script_core_dispatch_game_frame(0, 1, 1, 0);
        s2script_core_dispatch_game_frame(0, 1, 0, 0);
        let items = crate::crash::spool::scan(&d);
        assert_eq!(items.len(), 1, "dedup: one incident for a repeated identical throw");
        let crate::crash::spool::SpoolItem::Envelope(p) = &items[0] else { panic!("expected envelope") };
        let env: crate::crash::envelope::Envelope =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(env.kind, "js");
        assert_eq!(env.breadcrumb.plugin, "thrower");
        match env.detail {
            crate::crash::envelope::Detail::Js { message, stack, .. } => {
                assert!(message.contains("js-boom"));
                assert!(stack.contains("js-boom") || !stack.is_empty());
            }
            other => panic!("wrong detail: {:?}", other),
        }
        crate::crash::set_spool_dir("");
        s2script_core_shutdown();
    }

    #[test]
    fn unhandled_rejection_spools_a_js_incident() {
        let d = std::env::temp_dir().join(format!("s2crash-rej-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        crate::crash::set_spool_dir(d.to_str().unwrap());
        v8host::create_plugin_context("rejector");
        v8host::eval_in_context("rejector", "Promise.reject(new Error('rej-boom'));").unwrap();
        // The drain performs the microtask checkpoint AND flushes pending rejections.
        s2script_core_dispatch_game_frame(1, 1, 0, 1); // Post phase → frame_async_drain
        let items = crate::crash::spool::scan(&d);
        assert_eq!(items.len(), 1);
        let crate::crash::spool::SpoolItem::Envelope(p) = &items[0] else { panic!("expected envelope") };
        let env: crate::crash::envelope::Envelope =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(env.kind, "js");
        match env.detail {
            crate::crash::envelope::Detail::Js { message, .. } => assert!(message.contains("rej-boom")),
            other => panic!("wrong detail: {:?}", other),
        }
        crate::crash::set_spool_dir("");
        s2script_core_shutdown();
    }

    #[test]
    fn handled_rejection_is_not_reported() {
        let d = std::env::temp_dir().join(format!("s2crash-rej2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        crate::crash::set_spool_dir(d.to_str().unwrap());
        v8host::create_plugin_context("handled");
        // .catch attached synchronously — kPromiseHandlerAddedAfterReject cancels the pending report.
        v8host::eval_in_context("handled", "Promise.reject(new Error('nope')).catch(() => {});").unwrap();
        s2script_core_dispatch_game_frame(1, 1, 0, 1);
        assert!(crate::crash::spool::scan(&d).is_empty(), "a handled rejection must not report");
        crate::crash::set_spool_dir("");
        s2script_core_shutdown();
    }
```

Run: `cargo test -p s2script-core throwing_frame_handler`
Expected: FAIL — `assertion failed: ... expected 1 incident, got 0` (nothing reports yet).

- [ ] **Step 5: Instrument the TryCatch sites + register the promise-reject callback**

`core/src/v8host.rs`:

(a) `dispatch_onframe` — replace the `None => Err(())` arm of the `func.call` match (`:8825-8827`) with:

```rust
                // Exception thrown (or otherwise empty): report (kind=js) then count the error.
                None => {
                    let msg = tc.exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "uncaught exception".into());
                    let stack = tc.stack_trace()
                        .map(|s| s.to_rust_string_lossy(&*tc))
                        .unwrap_or_default();
                    crate::crash::report_js_error(
                        owner,
                        if phase == Phase::Pre { "OnGameFrame:pre" } else { "OnGameFrame:post" },
                        &msg,
                        &stack,
                    );
                    Err(())
                }
```

(b) `dispatch_game_event` — extend the existing throw branch (`:6605-6610`); after the `log_warn(...)` line add:

```rust
                let stack = tc.stack_trace()
                    .map(|s| s.to_rust_string_lossy(&*tc))
                    .unwrap_or_default();
                crate::crash::report_js_error(owner, &format!("event:{}", name), &msg, &stack);
```

(c) `load_plugin_js` — after the eval-error `log_warn` (`:8588`) add:

```rust
                    crate::crash::report_js_error(id, "load", &msg, "");
```

and after the onLoad-error `log_warn` (`:8619`) add:

```rust
                            crate::crash::report_js_error(id, "onLoad", &msg, "");
```

(d) Promise rejections. New thread-local next to `RESOLVERS` (`:481`):

```rust
    /// Pending unhandled rejections awaiting end-of-frame confirmation (D-2): promise identity
    /// hash → (message, stack). kPromiseHandlerAddedAfterReject removes its entry; whatever
    /// survives to the frame_async_drain flush is reported. Cleared on shutdown.
    static PENDING_REJECTS: std::cell::RefCell<std::collections::HashMap<i32, (String, String)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
```

The raw callback (place near `s2_current_plugin`):

```rust
/// Isolate-wide promise-reject callback (registered once in `init`). Runs inside V8 while our
/// code is on the stack (during a checkpoint/eval), so a CallbackScope is the ONLY legal scope.
/// Never touches HOST (already borrowed by the caller); only the PENDING_REJECTS map.
unsafe extern "C" fn promise_reject_cb(msg: v8::PromiseRejectMessage) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        use v8::PromiseRejectEvent::*;
        let mut storage = v8::CallbackScope::new(&msg);
        let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let scope = &mut scope;
        let promise = msg.get_promise();
        let id = promise.get_identity_hash().get();
        match msg.get_event() {
            PromiseRejectWithNoHandler => {
                let (text, stack) = match msg.get_value() {
                    Some(v) => {
                        let text = v.to_rust_string_lossy(scope);
                        let stack = v8::Local::<v8::Object>::try_from(v)
                            .ok()
                            .and_then(|o| {
                                let k = v8::String::new(scope, "stack")?;
                                o.get(scope, k.into())
                            })
                            .map(|s| s.to_rust_string_lossy(scope))
                            .unwrap_or_default();
                        (text, stack)
                    }
                    None => ("unhandled rejection".to_string(), String::new()),
                };
                PENDING_REJECTS.with(|m| m.borrow_mut().insert(id, (text, stack)));
            }
            PromiseHandlerAddedAfterReject => {
                PENDING_REJECTS.with(|m| { m.borrow_mut().remove(&id); });
            }
            PromiseRejectAfterResolved | PromiseResolveAfterResolved => {}
        }
    }));
}
```

Register it in `init` right after `isolate.set_microtasks_policy(...)` (`:8652`):

```rust
    isolate.set_promise_reject_callback(promise_reject_cb);
```

Flush at the END of `frame_async_drain` (`:9077`), immediately BEFORE the `periodic_sweep` line Task 3 added:

```rust
    // D-2: whatever unhandled rejections survived the checkpoint are now final — report them.
    let pending: Vec<(String, String)> =
        PENDING_REJECTS.with(|m| m.borrow_mut().drain().map(|(_, v)| v).collect());
    for (message, stack) in pending {
        // Owner attribution for a rejection is best-effort: the rejecting plugin's dispatch has
        // already unwound, so attribute to the breadcrumb's ring-latest plugin.
        let bc = crate::crash::breadcrumb::snapshot();
        let last = (bc.ring_head as usize + crate::crash::breadcrumb::RING_LEN - 1)
            % crate::crash::breadcrumb::RING_LEN;
        let owner = crate::crash::breadcrumb::read_cstr(&bc.ring[last].plugin);
        crate::crash::report_js_error(
            if owner.is_empty() { "unknown" } else { &owner },
            "unhandled-rejection",
            &message,
            &stack,
        );
    }
```

Clear the map in `shutdown` with the other pre-isolate-drop clears (`:8872`):

```rust
    PENDING_REJECTS.with(|m| m.borrow_mut().clear());
```

Note: `PENDING_REJECTS` holds only Strings (no `Global`s), so drop order vs the isolate is not load-bearing — the clear is hygiene. Also note the JS_LIMITER is process-global: tests that need a clean limiter use distinct messages (the tests above do).

(e) `core/src/crash/panic_hook.rs` — replace the `REPORTED` counter check in `report` with the shared limiter (signature = the message + location), keeping the counter as a hard backstop:

```rust
    let sig = crate::crash::dedup::fnv1a64(&["panic", &msg, &loc]);
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    {
        let mut g = match PANIC_LIMITER.lock() { Ok(g) => g, Err(p) => p.into_inner() };
        let rl = g.get_or_insert_with(crate::crash::dedup::RateLimiter::new);
        if !rl.should_report(sig, now_s) { return; }
    }
```

with, at module scope (replacing the `REPORTED`/`MAX_PANICS_PER_BOOT` pair):

```rust
static PANIC_LIMITER: std::sync::Mutex<Option<crate::crash::dedup::RateLimiter>> =
    std::sync::Mutex::new(None);
```

(the existing panic test still passes: first occurrence always reports).

- [ ] **Step 6: Run the integration tests**

Run: `cargo test -p s2script-core -- throwing_frame_handler unhandled_rejection handled_rejection`
Expected: `3 passed`. (If `CallbackScope` pin-init differs in v8 149.4, mirror the exact construction used for `HandleScope` in this file — `ScopeStorage` + `Pin::new_unchecked(...).init()` — the compile error will name the right shape.)

Then: `cargo test -p s2script-core` — all green.

- [ ] **Step 7: Gate suite + commit + submit**

```bash
git add core/src
git commit -m "crash-reporter: fatal JS error path (TryCatch sites + promise-reject callback, dedup)"
gt submit --no-interactive
```

---

### Task 6: Deliberate-crash harness + live-gate validation

**PR boundary:** branch `crash-reporter/crash-harness` — the dev trigger + the end-to-end live proof; PROGRESS entry.

**Files:**
- Create: `examples/crash-test/package.json`, `examples/crash-test/src/plugin.ts`, `examples/crash-test/tsconfig.json`
- Modify: `core/src/v8host.rs` (`crash_test_native` op append + `__s2_crash_test` native), `shim/include/s2script_core.h` (op append), `shim/src/s2script_mm.cpp` (op impl + tail assignment), `docs/PROGRESS.md`
- Test: cargo test for the gate + config gating; the live gate itself (runbook below)

**Interfaces:**
- Consumes: Task 3's `config::load().dev_test`, Task 1's ops-append pattern.
- Produces:
  - `S2EngineOps.crash_test_native: Option<CrashTestNativeFn>` with `pub type CrashTestNativeFn = extern "C" fn(kind: c_int);` (header: `typedef void (*s2_crash_test_native_fn)(int kind);`, appended after `server_build_number`)
  - JS native `__s2_crash_test(kind: string) -> boolean` — kinds `"segv"` (op 0: null volatile write, shim-side), `"abort"` (op 1: `abort()`), `"panic"` (Rust `panic!` in core), `"js"` (synchronous `throw` — done plugin-side, listed for completeness). Returns `false` (refused) unless `crashreporter.json` has `dev_test: true`.

- [ ] **Step 1: Write the failing gating test**

Append to `core/src/ffi.rs` tests:

```rust
    #[test]
    fn crash_test_native_is_gated_by_dev_test_config() {
        assert_eq!(s2script_core_init(Some(test_logger), None, std::ptr::null()), 0);
        v8host::create_plugin_context("harness");
        // No ops table + no dev_test config → every kind refuses (returns false), nothing raised.
        v8host::eval_in_context(
            "harness",
            r#"
                if (__s2_crash_test("segv") !== false) throw new Error("segv must be refused");
                if (__s2_crash_test("abort") !== false) throw new Error("abort must be refused");
                if (__s2_crash_test("panic") !== false) throw new Error("panic must be refused");
                if (__s2_crash_test("bogus") !== false) throw new Error("unknown kind must be refused");
            "#,
        )
        .unwrap();
        s2script_core_shutdown();
    }
```

Run: `cargo test -p s2script-core crash_test_native_is_gated`
Expected: FAIL — `eval error: __s2_crash_test is not defined` surfaces as an `Err` from `eval_in_context`.

- [ ] **Step 2: Implement the native + op**

`core/src/v8host.rs`:

(a) Type alias + ops tail (after `server_build_number`):

```rust
// --- Crash-harness (dev-only): raise a native fault on command (C-ABI; header must match) ---
pub type CrashTestNativeFn = extern "C" fn(kind: c_int);
```

```rust
    // --- Crash-harness — APPENDED after server_build_number; order is the ABI; do not reorder above. ---
    pub crash_test_native: Option<CrashTestNativeFn>,
```

(update the same `S2EngineOps` test literals as Task 1 with `crash_test_native: None,`).

(b) The native (near `s2_crash_set_game`):

```rust
/// Native `__s2_crash_test(kind) -> bool` — the deliberate-crash harness (spec §10). REFUSED
/// (returns false) unless crashreporter.json sets dev_test:true. kinds: "segv"/"abort" raise a
/// real native fault via the shim op; "panic" raises a Rust panic (recovered by catch_unwind,
/// reported by the panic hook); "js" is plugin-side (a plain throw) and unknown kinds refuse.
fn s2_crash_test(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if !crate::crash::config::load().dev_test {
            log_warn("WARN: __s2_crash_test refused (crashreporter.json dev_test is not true)");
            return;
        }
        let kind = args.get(0).to_rust_string_lossy(scope);
        match kind.as_str() {
            "panic" => {
                rv.set_bool(true);
                panic!("deliberate crash-harness panic (sm_crashtest panic)");
            }
            "segv" | "abort" => {
                let Some(f) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.crash_test_native) else {
                    log_warn("WARN: __s2_crash_test: crash_test_native op unavailable");
                    return;
                };
                rv.set_bool(true);
                f(if kind == "segv" { 0 } else { 1 }); // does not return
            }
            _ => {}
        }
    }));
}
```

Register in `install_natives` (next to `__s2_crash_set_game`):

```rust
    set_native(scope, global_obj, "__s2_crash_test", s2_crash_test);
```

`shim/include/s2script_core.h` — after the `s2_server_build_number_fn` typedef:

```c
/* Crash-harness (dev-only, gated core-side by crashreporter.json dev_test): raise a real
 * native fault on command. kind: 0 = null volatile write (SIGSEGV), 1 = abort() (SIGABRT). */
typedef void (*s2_crash_test_native_fn)(int kind);
```

struct tail (after `server_build_number`):

```c
    /* Crash-harness — APPENDED after server_build_number; order is the ABI. */
    s2_crash_test_native_fn crash_test_native;
```

`shim/src/s2script_mm.cpp` — the op (next to `s2_server_build_number`):

```cpp
// Crash-harness op (spec §10): a REAL fault in shim code, so the live gate exercises the exact
// Breakpad path a production crash takes. Only reachable through the dev_test-gated core native.
static void s2_crash_test_native(int kind) {
    if (kind == 1) abort();
    volatile int* p = nullptr;
    *p = 42; // SIGSEGV
}
```

and the tail assignment (after `ops.server_build_number`):

```cpp
    ops.crash_test_native = &s2_crash_test_native;
```

- [ ] **Step 3: Run the gating test**

Run: `cargo test -p s2script-core crash_test_native_is_gated`
Expected: PASS. Full suite: `cargo test -p s2script-core` — all green.

- [ ] **Step 4: Commit, then the harness plugin**

```bash
git add core/src shim/include shim/src
gt create crash-reporter/crash-harness -m "crash-reporter: dev-gated deliberate-crash natives (segv/abort/panic)"
```

`examples/crash-test/package.json`:

```json
{
  "name": "@demo/crash-test",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "s2script": {
    "apiVersion": "1.x"
  }
}
```

`examples/crash-test/tsconfig.json` (copy `examples/changeteam-demo/tsconfig.json` verbatim — same compiler surface).

`examples/crash-test/src/plugin.ts`:

```ts
// @demo/crash-test — the deliberate-crash harness (crash-reporter spec §10). DEV-ONLY:
// every native kind is refused by core unless configs/crashreporter.json sets dev_test:true.
//
//   sm_crashtest segv   — real SIGSEGV in the shim → Breakpad minidump + .s2meta, server dies
//   sm_crashtest abort  — SIGABRT, same path
//   sm_crashtest panic  — Rust panic: recovered by catch_unwind, REPORTED by the panic hook
//   sm_crashtest js     — synchronous JS throw from this handler (kind=js incident)
//   sm_crashtest reject — unhandled promise rejection (kind=js incident, "unhandled-rejection")
import { Commands } from "@s2script/sdk/commands";

declare function __s2_crash_test(kind: string): boolean;

export function onLoad(): void {
  Commands.registerServer("crashtest", (ctx) => {
    const kind = ctx.args[0] ?? "";
    console.log(`[crash-test] sm_crashtest ${kind}`);
    if (kind === "js") {
      throw new Error("deliberate crash-harness js throw (sm_crashtest js)");
    }
    if (kind === "reject") {
      Promise.reject(new Error("deliberate crash-harness rejection (sm_crashtest reject)"));
      ctx.reply("crash-test: rejection queued (reported at end of frame)");
      return;
    }
    if (kind === "segv" || kind === "abort" || kind === "panic") {
      const armed = __s2_crash_test(kind);
      ctx.reply(`crash-test: ${kind} ${armed ? "raised" : "REFUSED (set dev_test:true in configs/crashreporter.json)"}`);
      return;
    }
    ctx.reply("usage: sm_crashtest <segv|abort|panic|js|reject>");
  });
  console.log("[crash-test] armed: sm_crashtest <segv|abort|panic|js|reject>");
}
```

(Before finalizing, mirror the exact `Commands` API from `examples/changeteam-demo/src/plugin.ts` — if `registerServer` does not exist in the SDK `.d.ts`, use `Commands.register` as that file does; the typecheck gate decides. The bare `declare function __s2_crash_test` keeps the example compiling against the ambient stubs.)

- [ ] **Step 5: Typecheck + build the example**

```bash
./scripts/check-plugins-typecheck.sh
```
Expected: green including `examples/crash-test` (fix the `Commands` surface if the gate names a mismatch).

- [ ] **Step 6: Live gate (Docker CS2), full runbook**

```bash
make all                                   # host build for local iteration…
# …but DEPLOY the sniper build (host glibc will not load):
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
# Enable the harness + reporter (mock endpoint on the docker host):
cat > dist/addons/s2script/configs/crashreporter.json <<'EOF'
{ "enabled": true, "endpoint": "http://host.docker.internal:8787/ingest",
  "api_key": "dev", "include_minidump": true, "dev_test": true }
EOF
# Mock ingest endpoint (separate terminal): accept POSTs, print sizes, return 200.
python3 - <<'EOF' &
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = self.rfile.read(n)
        print(f"[ingest] {self.path} {n} bytes; envelope? {b'schema_version' in body}", flush=True)
        self.send_response(200); self.end_headers()
HTTPServer(("0.0.0.0", 8787), H).serve_forever()
EOF
# Build + deploy the harness plugin:
cd examples/crash-test && npx s2script build && cd ../..
cp examples/crash-test/dist/_demo_crash-test.s2sp dist/addons/s2script/plugins/
make docker-test
docker exec s2script-cs2 /patch-gameinfo.sh
docker compose -f docker/docker-compose.yml restart cs2   # NOT --force-recreate
```

Validation sequence (wait for the boot window per the live-gate memory, then):

```bash
python3 scripts/rcon.py "sm_crashtest js"       # → a kind=js incident in data/crashes
python3 scripts/rcon.py "sm_crashtest reject"   # → kind=js, dispatch=unhandled-rejection
python3 scripts/rcon.py "sm_crashtest panic"    # → server SURVIVES; kind=panic incident
docker exec s2script-cs2 ls /home/steam/cs2-dedicated/game/csgo/addons/s2script/data/crashes
# expect: <uuid>.json files; within ≤5 min (periodic sweep) they move to sent/ and the mock
# endpoint prints "[ingest] ... envelope? True" lines.
python3 scripts/rcon.py "sm_crashtest segv"     # → the server process DIES (chained SIGSEGV)
docker exec s2script-cs2 ls /home/steam/cs2-dedicated/game/csgo/addons/s2script/data/crashes
# expect: one <uuid>.dmp + <uuid>.dmp.s2meta pair
docker compose -f docker/docker-compose.yml restart cs2   # next boot
# expect on boot: "[s2script] crash handler armed"; the boot sweep uploads the native pair
# (mock endpoint prints a multipart POST with the minidump); files move to sent/.
```

Also confirm (spec §12): normal boot unregressed (no new WARNs beyond the crash-reporter banner when unconfigured), and with `crashreporter.json` absent nothing uploads (opt-in default). Verify the breadcrumb names the culprit: `docker exec s2script-cs2 cat .../sent/<uuid>.json | python3 -m json.tool | grep -A2 breadcrumb` shows `"plugin": "_demo_crash-test"` (or the loader's id for the harness) and `"dispatch": "command:crashtest"`-adjacent context (the ring's latest entries).

Record every observed output in the PR body's live-gate section.

- [ ] **Step 7: PROGRESS entry + final commit + submit**

Append a finished-slice entry to `docs/PROGRESS.md` (follow the existing entry format: slice name, what was built per PR, the live-gate transcript summary, deferred items — out-of-process Breakpad handler; per-op `note_engine_op` breadth; backend spec #2 + symbol pipeline spec #3 as the next cycles).

```bash
git add examples/crash-test docs/PROGRESS.md
git commit -m "crash-reporter: deliberate-crash harness plugin + live-gate validation"
gt submit --no-interactive
```

---

## Recommended model tiers per task (for the orchestrator)

| Task | Branch | Tier | Why |
|---|---|---|---|
| 1 | `crash-reporter/breadcrumb` | Sonnet | Mechanical POD + established ops-append pattern (many exact edit sites given) |
| 2 | `crash-reporter/panic-path` | Sonnet | Pure-Rust serde/fs; contract is fully specified |
| 3 | `crash-reporter/spool-uploader` | Sonnet | Mirrors the existing http.rs test/server patterns |
| 4 | `crash-reporter/breakpad` | Opus | Signal-safety, vendoring, glibc-2.31 linking — the riskiest slice |
| 5 | `crash-reporter/js-errors` | Opus | Raw V8 CallbackScope wiring + reject-event semantics |
| 6 | `crash-reporter/crash-harness` | Sonnet (Opus for the live gate if faults misbehave) | Mostly plumbing + runbook |

## Self-Review

**1. Spec coverage.**
- §6.1 breadcrumb (identity, current context, ring 16, plugin table, main-thread writes, torn-read tolerance) → Task 1. The spec's "best-effort JS file:line stamped cheaply on JS entry" → Task 1's `note_js_location` via `Function::get_script_line_number` (verified present in v8 149.4) plus the precise `file:line` on the error path (Task 5).
- §6.2 native fault (vendor Breakpad, `ExceptionHandler` at boot after core init, sigaltstack, chaining, minidump + `.s2meta` single-write sidecar, in-process) → Task 4. Out-of-process explicitly deferred (PROGRESS entry, Task 6 Step 7).
- §6.3 fatal JS (uncaught + unhandled rejection, rate-limit/dedup, orthogonal to degrade-per-descriptor) → Task 5, decision D-2.
- §6.4 Rust panic (set_hook, reported-not-swallowed, catch_unwind untouched) → Task 2.
- §6.5 envelope (frozen schema_version 1, per-kind detail, server_id, multipart minidump ref) → Task 2 (`envelope.rs` reproduces every field; round-trip tested), multipart in Task 3.
- §6.6 spool + upload-on-next-boot (+ periodic sweep for live kinds, retry/backoff, sent/) → Task 3.
- §6.7 config (enabled default false, endpoint, api_key, include_minidump, privacy toggles; operator-configured, not plugin-permissioned) → Task 3 (`crashreporter.json` via the existing `config_read` op, so it lives in `addons/s2script/configs/`).
- §9 degrade-safety → the golden rule in Global Constraints; every init path fail-off; Task 4 Step 5 is the signal-safety audit.
- §10 testing strategy → deliberate-crash harness (Task 6), signal-safety audit (Task 4), cargo unit tests (Tasks 1–3, 5), live gate (Task 6).
- §11 slices → the six tasks map 1:1. §12 success criteria → all exercised in Task 6 Step 6.
- §13 open questions → D-1..D-7 at the top.
- Gap check: `game.players`/`uptime` fields (§6.5) → Task 1 client-event tracking + `note_tick`; `gamedata.stale` → shim `s_gdFail` via `set_identity`. Covered.

**2. Placeholder scan.** No TBD/TODO/"similar to Task N" remain. Two deliberately-flagged verification points are instructions, not placeholders: Task 6's `Commands.registerServer`-vs-`register` check (the typecheck gate arbitrates; both code paths shown), and Task 5's note that the `CallbackScope` pin-init shape must mirror this file's existing scope construction if the compile error names a different one. Task 1 Step 8's `CrashSpoolDir` dirname-walk depth carries the same read-the-neighbor instruction with the concrete default shown.

**3. Type consistency.** Verified across tasks: `CrashBreadcrumb` field names used by `envelope::render` (Task 2) and `uploader::read_meta` (Task 3) match Task 1's struct; `Detail` variant shapes used in Tasks 2/3/5/6 match Task 2's enum; `CrashConfig` fields used in Tasks 3/5/6 match Task 3; `SpoolItem` shapes produced by Task 4's file naming (`<uuid>.dmp` + `<uuid>.dmp.s2meta`) match `spool::scan`'s suffix logic (`.json` / `.dmp` / `.dmp.s2meta`); the C ABI names (`s2script_core_crash_breadcrumb`, `_size`, `_set_identity` 6-arg form) are identical in `ffi.rs`, the header, and both shim call sites; `server_build_number` / `crash_test_native` append in the same order in the Rust struct, the C header, and the shim assignment tail. One fix applied during review: the panic-hook test in Task 2 asserts a single spool item — it sets a fresh per-test spool dir and clears it after, so Task 5's shared-limiter change (first occurrence always reports) keeps it green.

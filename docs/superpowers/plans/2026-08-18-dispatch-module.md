# Deep Dispatch module — implementation plan

Spec: `docs/superpowers/specs/2026-08-18-dispatch-module-design.md`.

One atomic core refactor. No changeset. Do not start the Jobs spine in this slice.

## Global constraints

- Behavior-preserving. Empty snapshot, nest-before-host, Busy=`Deferred`, Absent=`Delivered`, collapsing skip, host-path-only `AFTER_HANDLER` stay exact.
- Narrow isolate adapter only. Never expose `HOST` / `PLUGINS` / `REGISTRY`.
- Retarget imports; do not re-export Dispatch through `v8host`.
- `nest.rs` stays the outbound TLS adapter.
- Hook views, `ACTIVE_HOOK`, acquire, damage/usercmd/output construction, `dispatch_onframe`, TopMenu, inter-plugin emit stay in `v8host`.
- Update `ARCHITECTURE.md` only if it names `fan_out`'s old home. Do not edit `CLAUDE.md`.
- Do not run `make ci-native` or the live CS2 gate in this slice.

---

### Task 1 — extract the module

**Files:** create `core/src/dispatch.rs`; modify `core/src/lib.rs`, `core/src/v8host.rs`.

- [x] Add `mod dispatch`.
- [x] Move `Instrument`, `StopAt`, `Delivery`, `fan_out`, `fan_out_collapsing`, `fan_out_inner`, nested `CallbackScope`, both subscriber walks, `AFTER_HANDLER`, and `set_after_handler`.
- [x] Add `HostAccess`, `with_host_isolate`, `owner_is_live`, `clone_plugin_context` in `v8host`.
- [x] Consult `nest::top()` before `with_host_isolate`.
- [x] Invoke `AFTER_HANDLER` only on the host-borrow walk.
- [x] `dispatch_hook` save/restores the after-handler through `set_after_handler`.

### Task 2 — retarget callers

**Files:** `events.rs`, `client.rs`, `commands.rs`, `usermsg.rs`, `entity.rs`, `cookies.rs`, `ws.rs`, `net.rs`, `ffi.rs`.

- [x] Import Dispatch types from `crate::dispatch`.
- [x] Leave host helpers (`set_native`, `subscribe_into`, `engine_ops`, …) on `crate::v8host`.
- [x] No `pub use` of Dispatch from `v8host`.

### Task 3 — docs

- [x] Spec + this plan dated 2026-08-18.
- [x] Concise `docs/PROGRESS.md` completion entry.
- [x] `ARCHITECTURE.md` left unchanged (it does not name `fan_out`'s old file).

### Task 4 — verify

- [x] `rustfmt` on touched Rust files that need it.
- [x] Focused reentry / deferred / collapse tests.
- [x] `cargo test -p s2script-core`.

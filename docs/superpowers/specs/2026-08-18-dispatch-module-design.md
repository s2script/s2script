# Deep Dispatch module — design spec

**Status:** Implemented (2026-08-18). Behavior-preserving extraction.
**Audience:** core maintainers.
**Builds on:** isolate re-entry (2026-08-14); deferred-dispatch queue (`Delivery`); pickup-gate `AFTER_HANDLER`; A4 fan-out (`Instrument` / `StopAt`).
**Slice:** 1 of 2 in the Dispatch + Jobs deepening. Jobs is a later, independent PR.

---

## 1. Why

The fan-out invocation policy — nest vs host-borrow, Busy vs Absent, per-subscriber liveness, TryCatch isolation, HookResult collapse, `Delivery` — lived in `v8host.rs` beside isolate lifecycle, hook views, acquire folding, and async drain. Moving it to `core/src/dispatch.rs` makes that policy one module without changing any call-site contract.

This is not the 2026-07-30 audit's A4 "generic dispatcher + keyed channels" rewrite. No subscribe/FFI unification, no priority/auto-disable, no test-harness move.

## 2. What moved

`core/src/dispatch.rs` owns:

- `Instrument`, `StopAt`, `Delivery` (`#[must_use]`).
- `fan_out`, `fan_out_collapsing`, `fan_out_inner`.
- Nested `CallbackScope` path and both private subscriber walks.
- `AFTER_HANDLER` and the crate-private `set_after_handler` used by acquire folding.

Callers import from `crate::dispatch`. Nothing is re-exported through `v8host`.

## 3. What stayed

In `v8host.rs`: hook views, `ACTIVE_HOOK`, acquire sessions, damage/usercmd/output argument construction, `dispatch_onframe`, TopMenu, inter-plugin emit, and the isolate/lifecycle tables (`HOST` / `PLUGINS` / `REGISTRY`).

`core/src/nest.rs` remains the outbound TLS adapter. Dispatch reads `nest::top()` and does not own the stack.

Feature modules keep their mux, native, dispatch wrapper, and teardown.

## 4. Isolate adapter (the only new seam)

Dispatch never names `HOST`, `PLUGINS`, or `REGISTRY`. `v8host` exposes four crate-private adapters:

| Adapter | Role |
|---|---|
| `HostAccess::{Busy, Absent}` | Distinguishes a live borrow from an uninitialized host |
| `with_host_isolate` | Runs a closure on `&mut OwnedIsolate`, or returns `HostAccess` |
| `owner_is_live` | Generation-gated liveness |
| `clone_plugin_context` | Clones a plugin `Global<Context>` so the handler may re-enter the plugin table |

`with_host_isolate` is taken only after the nest-token path. A non-null `nest::top()` uses `CallbackScope` and never borrows the host.

## 5. Semantics that must not drift

- Empty snapshot → `Delivered` without borrowing the host.
- Non-null nest token → `CallbackScope`; never `HOST`.
- Busy engine-inbound notify (`HostAccess::Busy`, `#63`) → `Deferred`. Collapsing pre-hooks still skip with `Continue` and are not replayed (`fan_out_collapsing` discards `Delivery`).
- Missing host (`HostAccess::Absent`) → `Delivered`, not `Deferred`.
- HookResult collapse, `StopAt::Handled`, TryCatch isolation, breadcrumbs, liveness checks, same-hook skip: unchanged.
- `AFTER_HANDLER` is **host-path-only**. The nested CallbackScope walk never invokes it. Acquire folding save/restores via `set_after_handler`.

## 6. Proof

Existing in-isolate tests stay in `v8host::frame_tests` and drive Dispatch through real contexts. Focused reentry / deferred / collapse tests plus `cargo test -p s2script-core` are the gate for this slice. Live CS2 / sniper rebuild is a parent follow-up, not this extraction.

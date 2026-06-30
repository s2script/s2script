# Slice 1 — One Multiplexed Hook, Full Contract — Design Spec

- **Project:** s2script (TypeScript plugin framework for Source 2; SourceMod's spiritual successor)
- **Date:** 2026-06-30
- **Status:** Approved design, ready for implementation planning
- **Builds on:** Slice 0 (merged to `main`) — `core` Rust cdylib with V8 + 3-fn C ABI, C++ Metamod shim, sniper build, Docker live-gate harness.
- **Scope:** Slice 1 only — the multiplexer machinery bound to exactly one engine touchpoint (`OnGameFrame`). See `docs/ARCHITECTURE.md` §2.2.

---

## 1. Purpose & what it proves

Build the **real multiplexer/descriptor machinery once, generically**, and bind it to one engine touchpoint — `OnGameFrame` — so every later hook is `+1 descriptor`. Prove two handlers **compose** under the full `HookResult` contract (priority ladder, Pre/Post, collapse, short-circuit). This is the thesis of the whole framework (the "sole arbiter" composition model); it must be built properly, not shortcut into "just call a callback."

## 2. Decided directions

1. **SourceMod model (confirmed).** SourceMod shipped one core binary per engine branch and expressed all game-specifics as runtime **data** (gamedata) + author **packages** (includes/extensions), never per-game compiled binaries. Applied here: `OnGameFrame` is `ISource2Server::GameFrame`, which every Source 2 game has — by the doc's own litmus test it is **engine-generic → lives in `core`**. The single `core` cdylib is **unchanged** (no host/per-game-cdylib refactor); `games/cs2` stays empty; the `core ↛ games/*` CI guard stays. The real core/game-**package** boundary is proven later at Slice 3 (`pawn.health`, a generated-JS schema accessor in the `@s2script/cs2` JS package).
2. **Authoring DX target (confirmed): named-import registration calls + auto-ledger.** `import { onGameFrame } from "@s2script/events"; onGameFrame(handler, opts?)` — no chaining, tree-shakeable, typed payload, `HookResult | void` return. Subscriptions auto-tie to the plugin and auto-remove on unload; a disposable is returned only for dynamic teardown. **This is the Slice 5 std-lib target** delivered via the bundler (Slice 4) + the events package (Slice 5). Slice 1 implements the *shape* over a flat native primitive (see §5); the auto-ledger-to-plugin behavior arrives with plugin lifecycle (Slice 4).
3. **gamedata-cwd fix folded in.** Resolve the gamedata path relative to the plugin `.so` (via `dladdr`), not cwd.

## 3. The multiplexer (engine-generic `core`, pure Rust) — the heart

A new `core/src/multiplexer.rs`, **zero V8 and zero engine dependencies**, so the entire thing is `cargo test`-able without a server. This is where most of the slice's proof lives.

**Types:**
- `HookResult` — `Continue < Changed < Handled < Stop` (ordered by precedence).
- `Priority` — `High < Normal < Low < Monitor`.
- `Phase` — `Pre`, `Post`.
- `Descriptor` — a named touchpoint holding ordered `Subscription`s. `OnGameFrame` is one instance; the registry is keyed by name so a second hook is `+1 descriptor`.
- `Subscription { id, priority, phase, handler, enabled, error_count }`.

**Semantics (from §2.2):**
- **Ordering:** within a phase, subscriptions run by priority tier `High → Normal → Low → Monitor`; within a tier, registration order (stable).
- **Collapse:** the chain's result is the **max by precedence**. `Stop` **short-circuits** (remaining non-Monitor handlers in that phase are skipped). `Handled` does **not** short-circuit. `Monitor` handlers always run **after** the collapse is decided; they receive the collapsed result but their **return is ignored**.
- **Re-entrancy safety:** dispatch iterates a **snapshot** of subscription ids; before invoking each, it re-checks the subscription is still present and `enabled`. So a handler that subscribes/unsubscribes mid-dispatch cannot corrupt iteration — new subs take effect next dispatch, an unsubscribe of a not-yet-called handler is honored.
- **Error isolation:** handler invocation is fallible; a handler that errors is logged with its id, treated as `Continue`, and its `error_count` incremented. On reaching a threshold (`MAX_HANDLER_ERRORS = 10`) the subscription is **auto-disabled** (`enabled = false`) with a named log reason.
- **Lazy detour:** the descriptor tracks its count of `enabled` subscriptions (across both phases). On `0 → 1` it calls the host to **install** the detour; on `1 → 0` (including via auto-disable/unsubscribe) it calls the host to **remove** it.

**Testability seam:** handler invocation is abstracted behind a trait (e.g. `trait HandlerInvoker { fn invoke(&self, sub_id, ctx) -> Result<HookResult, HandlerError> }`). Unit tests supply a mock invoker (Rust closures returning scripted `HookResult`s / errors); the V8 layer (§5) supplies the real JS-invoking implementation. The detour install/remove is likewise a trait/callback so it's mockable.

## 4. `OnGameFrame` descriptor + dispatch flow

`OnGameFrame` is registered in `core` as the first descriptor instance. Its context payload is `{ simulating: bool, firstTick: bool, lastTick: bool, phase: Pre|Post }`.

**Flow:**
1. JS calls the native subscribe primitive → multiplexer records the subscription; on the first enabled subscriber, the multiplexer asks the shim (via the `request_hook` C-ABI callback) to install the SourceHook detour.
2. The engine calls `GameFrame`; the shim's SourceHook **pre** handler calls `s2script_core_dispatch_game_frame(Pre, …)`, the original runs, then the **post** handler calls `…(Post, …)`.
3. `core` dispatches the matching phase's subscriptions in priority order, collapses, and returns the collapsed `HookResult` (informational for this void hook).

**`resultApply` is trivial for `OnGameFrame` (by design).** `GameFrame` returns void; we never suppress the frame (that would freeze the game). So `Stop`/`Handled` control the **handler chain** (which handlers run), not the engine. The full collapse contract is still *proven* through observable chain behavior (a `Stop` at `High` skips a `Low` handler; `Handled` does not; `Monitor` runs after). The four `resultApply` suppression mechanisms (supercede/neutralize/override/out-param) are **Slice 5**, not here.

## 5. Native primitive + provisional surface (the V8 layer)

Extend `core/src/v8host.rs` (same pattern as the existing `console` install):

- **Native primitive (internal, low-level, flat):** the host installs a small internal API the std lib wraps — conceptually `subscribe(eventName: string, handler: fn, opts) -> id` and `unsubscribe(id)`. `subscribe` stores the handler as a `v8::Global<Function>` and registers it with the multiplexer. This is plumbing, not the authoring API.
- **Provisional Slice-1 surface:** a tiny built-in JS prelude (injected at context creation) defines the target *shape* over the primitive so the demo/tests read like the eventual import: a global `onGameFrame(handler, opts?)` returning a disposable `{ dispose() }`, plus `HookResult` / `Priority` / `Phase` constants. Explicitly a **placeholder** for the future `import { onGameFrame } from "@s2script/events"` — same call signature, different acquisition (global now; bundled import in Slice 4/5).
- **V8 handler-invoker:** the multiplexer's real `HandlerInvoker` enters the context, builds the `ctx` object, calls the stored `Global<Function>` under `TryCatch`, and maps the return to `HookResult` (`undefined`/no-return → `Continue`; out-of-range → `Continue` + a warning). A thrown JS exception becomes a `HandlerError` (feeding §3 error isolation).
- **Lifetime in Slice 1:** subscriptions are **process-global** (single shared context, no plugin identity yet). `dispose()` removes one; `s2script_core_shutdown` clears all. **Auto-ledger-to-plugin + auto-remove-on-unload is the Slice 4/5 behavior** this shape is designed to slot into.

## 6. C ABI + shim detour (the engine-coupled, live-gate part)

The ABI stays tiny — `init` gains one callback, plus one dispatch entry:

```c
typedef void (*s2_log_fn)(int level, const char* utf8_msg);
typedef void (*s2_hook_request_fn)(const char* descriptor, int enable); /* core → shim: install(1)/remove(0) */

int  s2script_core_init(s2_log_fn logger, s2_hook_request_fn request_hook);
int  s2script_core_eval(const char* utf8_js);
int  s2script_core_dispatch_game_frame(int phase, int simulating, int first, int last); /* shim → core; returns collapsed HookResult */
void s2script_core_shutdown(void);
```

- Every new FFI entry stays inside the existing `catch_unwind` discipline (no panic across the boundary).
- The **shim** implements `request_hook`: on `"OnGameFrame"` enable it `SH_ADD_HOOK`s `ISource2Server::GameFrame` (pre + post) via SourceHook (metamod-provided; `g_SHPtr`); on disable it `SH_REMOVE_HOOK`s. The pre/post handlers call `s2script_core_dispatch_game_frame`.
- **No new gamedata** — it's a vtable hook on the already-acquired `ISource2Server`, not a sig-scan. The exact `SH_DECL_HOOK` signature for `GameFrame(bool, bool, bool)` and the interface method are confirmed against hl2sdk `cs2` + CounterStrikeSharp (the same "confirm against the pinned headers" discipline as Slice 0's interface acquisition).

## 7. gamedata-cwd fix (folded in)

Replace the cwd-relative gamedata path in the shim with one resolved relative to the plugin's own `.so` via `dladdr` on a shim symbol: `<so_dir>/../../gamedata/core.gamedata.jsonc` (the `.so` lives at `addons/s2script/bin/linuxsteamrt64/`, gamedata at `addons/s2script/gamedata/`). This makes interface acquisition work regardless of the server's cwd (`game/bin/linuxsteamrt64/`). Re-verified by the live gate: the `interface OK:` lines must appear with no manual gamedata placement.

## 8. Testing strategy

- **Unit (`cargo test`, no V8 / no engine) — the bulk of the proof.** The multiplexer with mock invokers: priority ordering across tiers; collapse = max; `Stop` short-circuits, `Handled` does not; `Monitor` runs after + return ignored; Pre/Post separation; snapshot re-entrancy (subscribe & unsubscribe mid-dispatch); error isolation + auto-disable at threshold; lazy install/remove counting (`0→1` installs, `1→0` removes, auto-disable of the last handler removes).
- **Integration (`cargo test` + V8).** `eval` a script that registers two handlers at different priorities returning different `HookResult`s; call `s2script_core_dispatch_game_frame` directly (no engine); assert ordering, collapse, console output, the typed `ctx`, and that a throwing JS handler is isolated (logged, treated as `Continue`, eventually auto-disabled). Run with `--test-threads=1` (V8 platform is process-global).
- **Live (sniper build + Docker, operator-run).** The real `GameFrame` detour fires each tick; two handlers compose live (e.g. a per-N-ticks log from each, and a `Stop` at `High` demonstrably skipping a `Low` handler); subscribe/unsubscribe at runtime via RCON-driven `eval`. Also re-verifies §7 (interface acquisition now works without manual gamedata placement). Reuses `scripts/build-sniper.sh`, the Docker harness, and `scripts/rcon.py`.

## 9. Acceptance criteria

1. `core` builds (sniper) with the multiplexer; `cargo test` (the multiplexer + V8-integration suites) passes; `make check-boundary` stays green.
2. The C ABI grows to exactly the §6 surface; no panic crosses the FFI boundary.
3. On a live CS2 server: the `OnGameFrame` detour installs on first subscription and the dispatch fires each tick.
4. **Two handlers compose live** under the full contract: priority order observed; a `Stop` at `High` skips a lower-priority handler in the same phase; `Monitor` runs after and its return is ignored; Pre and Post both fire around the original.
5. Re-entrancy and error isolation hold: a handler that subscribes/unsubscribes mid-dispatch doesn't corrupt the run; a throwing handler is isolated and auto-disabled after the threshold, the server never crashes.
6. The detour is **removed** when the last subscription is gone (lazy lifecycle), confirmed live.
7. gamedata-cwd fix: interface acquisition logs `interface OK:` lines on the live server with no manual gamedata placement.
8. Reproduces from the README (sniper build + Docker runbook + RCON checks).

## 10. Out of scope (Slice 1)

`resultApply` suppression breadth (supercede/neutralize/override/out-param — Slice 5; `OnGameFrame`'s apply is trivial/chain-control only); the handle/`EntityRef` system (Slice 5); plugin identity, context-per-plugin, the ledger and auto-ledger-on-unload (Slice 4 — subscriptions are process-global this slice); the TS bundler / `import { … }` resolution and the `@s2script/events` package (Slice 4 build + Slice 5 std lib); additional descriptors beyond `OnGameFrame` (built generic so each later hook is `+1`); commands/chat specialized multiplexers; schema/entities; SchemaSystem module-factory wiring (still deferred). Note later needs as TODOs and stop.

## 11. File structure / deliverables

- `core/src/multiplexer.rs` (new) — the generic machinery + unit tests.
- `core/src/v8host.rs` (modify) — install the native subscribe primitive + provisional `onGameFrame`/`HookResult` prelude; the V8 `HandlerInvoker`.
- `core/src/ffi.rs` (modify) — `init` gains `request_hook`; add `s2script_core_dispatch_game_frame`; keep `catch_unwind` on all entries.
- `core/src/lib.rs` (modify) — `mod multiplexer;`.
- `shim/include/s2script_core.h` (modify) — the §6 ABI.
- `shim/src/s2script_mm.{h,cpp}` (modify) — SourceHook detour on `ISource2Server::GameFrame`, the `request_hook` callback, dispatch calls, and the `dladdr` gamedata-path fix.
- `games/cs2/` unchanged (empty); CI boundary guard unchanged.
- README (modify) — add the OnGameFrame demo to the live runbook; note the multiplexer + the provisional surface.
- Sniper build + Docker live gate + `scripts/rcon.py` reused.

## 12. Open items to validate during implementation

- The exact `ISource2Server::GameFrame` method, its vtable position, and the `SH_DECL_HOOK3_void` signature (3 bools) — confirmed against hl2sdk `cs2` + CounterStrikeSharp; SourceHook init (`g_SHPtr`, `SH_ADD_HOOK`) wired in the shim.
- Whether `GameFrame`'s SourceHook pre/post both reliably fire and the param shape (`simulating, bFirstTick, bLastTick`) matches across CS2 updates.
- The provisional prelude injection point (built-in script run once per context at init) and how it coexists with the existing `console` install.

## 13. DX / future authoring model (recorded target for Slices 4–5)

The chosen authoring DX, to be delivered by the bundler (Slice 4) + the events/std package (Slice 5), and which Slice 1's surface mirrors:

```ts
// src/lib/events/combat.ts — organized across files, SvelteKit-style
import { onGameFrame } from "@s2script/events";   // engine-generic events
import { onPlayerDeath } from "@s2script/cs2";    // game events from the game package

onGameFrame((frame) => { /* frame.simulating / firstTick / lastTick, typed */ });
onGameFrame((frame) => { if (cheat) return HookResult.Stop; }, { priority: "high" });
onPlayerDeath((e) => log(`${e.victim} died`));

const sub = onGameFrame(onTick);   // disposable only for dynamic teardown
sub.dispose();
```

Properties to preserve: per-event **named imports** (no chaining, tree-shakeable, autocomplete on available events); handler return `HookResult | void` (void = `Continue`); **auto-ledgered** registration (auto-removed on unload via §2.5); typed payloads; engine-generic events from `@s2script/events`, game events from `@s2script/cs2`. The native primitive (§5) is deliberately flat so this wrapper stays thin and the DX can evolve over a stable native layer, governed by `apiVersion`/semver.

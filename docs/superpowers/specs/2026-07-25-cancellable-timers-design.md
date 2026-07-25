# Cancellable timers — design

**Status:** Implemented and live-gated (2026-07-25).
**Audience:** plugin authors; core maintainers.
**Builds on:** the existing `TimerQueue`/`RESOLVERS`/ledger machinery in `core/src/v8host.rs`.

---

## 1. Goal

SourceMod's `CreateTimer` / `KillTimer`, ModSharp's timer handles: a timer you can **cancel**, and
one that **repeats**.

## 2. What existed

`@s2script/sdk/timers` had `delay`, `nextTick`, `nextFrame`, `threadSleep` — **all Promise-returning
and none cancellable**. A Promise is the wrong shape for cancellation: to "cancel" one you must
leave it forever unresolved, which leaks the continuation.

Underneath, though, most of the machinery was already right: `TimerQueue::remove(id)` exists (the
ledger teardown already uses it), timers are already ledgered via `record_timer`, and
`resolve_or_drop` already demonstrates the owner-liveness guard for firing into a plugin context.

## 3. Shape: a handle, not a Promise

```ts
import { after, every } from "@s2script/sdk/timers";
const t = every(1000, () => console.log("tick"));
t.alive;    // false once fired (one-shot), killed, or its plugin unloaded
t.kill();   // idempotent; safe from inside the callback
```

Free functions, not `ctx.timers` — plugin registration is load-window-only, but `CreateTimer` is
called at *runtime* from commands and event handlers, exactly like the existing `delay`.

A handle rather than a bare id: a bare number invites arithmetic on it and gives `alive` nowhere to
live.

## 4. Core

`TIMER_CBS: HashMap<u64, TimerCallback>` alongside `RESOLVERS`, sharing the same async-id space so
ledger teardown reaches queue, resolver and callback by one id. `TimerCallback` holds the owner tag,
the `Global<Function>`, and `interval_ms: Option<u64>` (`Some` ⇒ repeating).

The drain checks `TIMER_CBS` before `RESOLVERS`, fires via `fire_timer_cb` — which mirrors
`resolve_or_drop`'s liveness guard exactly, because firing into a disposed context is the
use-after-free this codebase keeps re-learning — and re-arms by pushing a fresh `Deadline`.
`TimerKind` is unchanged; repeat is re-arming, not a new kind.

## 5. Three things that are easy to get wrong

**A throwing callback must not kill the timer system.** `fire_timer_cb` catches, logs with the
owning plugin's id, and carries on; a repeater keeps repeating. Same per-handler containment the
multiplexer uses.

**A self-kill must not re-arm.** The drain removes the entry *before* firing, so a callback that
calls its own `kill()` finds nothing to remove — meaning "is it still in `TIMER_CBS`?" answers
*false* either way and cannot detect the self-kill. The first implementation used exactly that check
and a self-killing repeater fired forever; the test caught it. `__s2_timer_kill` therefore records
the id in `TIMER_KILLED` when both the map and queue lookups miss (the only way that happens for a
live id is a mid-fire self-kill), and the drain consults it before re-arming. Bounded by that
condition, cleared by teardown and shutdown.

**Unload must kill a repeater.** A repeating timer re-arms itself, so without the ledger dropping
`TIMER_CBS` it would fire into a dead context forever. `Resource::Timer(tid)` now clears queue,
resolver, callback and kill-record. The ledger is the teardown authority precisely so this does not
depend on the plugin's own cleanup running.

## 6. `every(0, …)` throws

A zero-interval repeat re-arms every drain and starves the frame. Refused loudly (`RangeError`)
rather than silently clamped. `after(0, …)` is fine — it fires on the next drain.

## 7. Testing

Eight core tests: one-shot fires exactly once then reports dead; `every` re-arms across drains;
`kill` stops a repeater and is idempotent; a callback can kill itself; unload drops both host maps;
a throwing callback keeps repeating; `every(0)` throws while `after(0)` fires.

**Mutation-verified** — self-kill record removed, teardown drop removed, re-arm disabled, 0ms guard
removed. Each was applied, the suite went red, the source restored byte-identical. Two of the four
initially appeared to "survive" because the test-name filter (`timer_`) silently excluded
`unload_kills_…` and `zero_interval_…`; re-run against the full suite, both bite. A mutation that
survives is either a missing test or a mis-scoped test run — check which before believing it.

## 8. Out of scope

Timer data payloads (a closure captures what it needs), `TIMER_FLAG_NO_MAPCHANGE` (no map-change
timer semantics yet), and pausing/rescheduling an existing handle.

---

## 9. Live-gate result (2026-07-25)

Docker CS2, sniper build from this branch, 0 load failures. Unlike the movement and `command()`
slices this one needs no players at all, so every criterion is provable on hardware — and was.

| | |
|---|---|
| one-shot: `alive=true` on arm, `alive=false` after firing | PASS |
| repeater: exactly 5 ticks then **self-kill**, still 5 four seconds later | PASS |
| `kill()` → `true`, second `kill()` → `false` (idempotent) | PASS |
| `every(0, …)` throws `RangeError` | PASS |
| **unload teardown**: alive at `ticks=2`, hot-reload, `ticks=0` and still 0 after 8s | PASS |

The self-kill row is the one that matters most: it is the exact bug the first implementation had
(detecting a self-kill by looking in a map the drain had already emptied), caught by a unit test and
now confirmed on the server. The unload row needed a second attempt — the first used a 700ms
repeater that had already self-killed at 5 ticks before the reload, so it proved nothing; re-run
with a 3s interval it was genuinely alive at reload time.

Server restored to its pre-gate baseline.

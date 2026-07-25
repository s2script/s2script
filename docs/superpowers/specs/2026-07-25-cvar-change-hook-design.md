# Cvar change hook — design

**Status:** Implemented and live-gated (2026-07-25).
**Builds on:** the `ICvar*` (`s_pCvar`) the shim already acquires at Load, and the `EventMux` +
`owner_stores` pattern used by `OUTPUT_MUX`/`MAP_MUX`.

---

## 1. Goal

SourceMod's `HookConVarChange`, ModSharp's cvar change hook: be told when a cvar's value changes.
s2script could read (`getCvar`), write (`setCvar`) and register (`registerCvar`) cvars, but had no
way to *react* to one changing.

## 2. No RE needed

`ICvar::InstallGlobalChangeCallback(FnChangeCallbackGlobal_t)` (`icvar.h:87`) is a plain vtable call
on the interface the shim already holds, and the callback hands over
`(ConVarRefAbstract*, slot, newValue, oldValue)` with both values **already as strings**.
`ConVarRefAbstract::GetName()` → `m_ConVarData->GetName()` → `m_pszName` is inline the whole way, so
it pulls in no tier1 symbol — the cascade `tier1_shims.cpp` exists to avoid.

## 3. One engine callback, fanned out in core

The shim installs **one** global callback unconditionally at Load — like the `FireOutputInternal`
detour, so there is no per-subscribe engine work — and forwards `(name, new, old)` to core. Core owns
`CVAR_MUX`, keyed by cvar name with `"*"` meaning every cvar, and decides who cares.

```ts
Server.onCvarChange("mp_friendlyfire", (name, next, prev) => { … });
Server.onCvarChange("*", (name, next, prev) => { … });   // name says which one moved
```

Returns `{ dispose() }`. Subscriptions are ledgered, so unload removes them whether or not
`dispose()` runs — the ledger is the teardown authority.

**Unload removes the engine callback.** Leaving it installed points the engine at a function inside
a `.so` that is about to be unloaded: a use-after-free on the next cvar write. Guarded by a
`s_cvarChangeCbInstalled` flag so Unload only removes what Load installed.

## 4. Notify-only, and that is not a shortcut

The engine's global change callback runs **after** the value has been applied, so there is nothing
to veto — handlers return nothing and the API says so. Vetoing would need
`ICvar::CallFilterCallback`, a different and much riskier hook; out of scope.

Dispatch mirrors `dispatch_map_start`: snapshot first so no mux borrow is held across JS, a
per-handler `TryCatch` so one thrower cannot stop the rest, and a `try_borrow_mut` graceful-skip so
a handler that *itself* sets a cvar is skipped rather than double-borrowing the isolate.

## 5. Testing

Five core tests: exact-name and `"*"` fan-out with correct `(name, new, old)`; a throwing handler
does not stop the others; `dispose()` stops delivery; unload drops the subscription and a later
change does not dispatch into a dead context; a non-function handler throws `TypeError`.

**Mutation-verified** — wildcard fan-out removed, ledger teardown no-oped, per-handler containment
removed, `off_change` no-oped. Each applied, suite red, source restored byte-identical.

## 6. Live gate — PASS

| | |
|---|---|
| control: no watch armed → no handler output | PASS |
| `"*"` watch: `sv_cheats`, `mp_friendlyfire` fire with correct `old -> new` | PASS |
| three consecutive toggles all fire, values track | PASS |
| a **no-op write** (same value) does NOT fire | PASS |
| `dispose()` → no further delivery | PASS |

The no-op row is worth keeping: the engine only calls back on a real change, so plugins do not need
to de-dupe. Server restored to its pre-gate baseline.

## 7. Out of scope

Vetoing a change (§4), per-client cvar queries (`QueryClientConVar`), and a typed
non-console `setCvar` — today's `setCvar` still routes through the console and carries the
documented injection caveat.

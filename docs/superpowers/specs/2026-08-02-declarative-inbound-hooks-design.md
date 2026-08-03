# Declarative inbound hooks — a gamedata-declared engine detour

**Status:** Approved — ready for planning.
**Audience:** core/shim maintainers; game-package authors adding an engine hook.
**Builds on:** A5b's `calls` descriptors and its two validators
(`docs/superpowers/specs/2026-08-01-gamedata-tiering-design.md` §9); `s2detour::Install`
(`shim/src/detour.h`); the lazy-install-on-first-subscribe pattern already used for
`UserCmd.onRun` (`shim/src/s2script_mm.cpp:1994`); `fan_out_collapsing` + `HookResult` (A4).
**Reference:** `alliedmodders/sourcemod` — `extensions/cstrike/{forwards,natives,extension}.cpp`
(`CS_OnTerminateRound`), read directly.

---

## 1. Why

The core-stabilization audit's headline finding was that **core has a declarative *outbound* path
but no declarative *inbound* path** — every engine→JS notification is a hand-rolled vertical slice.
A5 built the outbound half: an engine *call* is now a gamedata entry with zero core diff. The
inbound half is still 7–9 coordinated core edit sites per hook.

A5b then created the concrete need. It moved `TerminateRound` into `gamedata/cs2` ownership, so the
shim no longer names it — verified, `grep '"TerminateRound"' shim/src core/src` is empty. A detour
on that function would have to name the key again and fail `check-gamedata-owners.sh`. The boundary
we just moved would move back.

A5b also **lost a capability**: `Events.onPre("round_end")` no longer runs for a JS-issued
`terminateRound`, because `nextFrame`'s resolver holds the isolate borrow and pre-hooks are not
deferrable. Reading SourceMod showed that was never the right hook point anyway — SM's answer is
`CS_OnTerminateRound`, a forward fired from a **detour on the engine function**, on the outbound
path, where re-entrancy does not arise.

So one mechanism closes the audit's headline gap, restores the lost capability at the correct hook
point, and keeps the boundary A5b moved.

## 2. What SourceMod does (read from the source)

`CS_OnTerminateRound` is `ET_Event` with `Param_FloatByRef, Param_CellByRef`
(`extension.cpp:102`), fired from `DETOUR_DECL_MEMBER4(DetourTerminateRound, void, float delay,
int reason, int unknown, int unknown2)` (`forwards.cpp:139`). Four behaviours matter:

1. **Lazy install, gated on subscriber count.** `extension.cpp:351`:
   `if (!m_TerminateRoundDetourEnabled && g_pTerminateRoundForward->GetFunctionCount())
   m_TerminateRoundDetourEnabled = CreateTerminateRoundDetour();` — an unsubscribed hook installs
   no detour and costs no treadmill surface.
2. **A bypass latch for its own outbound call.** `natives.cpp` sets `g_pIgnoreTerminateDetour = true`
   before the `SDKCall` (three sites); the detour clears it, calls the original, and returns
   **without firing the forward** (`forwards.cpp:159-163`). **So a plugin-issued `CS_TerminateRound`
   does not fire `CS_OnTerminateRound`.** That is SM's semantic, not an accident.
3. **By-ref args are written back.** `PushFloatByRef(&delay)`, `PushCellByRef(&reason)`, then the
   possibly-modified values are handed to the original.
4. **`result >= Pl_Handled` returns without calling the original** — the termination is blocked.

The detour itself is compile-time C++ per signature (`DETOUR_DECL_MEMBER2/3/4`); gamedata supplies
only the *address*. **SM's hooks are not declarative in shape, only in location.** That is the key
constraint and this design keeps it (§4).

> **v1 SCOPE NOTE (added after the whole-branch review).** This spec is written as "a game package
> *or a plugin* declares a hook" (§5, §7). **As shipped, only a game package can.** Core's registry
> and its `engine:hooks` permission check are owner-generic and tested, but `s2s build` refuses both
> a `hooks` gamedata section and an `engine:hooks` permission, so no `.s2sp` can carry one — and the
> validator is not the only gap: a plugin-declared hook would also need a typed subscribe surface
> (`__s2_hook_on` is absent from `packages/sdk/globals.d.ts`, and `__s2pkg_game_ctx` is a
> game-package-only extension point) and a `gen-hooks` codegen that is not hardcoded to
> `gamedata/cs2/game.cs2.jsonc`. Opening that path is its own slice. The runtime checks were
> deliberately left as-is rather than narrowed to match: they are the correct checks for the design,
> and weakening a default-deny gate to fit a temporary scope is how a gate stops being one.

## 3. The bypass latch is also what makes this safe

Our constraint SM does not have: core holds `HOST.borrow_mut()` across all JS. A detour firing
while core is borrowed would hit the same wall A5b's D3 did — pre-hooks are not deferrable, so the
hook would be skipped.

The bypass removes the case **on the path it covers**. A hook does not fire for our own outbound
descriptor invocation, because that invocation sets the latch — so adopting SM's semantic for SM's
reasons also closes the re-entrancy hole for the JS→engine path a hook actually names.

**It does not close the case in general, and the implementation does not assume it does.** Our latch
is scoped to *(owner, call name)* where SM's `g_pIgnoreTerminateDetour` was global, so any other
JS→engine path to the same address — a plugin's own `calls` descriptor under `engine:calls` — arms
nothing. The backstop for that is the fan-out's existing re-entrancy guard: nothing runs, the engine
call proceeds unhooked (the safe direction), and `dispatch_hook` records the skip as a **named**
degrade on that hook, rate-limited to once per hook because it can fire on every engine call.

**Stated plainly, because it will surprise someone:** `GameRules.onTerminateRound` does **not** fire
when a plugin calls `GameRules.terminateRound()`. It fires when the *engine* ends a round. This
matches SourceMod exactly. A plugin wanting to observe another plugin's termination should use the
`round_end` event, which still dispatches (deferred one frame when re-entrant, per PR #71).

## 4. Shape is compile-time; location is data

You cannot detour an arbitrary signature from data — a handler must have the callee's exact ABI.
SM solves this with per-arity macros. We do the same, deliberately:

- **The shim ships a small, closed vocabulary of inbound *thunk shapes*.** Each is a compile-time
  C++ function with a concrete signature that marshals its args into a block-scoped view, calls
  core with a hook id, and applies the collapse.
- **Gamedata declares which shape a hook uses and where the function is.** The shim never names a
  game function; it names a *shape*. `check-gamedata-owners.sh` stays green.

Two shapes ship, chosen so the vocabulary is proven general rather than fitted to one case:

| Shape id | C signature | First consumer |
|---|---|---|
| `this_f32_i32_i32_i32` | `void(void* self, float, int, int, int)` | `TerminateRound` |
| `this_void` | `void(void* self)` | `Respawn` |

A third shape is a shim edit plus a table row — the same cost as adding a `fan_out` channel, and
explicitly *not* zero. Zero-core-diff applies to adding a *hook on an existing shape*; a new shape
is a core change, and the spec says so rather than implying otherwise.

## 5. The descriptor

Authored in the game package's gamedata, beside `calls` (a plugin's own gamedata is the design's
intent but is not buildable in v1 — see the scope note above):

```jsonc
"hooks": {
  "onTerminateRound": {
    "target": { "kind": "signature", "name": "TerminateRound",
                "validate": { "string-xref": { "at": 11, "dispOff": 3, "instrLen": 7,
                                               "expect": "TerminateRound" } } },
    "shape":   "this_f32_i32_i32_i32",
    "params":  ["delay", "reason", "_unused3", "_unused4"],
    "mutable": ["delay", "reason"],
    "bypassWith": "terminateRound"
  }
}
```

- `params` names the shape's positional args for the generated `.d.ts` and the JS view. Documentary
  plus type generation; the runtime marshals by position.
- `mutable` lists which params JS may write back. Anything absent is read-only in the view.
- `bypassWith` names the `calls` descriptor in the **same owner** whose invocation sets the latch —
  the declarative form of `g_pIgnoreTerminateDetour`. Validated at load: an unknown name fails the
  descriptor by name.

**A validator is MANDATORY for every hook**, unlike `calls` where uniqueness alone suffices for a
signature target. A wrong call address misbehaves; a wrong *detour* address overwrites the prologue
of whatever is actually there. `s2detour::Install` already refuses a relative/rip-relative prologue,
but that is a decode check, not an identity check. A `hooks` entry without `validate` fails the
**build**, not the load.

## 6. Lifecycle

**Resolution** reuses A5b's path unchanged: core resolves the target through
`S2_EngineCallResolve`, including both validators, and holds the resolved address. No second
resolver.

**Install is lazy, on first subscribe**, reusing the `UserCmd.onRun` precedent
(`s2script_mm.cpp:1994`, "idempotently on the FIRST-EVER subscribe — core calls it"). No
subscribers, no patched bytes.

**Removal** is `s2detour::RemoveAll()` at Unload, which already restores every installed prologue.
A hook whose last subscriber goes away keeps its detour installed until Unload — uninstalling a
live detour races the engine calling through it, and SM does not do it either.

**Dispatch** goes through `fan_out_collapsing`, so priority, per-handler `TryCatch` isolation and
the standard collapse all come for free. Per ARCHITECTURE.md the collapse takes the max by
precedence; **`Handled` or `Stop` suppresses the original call**, matching SM's `result >=
Pl_Handled`. `Changed` means "I wrote a mutable param, still call the original."

**Mutation** is a block-scoped view over the thunk's stack args, exactly like `Damage.onPre`'s
`info` — no pointer crosses into JS, and the view cannot outlive the dispatch.

## 7. Permissions

`engine:hooks`, mirroring `engine:calls`: mandatory manifest declaration plus operator allow-list,
default-deny. The game package is exempt as first-party runtime, via the same reserved owner A5b
introduced.

A hook is strictly more dangerous than a call — it patches bytes and can suppress engine behaviour
— so the permission is **separate** from `engine:calls`. An operator granting a plugin the ability
to call engine functions has not thereby granted it the ability to detour them.

In v1 the game package is the only declarer, so `engine:hooks` has no plugin that can request it
(see the scope note above). The permission ships anyway, enforced, because the alternative — adding
the gate later, once plugins are already declaring hooks — is the retrofit this project's
default-deny posture exists to avoid.

## 8. Scope

**In:** the `hooks` grammar, two shapes, resolution + mandatory validation, lazy install, the bypass
latch, the collapse contract, `.d.ts` generation from the descriptor, `GameRules.onTerminateRound`
and `Player.onRespawn` as the two shipped consumers.

**Out:** uninstall-on-last-unsubscribe (§6); hooks on vtable targets (signature targets only in v1 —
a vtable detour has different failure modes and no consumer needs it); return-value interception
(both shapes are `void`; a non-void shape is additive); plugin-authored hooks reaching *engine-*
generic functions (the boundary is unchanged — a hook lives in the owner whose gamedata declares
it); and — per the scope note above — plugin-**declared** hooks end to end (the SDK validator
sections, a plugin-side typed subscribe surface, and a non-game-hardcoded `gen-hooks`).

## 9. Testing

Offline: descriptor grammar (unknown shape, missing `validate`, unknown `bypassWith` all fail the
build); the collapse contract (`Handled` suppresses, `Changed` writes back, `Continue` passes
through); the bypass latch (an outbound call through the named descriptor does not fire the hook);
lazy install (no subscribers → `s2detour::Install` never called).

Live gate — the parts only a real server proves:
1. A natural round end fires `onTerminateRound` with a plausible `reason`.
2. `GameRules.terminateRound()` from JS does **not** fire it (the bypass).
3. `HookResult.Handled` from a handler actually prevents the round ending.
4. A mutated `reason` reaches the engine — the round ends with the substituted reason.
5. With no subscriber, the boot log shows the detour was never installed.

## 10. Risks

| Risk | Mitigation |
|---|---|
| A wrong detour address corrupts an unrelated function | Validator mandatory (§5); `s2detour::Install` refuses an unrelocatable prologue; failure degrades that hook by name |
| The bypass latch leaks (set, then the call fails before the detour clears it) | Clear it on both paths and scope it to the invoke; a stuck latch silently disables the hook, so log when a set latch is observed at a second invoke |
| A hook handler that itself calls the hooked function | The latch is set only by the descriptor invoke path; a handler calling `terminateRound()` sets it and is bypassed — same as SM |
| Shape vocabulary grows per consumer | Accepted and stated (§4): a new shape is a core change; only new hooks *on an existing shape* are zero-diff |
| High-frequency detour cost (`Respawn` fires every round start) | Lazy install means unsubscribed cost is zero; subscribed cost is one collapse per engine call, the same as any notify channel |

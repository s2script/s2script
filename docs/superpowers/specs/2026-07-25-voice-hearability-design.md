# Voice hearability — design

**Status:** Approved — ready for planning.
**Audience:** plugin authors building proximity/team-scoped voice; core+shim maintainers.
**Builds on:** the voice-control slice's already-installed `SetClientListening` PRE hook
(`shim/src/s2script_mm.cpp`), and the `checktransmit` slice's owner-merged-rule pattern
(`core/src/v8host.rs` `TRANSMIT_RULES` / `transmit_recompute_and_push`).

---

## 1. Goal

Let a plugin decide **who can hear whom**, per (receiver, sender) pair — ModSharp's `ClientCanHear`
capability — without putting JavaScript on the engine's voice hot path.

This closes the last fully-absent item in the voice category. `Client.voiceMuted` already answers "can
this player speak at all"; nothing today answers "can *this* player hear *that* one", which is what
proximity voice, team-isolated comms, and spectator-hears-all need.

**Non-goal:** proximity voice itself. This slice ships the mechanism; distance math and recompute
cadence belong to a plugin.

## 2. Why this is small

The hook already exists and already carries both slots:

```cpp
SH_DECL_HOOK3(IVEngineServer2, SetClientListening, SH_NOATTRIB, 0, bool,
              CPlayerSlot /*receiver*/, CPlayerSlot /*sender*/, bool /*bListen*/);
```

`Hook_SetClientListening` currently collapses that to a single per-sender flag
(`s_voiceMuted[sender]`). Hearability is the same hook point with the receiver dimension kept. No new
sig-scan, no new detour, no new RE, no new gamedata.

## 3. The hot-path constraint (why not a callback)

The hook fires per (receiver, sender) pair per voice refresh — up to O(n²) — and the game re-asserts
the listen matrix continuously. The existing code documents the contract: *plain array reads, no
FFI/JS/allocations*.

A `ClientCanHear`-style callback into JS would therefore run up to 64×64 isolate entries per refresh.
That is the cost this design exists to avoid, so the rule is **declarative state, evaluated in the
shim** — the same choice `Transmit` already makes for per-client entity visibility.

## 4. Layering (decided)

Two independent layers, checked in order:

1. **`Client.voiceMuted`** — global per-sender moderation mute. Unchanged. If set, the sender is
   inaudible to everyone, full stop.
2. **Hearability** — the per-pair rule. Consulted only when the sender is not muted.

Moderation always beats a gameplay rule, `plugins/basecomm` keeps working untouched, and this slice is
purely additive: no existing behavior changes and no shipped SDK surface breaks.

## 5. Data model

**Shim holds only the merged result** (mirroring `TransmitEntry`):

```cpp
static uint64_t s_voiceAudible[kMaxClientSlots] = {0};  // bit r set = receiver r may hear sender s
static uint64_t s_voiceHasRule = 0;                     // bit s set = sender s has a rule at all
```

`s_voiceHasRule` is required: a mask of `0` means "audible to nobody", which must be distinguishable
from "no rule, leave the engine alone".

**`kMaxClientSlots` is 64, and a receiver mask is a `uint64`.** Those two numbers being equal is
convenient, not principled — the same coupling `Transmit`'s `uint64_t mask` already has. If the slot
cap ever rises, both must change together; the plan pins this with a `static_assert`.

**Slot-reuse hygiene.** A rule is authored about the player occupying a slot, and the engine recycles
slots. Both halves of the state are therefore cleared when a client disconnects: the shim clears
`s_voiceAudible[s]` / the `s_voiceHasRule` bit in `Hook_ClientDisconnect` (alongside the mute clear
already there), and core drops every owner's rule for that sender from `dispatch_client_event`. The
core-side clear must run **before** that dispatcher's no-subscriber early return, so it happens whether
or not any plugin subscribed to `"disconnect"`. Without this, a departing player's rule silences —
or grants hearing to — whoever connects into the slot next, with no plugin action and no way for them
to discover why.

**Core owns policy** (mirroring `TRANSMIT_RULES`):

```rust
VOICE_RULES: owner -> (senderSlot -> u64 mask)
```

Merged **AND** across owners — most-restrictive-wins, exactly as `transmit_recompute_and_push` does.
Two plugins each restricting a speaker yields the intersection. AND is also the safe direction for a
moderation-adjacent feature: a plugin can never *widen* what another plugin restricted.

Recompute-and-push runs on every set, reset, and owner teardown.

## 6. Hot path

```cpp
if (!degraded && bListen && s >= 0 && s < kMaxClientSlots) {
    if (s_voiceMuted[s]) → deny;                                           // layer 1 (NOT counted)
    else if ((s_voiceHasRule >> s) & 1) {
        // r < 0 is the engine's broadcast/console pseudo-receiver: no bit to test, so a rule
        // cannot deny it. Bounds-check r before indexing.
        if (r >= 0 && r < kMaxClientSlots && !((s_voiceAudible[s] >> r) & 1)) {
            s_voiceRewrites++;                                             // layer 2 only
            deny;
        }
    }
    if (deny) → rewrite bListen = false;
}
```

Two shifts and two tests added to an existing branch. No FFI, no JS, no allocation — the documented
contract is preserved verbatim.

## 7. Engine ops

Two new ops, **appended** at the end of `S2EngineOps` (the struct is ABI-ordered; new fields append
only, never insert), and populated at the matching end of the shim initializer:

```rust
pub type VoiceAudibleSetFn   = extern "C" fn(sender: c_int, mask: u64) -> c_int;
pub type VoiceAudibleClearFn = extern "C" fn(sender: c_int) -> c_int;
```

Return convention, stated explicitly because `0` would otherwise be ambiguous:

| Op | `1` | `0` |
|---|---|---|
| `voice_audible_set` | rule applied | degraded (§8), or slot out of range |
| `voice_audible_clear` | a rule was present and removed | nothing to remove, **or** degraded |

`clear` deliberately collapses "absent" and "degraded" into `0` — both mean "no rule is in force
afterwards", which is all a caller can act on. `set` does not, because "applied" and "silently
ignored" are the distinction that matters.

## 8. Degradation

No new failure mode. Hearability rides the existing `s_voiceListenDegraded` flag, set by the voice
slice's own runtime validation — first-fire argument sanity plus the one-shot
`Get/SetClientListening` round-trip that guards the hand-patched `eiface.h` vtable slots.

Two conditions disable enforcement, and the ops must check BOTH: `s_voiceListenDegraded` (validation
failed) and `!s_voiceListenHookInstalled` (the PRE hook never installed, because `EngineToServer` did
not resolve — the classic post-CS2-update failure). The ops are wired unconditionally at Load, so
checking only the degrade flag would accept rules that can never be enforced and report success.

In either state hearability ops return `false` rather than silently accepting rules that will never be
applied. A plugin can therefore tell the difference
between "rule set" and "voice control unavailable on this build".

## 9. Plugin API — `@s2script/sdk/voice`

A new subpath, mirroring `@s2script/sdk/transmit` so the mental model transfers:

```ts
export interface VoiceStats {
  /** SetClientListening pre-hook invocations observed. */
  calls: number;
  /** Senders currently carrying a hearability rule. */
  entries: number;
  /** Times a HEARABILITY rule rewrote bListen to false. Does NOT count `voiceMuted` denials. */
  rewrites: number;
}

export declare const Voice: {
  /** This sender is audible ONLY to `receivers`. An empty array = audible to nobody. */
  setAudibleTo(sender: number, receivers: readonly number[]): boolean;
  /** Drop this plugin's rule for `sender` (the engine decides again once no owner has a rule). */
  reset(sender: number): boolean;
  /** Drop every rule this plugin owns. */
  resetAll(): void;
  /** Hot-path counters, or null when the running shim predates this capability. */
  stats(): VoiceStats | null;
};
```

`stats().rewrites` exists for the same reason the engine-call demo counts entities: a rule that is set
but never applied looks identical to one that works. A climbing `rewrites` is the observable effect,
and it is what the live gate asserts.

**It counts layer-2 denials ONLY.** Incrementing it for `voiceMuted` denials too would make a
basecomm gag keep the counter climbing with no hearability rule in force — which would let the live
gate pass falsely, and would make its "rewrites stop climbing after reset" step fail spuriously. The
counter is scoped to the thing it is named after.

**No timing counters**, unlike `TransmitStats`'s `nsLast`/`nsMax`. `Transmit` times once per snapshot;
this hook fires per (receiver, sender) *pair*, so a clock read per invocation would cost more than the
check it measures and would break the §3 contract. Three plain counters only.

Adding a subpath makes `scripts/check-examples-coverage.sh` demand a consumer, so the slice also ships
an `examples/cookbook/src/recipes/voice.ts` recipe.

## 10. Teardown

`VOICE_RULES` is owner-keyed and registered through `crate::owner_stores::register`, alongside the
`"TRANSMIT"` entry it mirrors:

```rust
crate::owner_stores::register(
    "VOICE",
    Box::new(|owner| { voice_remove_owner(owner); }),
    Box::new(|_ids| {}),            // not a scope surface — same as TRANSMIT
);
```

Plugin unload therefore drops that owner's rules and recomputes, so a departed plugin can never leave
players silenced. This is the ledger-is-the-teardown-authority rule: the plugin's own cleanup code is
not trusted to run.

## 11. Testing

- **Core (`cargo test -p s2script-core`):** AND-merge across two owners; owner teardown recomputes and
  clears; mask encoding round-trip; `setAudibleTo` with an empty array yields mask 0 **with** the
  has-rule bit set (the distinction §5 depends on); ops-absent and degraded paths return false.
- **Shim:** compiles; `static_assert(kMaxClientSlots <= 64)` pins the mask-width coupling.
- **Live gate (Docker CS2, bots) — what is ACTUALLY provable:** `stats().entries` tracking a rule
  reaching the shim (exercises the core→shim push, the AND-merge, and the has-rule bit), `reset`
  dropping it, and **`entries` returning to 0 across a plugin unload** (the owner-store teardown,
  criterion 4).

**`stats().rewrites` CANNOT be exercised with bots, and the gate must not depend on it.** The hot path
only considers denying when the engine passes `bListen == true`, and the engine only does that when a
client is actually transmitting voice. Bots never transmit. Measured on a 12-bot server: with an empty
mask on all 12 senders and ~160k hook `calls`, `rewrites` stayed at exactly 0.

An earlier draft of this section asserted the opposite — "bots do not transmit voice, so the
server-side proof is the `rewrites` counter" — which is self-contradictory: the counter only moves
when voice IS transmitted. The denial path is therefore proven by unit test and by mechanism, not by
observation, and **audible two-human confirmation is the only way to close it** — the same Tier-2
deferral the voice-control slice itself carries for mute.

## 12. Out of scope

- Proximity voice itself (a plugin, not framework).
- Receiver-keyed rules (`setHearsFrom`) — the sender-keyed whitelist covers the use cases and mirrors
  `Transmit`; adding the inverse later is additive.
- Raising the 64-slot cap.
- A per-pair JS callback — §3 is the reason.
- `ClientSpeaking` / `EmitMusic` and the rest of ModSharp's audio hook family.

## 13. Success criteria

1. A plugin restricts who hears a speaker, and `stats().entries` proves the rule reached the shim.
   (The engine honoring it is proven by unit test + mechanism; see §11 — `rewrites` needs real voice.)
2. `Client.voiceMuted` still overrides, and `basecomm` is unmodified.
3. Two plugins' rules AND-merge; neither can widen the other.
4. Plugin unload drops its rules automatically.
5. Degraded voice validation makes the ops return `false`, never a silent no-op.
6. Hot path stays FFI/JS/allocation-free.
7. `make ci` green, including `check-boundary` and `check-examples-coverage`.

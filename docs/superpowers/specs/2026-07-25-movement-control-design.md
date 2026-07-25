# Movement control — design

**Status:** Tier A implemented and live-gated (2026-07-25). Tier B deferred — see §7.
**Audience:** plugin authors writing movement gameplay; core + codegen maintainers.
**Builds on:** the `pawn.movementServices` nav wrapper (`games/cs2/nav-targets.json`), the navgen
pipeline (`packages/sdk/src/navgen/`), and the `EntityRef.write*Via` chain-write surface.

---

## 1. Goal

Close ModSharp's movement surface — `GetMaxSpeed`, duck/stamina/friction control, and the
`ProcessMovement` intercept.

## 2. What we already have (checked, not assumed)

Before designing anything new: `pawn.movementServices` **already exists** and already exposes 53
generated accessors over the full `CCSPlayer_MovementServices` inheritance chain
(`CCSPlayer_MovementServices → CPlayer_MovementServices_Humanoid → CPlayer_MovementServices →
CPlayerPawnComponent`), including `maxspeed`, `stamina`, `surfaceFriction`, `duckAmount`,
`cmdForwardMove`. `@s2script/usercmd` (PR #33) already intercepts input *before* movement runs.

**Every one of those 53 accessors is read-only.** `grep -c 'set: function' games/cs2/js/nav.generated.js`
returns **0**. The gap is not "we cannot reach movement state" — it is "we can only look at it".

And the machinery to fix that already exists too: `FieldDescriptor.writable` is computed by
`classifyField` and honored by **schemagen**'s emitters (`if (f.writable)` → emit a setter).
**navgen's emitters simply ignore the flag.**

## 3. The slice splits in two, and only Tier A ships here

| | Tier A — writable movement fields | Tier B — `ProcessMovement` intercept |
|---|---|---|
| New RE | **none** | vtable slot identification + validation |
| Buys | `GetMaxSpeed` parity, duck/stamina/friction/jump control | pre/post movement hook, `WalkMove` |
| Risk | contained: a curated field list | a per-tick hook in the hottest path in the game |

Per *build by risk, not by layer*, these are separate slices and separate PRs. Tier A is a thin
vertical slice with no unknowns; Tier B is an RE spike that should not ride along with it.

## 4. Tier A: a curated writable allowlist, not a flipped flag

Honoring `f.writable` in navgen would make **53 fields writable at once** — including engine
bookkeeping where a write is undefined behaviour:

```
m_nTraceCount   m_bInStuckTest   m_StuckLast   m_nGameCodeHasMovedPlayerAfterCommand
m_bHasEverProcessedCommand   m_nLastCommandNumberProcessed
```

and it would silently do the same to the other three nav wrappers (`SceneNode`, `WeaponServices`,
`AimPunchServices`), which this slice has not reasoned about at all.

So writability is **opt-in per field, declared in `nav-targets.json`**:

```jsonc
{ "prop": "movementServices", "wrapper": "MovementServices", ...,
  "writable": [ "m_flMaxspeed", "m_flStamina", "m_flSurfaceFriction", "m_flDuckSpeed",
                "m_flDuckAmount", "m_bDucked", "m_bDuckOverride", "m_flFallVelocity",
                "m_flCmdForwardMove", "m_flCmdLeftMove", "m_flCmdUpMove",
                "m_flForwardMove", "m_flLeftMove", "m_flUpMove" ] }
```

This is the charter's *layout is data, semantics are code* line landing exactly where it should.
Which byte a field lives at is regenerable layout. **Whether writing it is safe is a behavioural
fact** — reviewed, deliberate, and diffable. A field absent from the list stays read-only, and a
field in the list that the catalog says is not writable (a 64-bit or vector kind, which has no
`write*Via`) is a **generation-time error**, not a silently-dropped setter.

`m_flMaxspeed` is the ModSharp `GetMaxSpeed` equivalent: the server's movement code reads it every
tick, so writing it is how you make a player faster or slower.

## 5. The `write*Via` surface is incomplete — and that is a fail-closed, not a gap to paper over

Existing chain writers (`core/src/v8host.rs:1082-1094`): `writeInt32Via`, `writeFloat32Via`,
`writeBoolVia`. Missing: `writeInt8Via`, `writeInt16Via`, `writeUInt8Via`, `writeUInt16Via`,
`writeUInt32Via`.

Every field in §4's list is `f32` or `bool`, so **this slice adds no natives**. The allowlist
validator rejects any field whose kind has no chain writer, naming the missing method. When a later
slice wants a `u8` movement field it adds the native deliberately, rather than discovering at runtime
that the setter silently never existed.

## 6. Networked writes do NOT replicate, and the spec says so rather than hiding it

`notifyStateChanged(offset)` resolves the **root entity** and calls
`ent_state_changed(entityPtr, offset)` (`core/src/v8host.rs:4071`). A nav-chain write does not change
the pawn — it changes the *movement-services subobject* hanging off `m_pMovementServices`. Calling
the root-entity notifier with a chain-relative offset would mark the wrong bytes on the wrong object
and corrupt the change-tracking bitfield.

Several of these fields are genuine `CNetworkVarBase` members (`m_flMaxspeed`'s
`NetworkVar_m_flMaxspeed` RTTI is present in `libserver.so`), so this is real, not theoretical.

**Therefore Tier A generates setters that write but do not notify.** The consequence, stated plainly
in the TSDoc so no plugin author has to discover it:

> The server reads these every movement tick, so gameplay effects apply immediately. The value is
> **not** flagged for replication, so a client predicting the old value may see brief mismatch
> (jitter) until the next authoritative correction. SourceMod's `SetEntPropFloat` on these fields
> behaves the same way.

A chain-aware `notifyStateChangedVia` is **deferred, with a reason**: it must call
`NetworkStateChanged` on the subobject pointer, and whether `CCSPlayer_MovementServices` (a
`CPlayerPawnComponent`, *not* obviously a `CEntityInstance`) is a valid receiver is unverified.
Guessing wrong is a crash in the networking path, not a degraded read. That needs its own RE + live
gate — exactly the `CCommand` lesson from PR #17: a plausible-looking call is not a verified one.

## 7. Tier B groundwork: `ProcessMovement` is a SIGNATURE, not a vtable slot

Two wrong turns are recorded here because each cost real time.

**Wrong turn 1 — a fictitious vtable.** An earlier revision published "primary vtable at
`0x2487678`, exactly 24 slots". That was derived by scanning raw file bytes for a qword equal to the
typeinfo address. CS2 vtables live in `.data.rel.ro` and are filled at load by
`R_X86_64_RELATIVE` relocations, so the file bytes are *not* the pointers; the scan matched unrelated
data. Its "slot functions" disassembled to KV3 entity serialization and round/team code.

Resolved properly, every hop from `readelf -r`:

| hop | address | how |
|---|---|---|
| name string | `0x821b00` | `.rodata`, `26CCSPlayer_MovementServices` |
| typeinfo | `0x2489e80` | the reloc **targeting** the name sits at typeinfo+8 |
| vtable slot 0 | `0x248ace0` | the reloc **targeting** the typeinfo sits at vtable+8 |
| slot count | **59** | length of the contiguous relocation run |

Independently corroborated: CS2Fixes' gamedata lists
`CCSPlayer_MovementServices::CheckMovingGround` at linux vtable index **45**, which only fits a
vtable of ≥46 slots — consistent with 59, and impossible under the fictitious 24.

**Wrong turn 2 — assuming it was a vtable slot at all.** It is not.
[CS2Fixes' gamedata](https://github.com/Source2ZE/CS2Fixes/blob/main/gamedata/cs2fixes.jsonc) carries
`ProcessMovement` as a **byte signature**, which is strictly better: directly self-resolvable against
our own binary, no slot-index ambiguity, and it survives vtable reordering.

```
linux: 55 48 89 E5 41 57 41 56 41 55 49 89 F5 41 54 53 48 89 FB 48 83 EC ? 48 8B 7F
```

**Validated against our pinned build** (`docker/cs2-data`, not taken on trust):

- **UNIQUE** — exactly one match in `libserver.so`, at file offset `0x157d5f0`. A borrowed pattern
  that matched twice would be unusable; this is the check `docs/re-strategy.md` requires.
- It lands next to the `PreWalkMove` string xref at `0x157ccaf` — the movement code region.
- **Detourable**, which `EmitSound` is not. `s2detour` steals 14 bytes; here that spans
  `push rbp / mov rbp,rsp / push r15 / push r14 / push r13 / mov r13,rsi / push r12` = 15 bytes of
  plain push/mov with **no relative or rip-relative operand**, so `detour.cpp`'s two refusal
  conditions do not apply.
- Prototype from the prologue: `this` in `rdi` (`mov rbx,rdi`), a second pointer arg in `rsi`
  (`mov r13,rsi`) — i.e. `void ProcessMovement(CCSPlayer_MovementServices*, CMoveData*)`.

### What still blocks the hook: per-player identity

The detour receives the *movement services* pointer, and there is no validated way to get from it to
a player slot. `CPlayerPawnComponent` exposes only `__m_pChainEntity`, and our schema catalog knows
just `m_PathIndex @32` inside `CNetworkVarChainer` — the owner pointer's offset is **not** a schema
field. Dereferencing a guessed offset in the per-tick movement path is precisely the crash class the
degrade doctrine exists to prevent, so it is not being guessed.

The sound design is a **reverse map**: cache `(services* → slot)` for the 64 slots, built from the
already-validated `CBasePlayerPawn::m_pMovementServices` chain, and refreshed on a miss. Dispatch
then costs pointer compares, no derefs. That plus the detour, a blockable `HookResult`, lazy arming
on first subscribe, and a live gate is the next slice — it is not a bolt-on to this one.

## 8. Testing

- **navgen unit tests:** a writable-listed `f32`/`bool` emits a getter *and* setter; an unlisted
  field emits getter only; a listed field with a non-writable kind is a generation-time error; a
  listed field that does not exist in the catalog is a generation-time error (catches a typo'd or
  CS2-update-renamed field instead of silently emitting nothing).
- **Mutation-verify each of those four**, per the discipline the voice slice established: introduce
  the bug, confirm the test fails, restore. A test that has never failed is not evidence.
- **`check-nav-generated.sh`** must stay green — the committed codegen is regenerated and diffed.
- **`check-plugins-typecheck`** proves `MovementServices` fields lose `readonly` in the emitted
  `.d.ts` only for allowlisted fields.
- **Live gate:** `sm_speed <slot> <mult>` in the cookbook sets `maxspeed` and `sm_movement <slot>`
  reads it back on real engine memory.
  ~~Bot-provable — bots move under server-side movement code, so unlike `Client.command` this slice
  **can** be proven on hardware, and must be.~~ **That claim was wrong; see §11.**

## 9. Out of scope

- Tier B (`ProcessMovement`/`WalkMove` hooks) — its own spec.
- `notifyStateChangedVia` — §6.
- New `write*Via` natives — §5.
- Setters on the other three nav wrappers; each needs its own reasoning about what is safe to write.

## 10. Success criteria

1. `pawn.movementServices.maxspeed = 400` reaches real engine memory on a live server and survives
   the movement tick. (Originally worded as "changes a bot's movement speed" — not achievable here,
   §11.)
2. Only allowlisted fields gain setters; the other 39 stay read-only, and the other three wrappers
   are untouched (verified by diffing the generated files).
3. A typo'd or non-writable-kind allowlist entry **fails generation**, naming the field.
4. The no-replication caveat is in the emitted TSDoc, not only in this document.
5. `make ci` green, `check-nav-generated` included.

---

## 11. Live-gate result (2026-07-25)

Docker CS2, `de_inferno`, 12 bots, sniper build from this branch. 3 plugins loaded, 0 faults.

**PASS — the write path works end to end on hardware.**

| step | result |
|---|---|
| `sm_movement 0` | `maxspeed=260 stamina=0 friction=1 ducked=false duckAmount=0` |
| `sm_speed 0 2` | `maxspeed 260 -> 520` |
| read back | `520` |
| re-read at +2s / +5s / +10s | `520`, `520`, `520` — **not** recomputed by the movement tick |
| `mp_restartgame` → fresh pawns | back to `260` on all 8 slots |

The last row matters: a fresh pawn reading 260 again proves these are genuine per-pawn engine reads
and writes through the `m_pMovementServices` chain, not a cached JS value.

### The "bot moves faster" half could NOT be proven, and my spec was wrong to promise it

§8 asserted this slice was bot-provable "because bots move under server-side movement code". **The
bots on this server do not move at all.** Measured, not assumed — the cookbook sampler reports both
`absVelocity` magnitude and frame-to-frame position delta:

```
slot 0 over 192 frames: peak absVelocity=0.0 u/s, peak by position delta=0.0 u/s
slot 1 / 3 / 5 over 128 frames: 0.0 / 0.0        (same on every slot tried)
```

`mp_restartgame`, `mp_warmup_end`, `mp_freezetime 0`, `bot_stop 0`, `bot_wander 1` change nothing;
the map directory holds only `.vpk`s with no navigation data, so the bots have nowhere to walk.

I also tried driving movement directly — writing the allowlisted `forwardMove`/`cmdForwardMove` every
frame — on the theory that those are the inputs the movement code consumes. Still `0.0`: a
game-frame write lands at the wrong point in the tick relative to `ProcessUserCmd`. That experiment
was **removed** from the cookbook rather than shipped, because an example that appears to do
something it does not is worse than no example.

**So "the engine acts on the written value" is deferred to a human-client session**, the same Tier-2
deferral the voice slices carry. This is the second time I have written a gate whose premise
contradicted itself; the check that catches it is running it, not reviewing it.

Server restored to its pre-gate baseline afterwards.

# Movement control — design

**Status:** Approved — ready for planning.
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

## 7. Tier B groundwork (recorded, not built)

RE already done, so the next slice does not restart cold:

- `libserver.so` exports **no** movement symbols at all — `nm -D --defined-only | grep -iE
  'ProcessMovement|WalkMove|GetMaxSpeed'` is empty. Everything must be RTTI- or signature-resolved.
- RTTI **is** present: `26CCSPlayer_MovementServices` at `.rodata:0x821b00`. Its `__class_type_info`
  is at `0x2487e80`, referenced by five vtables; the primary (`offset_to_top == 0`) is at
  `0x2487678`, so **slot 0 is at `0x2487680`**.
- That vtable is **exactly 24 slots** — qword 24 is `0xffffffff2bf4f9c5`, the start of an unrelated
  hash/string table, so the bound is read off the data, not assumed.
- Identifying *which* of the 24 is `ProcessMovement` is the open question. The obvious
  string-xref shortcut is a red herring: the only `ProcessMovement` mention
  (`.rodata:0x9828b8`) sits in a *spawn* assert about ground flags, referenced from `0xb07f74`,
  which is not the function itself.

All addresses above are for the currently-installed build and are **hints for the next slice, not
constants to ship** — per `docs/re-strategy.md`, the shipped resolver must self-resolve against
whatever binary it loads into.

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
- **Live gate:** `sm_speed <slot> <mult>` in the cookbook sets `maxspeed`; a bot's movement speed
  visibly changes and reverts. Bot-provable — bots move under server-side movement code, so unlike
  `Client.command` this slice **can** be proven on hardware, and must be.

## 9. Out of scope

- Tier B (`ProcessMovement`/`WalkMove` hooks) — its own spec.
- `notifyStateChangedVia` — §6.
- New `write*Via` natives — §5.
- Setters on the other three nav wrappers; each needs its own reasoning about what is safe to write.

## 10. Success criteria

1. `pawn.movementServices.maxspeed = 400` changes a bot's movement speed on a live server.
2. Only allowlisted fields gain setters; the other 39 stay read-only, and the other three wrappers
   are untouched (verified by diffing the generated files).
3. A typo'd or non-writable-kind allowlist entry **fails generation**, naming the field.
4. The no-replication caveat is in the emitted TSDoc, not only in this document.
5. `make ci` green, `check-nav-generated` included.

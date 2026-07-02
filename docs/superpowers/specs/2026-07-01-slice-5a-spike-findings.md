# Slice 5A — Spike findings: `CEntityIdentity` serial offsets + `CEntityHandle` bit-split

**Task:** Confirm, live on the Docker CS2 server, the three engine-layout constants the Slice-5A
`EntityRef`/handle natives need — the offset from a `CEntityInstance*` to its `CEntityIdentity*`, the
offset of the serial-bearing `CEntityHandle` within a `CEntityIdentity`, and the `CEntityHandle`
index/serial bit-split — plus the invalid-handle sentinel and proof that the serial invalidates on
entity destruction/reuse. Companion to `2026-07-01-slice-5a-entityref-design.md` §8.

**Method:** static read of the vendored hl2sdk headers (the exact structs the shim already compiles
against) to form the hypothesis, then a **live memory probe** on the running CS2 server. A throwaway
native `__s2_spike_probe(idx, handleFieldOff, verbose)` (in `core/src/v8host.rs`, since deleted) took
`ops.ent_by_index(idx) → CEntityInstance*`, logged raw pointer/`u32` values at a conservative
in-object offset range, chased the candidate `CEntityIdentity*`, decoded the handle both ways
(15-bit vs 16-bit index split), and cross-checked the controller→pawn handle round-trip. A throwaway
demo `.s2sp` drove it against the two live bot controllers (entity indices 1 and 2). All raw reads were
bounded to header-backed in-object offsets and gated on plausible (non-null, >0x10000, 8-aligned)
pointers.

**Server state:** `de_inferno`, 2 bots (slots 0/1 → controller entity indices 1/2), map live and
ticking, addon loaded via Metamod. Sniper build (`scripts/build-sniper.sh`) → `s2script.so`
(GLIBC_2.14) + `libs2script_core.so` (GLIBC_2.30).

---

## The three confirmed constants (exact values)

| Constant | Value | Meaning |
|---|---|---|
| `ENT_IDENTITY_PTR_OFFSET` | **`0x10`** (16) | Byte offset of the `CEntityIdentity*` (`m_pEntity`) within a `CEntityInstance`. |
| `ENT_IDENTITY_HANDLE_OFFSET` | **`0x10`** (16) | Byte offset of the `CEntityHandle` `uint32` (`m_EHandle`, packs index+serial) within a `CEntityIdentity`. |
| `HANDLE_ENTRY_BITS` | **`15`** | Index = `handle & 0x7FFF` (low 15 bits); serial = `handle >> 15` (high 17 bits). |

**Invalid-handle sentinel:** `INVALID_EHANDLE_INDEX = 0xFFFFFFFF` (const.h:77; the `CEntityHandle`
default `m_Index`). Decodes to index `0x7FFF` / serial `0x1FFFF` under the 15-bit split; treat the raw
`0xFFFFFFFF` value (and the shim's existing `EF_IS_INVALID_EHANDLE` flag on a free slot) as invalid.

### Decode / validate reference (engine-generic, no game names)

```
index  = handle & ((1 << 15) - 1)      // handle & 0x7FFF
serial = handle >> 15                    // 17 bits
invalid if handle == 0xFFFFFFFF

// EntityRef validation:
//   ident   = *(CEntityIdentity**)(entity_ptr + 0x10)
//   live_eh = *(u32*)(ident + 0x10)
//   valid   = (live_eh >> 15) == ref.serial   (and index matches / slot non-empty)
```

Note: the SDK's `CEntityIdentity::GetRefEHandle()` subtracts `(m_flags & EF_IS_INVALID_EHANDLE)` (0/1)
from the serial so a *free* slot's stored handle can never equal a live one. For a **live** entity
`m_flags & EF_IS_INVALID_EHANDLE == 0`, so the raw `m_EHandle` at `+0x10` equals the ref handle — which
is exactly what the live probe observed (controller `m_hPlayerPawn` == the pawn's own raw `m_EHandle`).

---

## Static basis (vendored hl2sdk — the structs the shim already links)

- `CEntityInstance` (`entity2/entityinstance.h:68,160-168`): vtable (0x00), `CUtlSymbolLarge
  m_iszPrivateVScripts` (0x08, one `const char*` = 8 bytes, `tier1/utlsymbollarge.h:84`), **`CEntityIdentity*
  m_pEntity` (0x10)**, `m_hPrivateScope` (0x18), `m_pKeyValues` (0x20), `m_CScriptComponent` (0x28).
- `CEntityIdentity` (`entity2/entityidentity.h:71,100-124`): **`CEntityInstance* m_pInstance` (0x00)**,
  `CEntityClass* m_pClass` (0x08), **`CEntityHandle m_EHandle` (0x10)**, `int32 m_nameStringTableIndex`
  (0x14), `CUtlSymbolLarge m_name` (0x18) … `EntityFlags_t m_flags` (0x30).
- `CEntityHandle` (`entityhandle.h:59-67`): `union { uint32 m_Index; struct { uint32 m_EntityIndex : 15;
  uint32 m_Serial : 17; } m_Parts; }` → **15 index bits, 17 serial bits**.

**Why a live probe was still required:** `const.h` carries the *legacy* `NUM_ENT_ENTRY_BITS =
MAX_EDICT_BITS + 2 = 16` (const.h:63,75), which contradicts the actual `CEntityHandle` bitfield's 15
index bits. That 15-vs-16 ambiguity, plus the treadmill rule ("own your schema/offset layer rather than
trusting the SDK"), is exactly what the live probe disambiguates.

---

## Step 1 — reach `CEntityIdentity` from an entity pointer (CONFIRMED, `ENT_IDENTITY_PTR_OFFSET = 0x10`)

Verbose probe of the slot-0 controller (entity index 1):

```
SPIKE ---- idx=1 base=0x7f5a6d1e7800 ----
SPIKE base+0x00 = 0x7f5b2794c9a8          <- vtable (identical for idx=2: same class → same vtable)
SPIKE base+0x08 = 0x0                       <- m_iszPrivateVScripts (null)
SPIKE base+0x10 = 0x7f5b23518578          <- CEntityIdentity* (m_pEntity)
SPIKE identity(base+0x10) = 0x7f5b23518578
SPIKE identity+0x00 (m_pInstance back) = 0x7f5a6d1e7800  base-match=true   <- back-pointer == base
```

The `m_pInstance` back-pointer at `identity+0x00` reads back **exactly** the original entity pointer
(`base-match=true`) for both controllers (idx=1 → 0x7f5a6d1e7800, idx=2 → 0x7f5a6d1e9400). This is a
round-trip proof that `base+0x10` is the `CEntityIdentity*` and `identity+0x00` is `m_pInstance`.

## Step 2 — `CEntityHandle` bit-split (CONFIRMED, `HANDLE_ENTRY_BITS = 15`, `ENT_IDENTITY_HANDLE_OFFSET = 0x10`)

`m_EHandle` at `identity+0x10`, decoded both ways:

```
SPIKE m_EHandle(identity+0x10)=0xe1a68001  15[idx=1 ser=115533 match=true]   16[idx=32769 ser=57766 match=false]
SPIKE m_EHandle(identity+0x10)=0xe5a68002  15[idx=2 ser=117581 match=true]   16[idx=32770 ser=58790 match=false]
```

The 15-bit split yields the correct entity index (1, 2); the 16-bit split fails (only when the serial's
LSB is set — the decisive odd-serial case). Controller→pawn round-trip closes the loop:

```
SPIKE ctrl.m_hPlayerPawn(base+0xbbc)=0x459e82dc  15[idx=732 ser=35645]   (slot-0 controller)
SPIKE pawn idx=732 m_EHandle=0x459e82dc  ctrl-handle-match=true  15[idx=732 ser=35645]
SPIKE ctrl.m_hPlayerPawn(base+0xbbc)=0x340182e0  15[idx=736 ser=26627]   (slot-1 controller)
SPIKE pawn idx=736 m_EHandle=0x340182e0  ctrl-handle-match=true  15[idx=736 ser=26627]
```

The `m_hPlayerPawn` handle read from the controller (schema offset `0xbbc`, resolved live) decodes with
the **15-bit** index to a pawn entity whose **own** `m_EHandle` at `identity+0x10` is byte-for-byte equal
to the stored handle. The 16-bit index (33500/33504) is out of range and does not round-trip. This is
independent confirmation of both `ENT_IDENTITY_HANDLE_OFFSET = 0x10` and `HANDLE_ENTRY_BITS = 15`.

## Step 3 — validity on death/destruction/reuse (CONFIRMED)

Baseline (pre-event), direct probe of the two pawn entity indices:

```
SPIKE m_EHandle(identity+0x10)=0x459e82dc  15[idx=732 ser=35645 match=true]   (pawn @ index 732)
SPIKE m_EHandle(identity+0x10)=0x340182e0  15[idx=736 ser=26627 match=true]   (pawn @ index 736)
```

- `mp_restartgame 1` **reset the round but reused the same pawn entities** — serials unchanged
  (732→35645, 736→26627). (Recorded so the design knows a plain round-restart is *not* a destroy.)

- **`bot_kick` (true entity destruction)** — every probed index returns null:
  ```
  SPIKE idx=1: ent null    SPIKE idx=2: ent null
  SPIKE idx=732: ent null  SPIKE idx=736: ent null
  ```
  A stashed `EntityRef` whose entity was destroyed resolves to **null** (the shim's `ent_by_index`
  returns null on the freed/`EF_IS_INVALID_EHANDLE` slot). The server kept ticking.

- **Re-add bots (slot reuse with a fresh serial)** — controller indices 1 and 2 come back with the
  **serial incremented**:
  ```
  SPIKE m_EHandle(identity+0x10)=0xe1a70001  15[idx=1 ser=115534 ...]   (was ser=115533)
  SPIKE m_EHandle(identity+0x10)=0xe5a70002  15[idx=2 ser=117582 ...]   (was ser=117581)
  SPIKE ctrl.m_hPlayerPawn=0x43e483f9  15[idx=1017 ser=34761]   (new pawn; was idx=732)
  SPIKE ctrl.m_hPlayerPawn=0x1e1903fd  15[idx=1021 ser=15410]   (new pawn; was idx=736)
  ```
  A stale `EntityRef{index:1, serial:115533}` now **fails** the serial-equality check against the live
  `115534`, even though entity index 1 is re-occupied — proving the serial-compare catches the
  "different entity reused the index" case (not just outright destruction). New pawns landed at fresh
  indices (1017/1021) with fresh serials; the old pawn indices (732/736) stayed null.

Both invalidation modes the design relies on are confirmed: **destroyed → null**, and **slot reused →
serial changed → stale ref rejected.**

---

## GO / NO-GO

**GO.** All three constants are confirmed live, self-consistently, across multiple entities and two
invalidation paths:

- `ENT_IDENTITY_PTR_OFFSET = 0x10`
- `ENT_IDENTITY_HANDLE_OFFSET = 0x10`
- `HANDLE_ENTRY_BITS = 15` (index `& 0x7FFF`, serial `>> 15`); invalid sentinel `0xFFFFFFFF`.

The Slice-5A `(index, serial)` natives can build on: `ident = *(entity + 0x10)`; `live_eh = *(u32*)(ident
+ 0x10)`; `valid = (live_eh >> 15) == ref.serial` with the index in the low 15 bits, gated on a non-null
`ent_by_index(index)` (which already null-checks `EF_IS_INVALID_EHANDLE`). Per "layout is data, semantics
are code," these three offsets/bit-widths should be named engine-generic constants in `core` with a
`TODO(gamedata)` to migrate to a regenerable gamedata file when the treadmill tooling lands.

**Caveat for the load-bearing tasks:** a plain `mp_restartgame` does **not** destroy pawn entities
(serials persist); the live gate must use actual death/destruction (bot_kick, natural round death, or a
lethal `hurt`) to exercise the null/serial-change path.

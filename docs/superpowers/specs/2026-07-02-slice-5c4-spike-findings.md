# Slice 5C.4 — live spike + gate findings (pointer-chain `origin`/`angles`)

**Server:** Docker CS2 (`s2script-cs2`), de_inferno, `bot_quota 2`.
**Core:** sniper-built with the 5C.4 `__s2_ent_ref_read_floats_chain` native (`libs2script_core.so` GLIBC 2.30).
**Plugin:** `examples/demo-plugin` reading `pawn.origin` + `pawn.angles` via the pointer-chain accessors.

## The chain (confirmed live)

`pawn.origin` follows, entirely in-core, a two-pointer chain (offsets resolved live via `__s2_schema_offset`):

```
pawn (entity, serial-gated)
  └─ m_CBodyComponent (ptr on CBaseEntity)   ── deref
       └─ m_pSceneNode (ptr on CBodyComponent) ── deref
            └─ m_vecAbsOrigin (Vector on CGameSceneNode)  ── read 3 floats → Vector
            └─ m_angAbsRotation (QAngle on CGameSceneNode) ── read 3 floats → QAngle (pawn.angles)
```

The raw `CBodyComponent*`/`CGameSceneNode*` never cross to JS; the native derefs + reads + returns a copied
`{x,y,z}`.

## Spike — the origin reads a real de_inferno world position

Two bots at DISTINCT spawn points (CT vs T), sane world coordinates (not `(0,0,0)`, not garbage, not `null`):

```
[demo] tick 257 players=2
  slot=0 origin=Vector(-1662.18, 288.76, -63.97)  angles=QAngle(0, 77.5, 0)
  slot=1 origin=Vector(2353, 1977, 135.52)         angles=QAngle(0, 97.5, 0)
```

`origin` magnitudes are order ±1000..±2500 with a sensible `z` (−64..135) — plausible de_inferno map extents.
The two bots sit far apart (opposite spawns), which is exactly right. `angles` is a plausible body rotation
(pitch 0, yaw ~77–97, roll 0). This confirms the pointer chain end-to-end: the deref path + `m_vecAbsOrigin`
offset are correct.

## Disconnect degrade + liveness

`bot_kick` → `players=0` from the next demo tick (the 5C.2 occupancy filter drops the now-pawnless controllers;
a stored `Pawn`'s `origin` would read `null` via the native's root serial gate). The server kept ticking past
the kick (tick 1537 → 2817+), no crash, no segfault; rcon `status` → `0 bots, not hibernating`.

**Result:** spike + gate both PASS. The in-core pointer-chain read gives correct world positions live, degrades
safely, server stable.

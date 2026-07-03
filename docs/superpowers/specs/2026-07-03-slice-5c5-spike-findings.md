# Slice 5C.5 — live-gate findings (curated pointer-chain wrappers)

**Server:** Docker CS2 (`s2script-cs2`), de_inferno, `bot_quota 2`.
**Core:** sniper-built with the 5C.5 `__s2_ent_ref_read_chain` native.
**Plugin:** `examples/demo-plugin` reading fields behind pointer chains via the generated typed wrappers.

## Result: all four curated targets read correct through the chains, degrade on disconnect

Two bots, per-tick (before `bot_kick`):

```
[demo] tick 769 players=2
  slot=0 absOrigin=Vector(-1675.62, 351.70, -63.97) scale=1 activeWeapon=ref#1014 ducked=false origin(alias)=ok
  slot=1 absOrigin=Vector(2472.35, 2005.97, 134.51) scale=1 activeWeapon=ref#1013 ducked=false origin(alias)=ok
```

- **`pawn.sceneNode.absOrigin`** (Vector, via `CBaseEntity.m_CBodyComponent → CBodyComponent.m_pSceneNode →
  CGameSceneNode.m_vecAbsOrigin`): two DISTINCT plausible de_inferno spawn coords — matches 5C.4's origin. ✓
- **`pawn.sceneNode.scale`** (float, same chain, `m_flScale`): `1` (normal). ✓
- **`pawn.weaponServices.activeWeapon`** (via `m_pWeaponServices → CCSPlayer_WeaponServices.m_hActiveWeapon`, a
  handle → `EntityRef`): `ref#1014`/`ref#1013` — valid, distinct weapon entity refs (the bots' weapons). The
  `isValid()` guard on `readHandleVia` means these are LIVE refs, not dead handles. ✓
- **`pawn.movementServices.ducked`** (bool, via `m_pMovementServices → CCSPlayer_MovementServices.m_bDucked`):
  `false` (bots standing). ✓
- **`pawn.origin` (5C.4 compat alias → `pawn.sceneNode.absOrigin`)**: `ok`. ✓

The `aimPunchServices` target (not in the demo output) uses the identical curated-wrapper mechanism, exercised
by the other three.

## Boot-window safety (the T3-review fix, confirmed)

`applyNav` resolves each path hop's offset PER ACCESS (not baked at IIFE load), so it self-heals if the plugin
context is created before the schema is warm — the same discipline as `schema.generated.js` + 5C.4. All wrappers
read correct on the first observed tick post-boot (no permanent-null regression).

## Disconnect degrade + liveness

`bot_kick` → `players=0` from tick 4865 on (a wrapper on a stale root re-resolves the chain, serial-gated, →
`null`). The server kept ticking past the kick (tick 4097 → 5377+), no crash, no segfault; rcon `status` → `0
bots, not hibernating`.

**Result:** the curated pointer-chain codegen reads correct live through every chain, degrades safely on
disconnect, server stable.

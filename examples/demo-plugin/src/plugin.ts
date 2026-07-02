import { OnGameFrame, EntityRef } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

// A minimal @s2script/cs2-targeting plugin exercising the Slice 5A EntityRef guardrail.
// It STASHES a Pawn (an EntityRef-backed {index,serial}) once, then every ~256 frames reads
//   - the STASHED pawn's health: stays 100 while its entity lives; goes null the moment that
//     entity is destroyed (serial mismatch) — proving a held ref never dereferences a stale
//     pointer, closing the Slice-3 use-after-free.
//   - a FRESH Pawn.forSlot(0): recovers with the new serial once a pawn respawns / a bot rejoins.
// Slice 5B.2: also reads typed fields (float32/bool/handle) on the FRESH pawn via fresh.ref,
// demonstrating the kind-dispatched EntityRef typed methods over real CS2 schema fields.
// Built to a .s2sp by `npx s2script build`; dropped into addons/s2script/plugins/ to load.

// dev-facing: schema offsets resolved live (Slice 3); __s2_schema_offset walks base classes,
// so fields inherited by CCSPlayerPawn from any ancestor class resolve correctly.
declare const __s2_schema_offset: (cls: string, field: string) => number;

let stashed: Pawn | null = null;
let ticks = 0;

// Resolved once after the schema is warm (first live pawn); -1 means not yet resolved.
let FRICTION_OFF = -1;   // m_flFriction  (float32, on CBaseEntity)
let RAGDOLL_OFF  = -1;   // m_bClientSideRagdoll (bool, on CBaseEntity)
let OWNER_OFF    = -1;   // m_hOwnerEntity (handle → CBaseEntity, on CBaseEntity)

export function onLoad(): void {
  console.log("[demo] onLoad");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    if (!stashed) stashed = Pawn.forSlot(0);        // stash once
    const fresh = Pawn.forSlot(0);

    // --- Slice 5B.2: lazily resolve offsets once the schema is warm, then read typed fields ---
    if (fresh) {
      if (FRICTION_OFF < 0) FRICTION_OFF = __s2_schema_offset("CCSPlayerPawn", "m_flFriction");
      if (RAGDOLL_OFF  < 0) RAGDOLL_OFF  = __s2_schema_offset("CCSPlayerPawn", "m_bClientSideRagdoll");
      if (OWNER_OFF    < 0) OWNER_OFF    = __s2_schema_offset("CCSPlayerPawn", "m_hOwnerEntity");
    }

    const friction: number | null  = (fresh && FRICTION_OFF >= 0) ? fresh.ref.readFloat32(FRICTION_OFF) : null;
    const ragdoll: boolean | null  = (fresh && RAGDOLL_OFF  >= 0) ? fresh.ref.readBool(RAGDOLL_OFF)     : null;
    // readHandle decodes the CEntityHandle field and returns a live, serial-gated EntityRef (or null):
    const owner: EntityRef | null  = (fresh && OWNER_OFF    >= 0) ? fresh.ref.readHandle(OWNER_OFF)     : null;
    const ownerInfo = owner
      ? ("idx=" + owner.index + " valid=" + owner.isValid()) : "null";

    console.log("[demo] tick " + ticks
      + " stashed.health=" + (stashed ? stashed.health : "none")   // null once that pawn died
      + " fresh.health="   + (fresh   ? fresh.health   : "none")   // works again after respawn
      + " friction="  + friction
      + " ragdoll="   + ragdoll
      + " owner="     + ownerInfo);

    if (stashed && stashed.health === null) { stashed = null; }     // re-stash next tick
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}

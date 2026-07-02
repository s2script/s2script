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
let FRICTION_OFF   = -1; // m_flFriction  (float32, on CBaseEntity)
let RAGDOLL_OFF    = -1; // m_bClientSideRagdoll (bool, on CBaseEntity)
let CONTROLLER_OFF = -1; // m_hController (handle → CBasePlayerController, on CBasePlayerPawn) — always set on a live pawn
let PLAYERPAWN_OFF = -1; // m_hPlayerPawn (handle → CCSPlayerPawn, on CCSPlayerController) — read back THROUGH the controller ref

export function onLoad(): void {
  console.log("[demo] onLoad");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    if (!stashed) stashed = Pawn.forSlot(0);        // stash once
    const fresh = Pawn.forSlot(0);

    // --- Slice 5B.2: lazily resolve offsets once the schema is warm, then read typed fields ---
    if (fresh) {
      if (FRICTION_OFF   < 0) FRICTION_OFF   = __s2_schema_offset("CCSPlayerPawn", "m_flFriction");
      if (RAGDOLL_OFF    < 0) RAGDOLL_OFF    = __s2_schema_offset("CCSPlayerPawn", "m_bClientSideRagdoll");
      if (CONTROLLER_OFF < 0) CONTROLLER_OFF = __s2_schema_offset("CCSPlayerPawn", "m_hController");
      if (PLAYERPAWN_OFF < 0) PLAYERPAWN_OFF = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    }

    const friction: number | null  = (fresh && FRICTION_OFF >= 0) ? fresh.ref.readFloat32(FRICTION_OFF) : null;
    const ragdoll: boolean | null  = (fresh && RAGDOLL_OFF  >= 0) ? fresh.ref.readBool(RAGDOLL_OFF)     : null;
    // readHandle decodes the CEntityHandle field into a live, serial-gated EntityRef (or null).
    // Then CHAIN a read THROUGH that ref: the controller's m_hPlayerPawn handle back to the pawn —
    // proving the handle-derived ref is not just data but a usable, live EntityRef.
    const ctrl: EntityRef | null   = (fresh && CONTROLLER_OFF >= 0) ? fresh.ref.readHandle(CONTROLLER_OFF) : null;
    let ctrlInfo = "null";
    if (ctrl) {
      const pawnBack: EntityRef | null = PLAYERPAWN_OFF >= 0 ? ctrl.readHandle(PLAYERPAWN_OFF) : null;
      ctrlInfo = "idx=" + ctrl.index + " valid=" + ctrl.isValid()
        + " pawnBack=" + (pawnBack ? ("idx=" + pawnBack.index + " valid=" + pawnBack.isValid()) : "null");
    }

    console.log("[demo] tick " + ticks
      + " stashed.health=" + (stashed ? stashed.health : "none")   // null once that pawn died
      + " fresh.health="   + (fresh   ? fresh.health   : "none")   // works again after respawn
      + " friction="  + friction
      + " ragdoll="   + ragdoll
      + " controller=" + ctrlInfo);

    if (stashed && stashed.health === null) { stashed = null; }     // re-stash next tick
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}

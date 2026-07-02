import { OnGameFrame } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

// A minimal @s2script/cs2 plugin exercising the Slice 5B.3 GENERATED typed field accessors.
// Every field below is a generated property on Pawn (from `s2script gen-schema` over the schema
// catalog) — the author writes `pawn.health` / `pawn.friction`, NOT the raw
// `__s2_schema_offset(...)` + `pawn.ref.readFloat32(...)` plumbing (that is now internal to the
// generated getters/setters, which resolve offsets live and stay serial-gated T | null).
// It also STASHES a Pawn once and reads its generated `health` every ~256 frames: stays 100 while
// the entity lives, goes null the moment it's destroyed (serial mismatch) — the Slice-5A guardrail,
// now exercised through a generated accessor. Built to a .s2sp by `s2script build`.
let stashed: Pawn | null = null;
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad (generated accessors)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    if (!stashed) stashed = Pawn.forSlot(0);        // stash once
    const p = Pawn.forSlot(0);

    // Generated typed accessors — no offsets, no ref.read* in author code:
    const controller = p ? p.controller : null;      // generated handle accessor → EntityRef | null
    console.log("[demo] tick " + ticks
      + " health=" + (p ? p.health : "none")                 // generated (m_iHealth, int32)
      + " friction=" + (p ? p.friction : "none")             // generated (m_flFriction, float32)
      + " ragdoll=" + (p ? p.clientSideRagdoll : "none")     // generated (m_bClientSideRagdoll, bool)
      + " team=" + (p ? p.teamNum : "none")                  // generated (m_iTeamNum)
      + " controller=" + (controller ? ("idx=" + controller.index + " valid=" + controller.isValid()) : "null")
      + " stashed.health=" + (stashed ? stashed.health : "none"));  // null once that pawn dies

    if (stashed && stashed.health === null) { stashed = null; }     // re-stash next tick
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}

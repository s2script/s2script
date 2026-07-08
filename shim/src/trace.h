#pragma once
// Ray-trace structs + the CNavPhysicsInterface::TraceShape call (ray-trace slice, Task 1).
//
// ENGINE-GENERIC (Source-2 physics) — nothing in this file names a CS2 class or field. Ray_t,
// CTraceFilter, CGameTrace, RnQueryShapeAttr_t / InteractionLayers-shaped masks are all vendored
// hl2sdk Source-2 SDK types (public/gametrace.h, which pulls in public/cmodel.h for Ray_t) —
// exactly the types the reference project (FUNPLAY-pro-CS2/Ray-Trace) uses. CNavPhysicsInterface
// itself is a Source-2 physics-query interface (not a CS2 game class); it is resolved by RTTI
// name (s2vtable::GetVTableByName), never hardcoded as a vtable index in code — the index is
// gamedata.
#include <gametrace.h>   // Ray_t (via cmodel.h), CTraceFilter, CGameTrace, CEntityInstance, Vector

#include <cstdint>

namespace s2trace {

// ---------------------------------------------------------------------------
// InteractionLayers masks (ported from the reference project's public/craytraceinterface.h,
// itself derived from the same LAYER_INDEX_CONTENTS_* bit positions hl2sdk's public/const.h +
// public/bspflags.h already define for CS2's Source-2 physics layers). Individual bits first,
// then the named composite masks @s2script/trace exposes as `TraceMask` (Task 2 copies these
// numeric values into JS — kept here too as the typed C++ source of truth / for any future
// shim-side C++ caller).
// ---------------------------------------------------------------------------
constexpr uint64_t kLayerSolid       = 1ull << 0;
constexpr uint64_t kLayerHitboxes    = 1ull << 1;
constexpr uint64_t kLayerPlayerClip  = 1ull << 4;
constexpr uint64_t kLayerNPCClip     = 1ull << 5;
constexpr uint64_t kLayerWindow      = 1ull << 12;
constexpr uint64_t kLayerPassBullets = 1ull << 13;
constexpr uint64_t kLayerPlayer      = 1ull << 18;
constexpr uint64_t kLayerNPC         = 1ull << 19;
constexpr uint64_t kLayerPhysicsProp = 1ull << 21;

// Bullet trace against solid world + player-clip + windows + players/NPCs/physics props — the
// reference project's default ("custom base 0x2C3011").
constexpr uint64_t kMaskShotPhysics = kLayerSolid | kLayerPlayerClip | kLayerWindow |
                                      kLayerPassBullets | kLayerPlayer | kLayerNPC | kLayerPhysicsProp;
// Hitboxes only (headshot-style detection).
constexpr uint64_t kMaskShotHitbox  = kLayerHitboxes | kLayerPlayer | kLayerNPC;
// Physics + hitboxes (a full bullet trace, world geometry AND hitbox precision).
constexpr uint64_t kMaskShotFull    = kMaskShotPhysics | kLayerHitboxes;
// World geometry only, no entities at all.
constexpr uint64_t kMaskWorldOnly   = kLayerSolid | kLayerWindow | kLayerPassBullets;
// Grenade trajectory trace (world + physics props, no players).
constexpr uint64_t kMaskGrenade     = kLayerSolid | kLayerWindow | kLayerPhysicsProp | kLayerPassBullets;
// Brushes only (no clip volumes, no entities).
constexpr uint64_t kMaskBrushOnly   = kLayerSolid | kLayerWindow;
// Player movement collision (world + player-clip).
constexpr uint64_t kMaskPlayerMove  = kLayerSolid | kLayerWindow | kLayerPlayerClip | kLayerPassBullets;
// NPC movement collision (world + npc-clip).
constexpr uint64_t kMaskNPCMove     = kLayerSolid | kLayerWindow | kLayerNPCClip | kLayerPassBullets;

// Cross-checks the bit positions above against the reference project's own static_assert
// (craytraceinterface.h: `static_assert(MASK_SHOT_PHYSICS == 0x2c3011, ...)`) — if this ever
// fails, a bit position was transcribed wrong.
static_assert(kMaskShotPhysics == 0x2c3011ull, "kMaskShotPhysics must equal the reference value 0x2c3011");

// ---------------------------------------------------------------------------
// CTraceFilterEx: a CTraceFilter with explicit mask fields + an optional single ignore entity.
// CTraceFilter itself has no constructor overload that takes "explicit interacts-with/exclude
// masks AND a single ignore entity" together, so this thin subclass supplies it. Defined inline
// (header-only) — a 3-field wrapper with no out-of-line logic, mirroring the reference project's
// CTraceFilterEx in src/raytrace.h.
// ---------------------------------------------------------------------------
class CTraceFilterEx : public CTraceFilter {
public:
    CTraceFilterEx(uint64_t interactsWith, uint64_t interactsExclude, CEntityInstance* ignoreEnt)
        : CTraceFilter(interactsWith, COLLISION_GROUP_DEFAULT, /*bIterateEntities=*/true) {
        m_nInteractsAs      = 0;
        m_nInteractsWith    = interactsWith;
        m_nInteractsExclude = interactsExclude;
        if (ignoreEnt) SetPassEntity1(ignoreEnt);
    }
};

// ---------------------------------------------------------------------------
// The trace-call result, shaped identically to shim/include/s2script_core.h's S2TraceResult (kept
// as a separate type so this engine-generic helper doesn't need to depend on the shim<->core ABI
// header; s2script_mm.cpp's op wrapper copies field-by-field). NOTE: the exact CGameTrace field
// offsets consumed here (m_vEndPos/m_vHitNormal/m_flFraction/m_bStartInSolid/m_pEnt) come straight
// from the vendored hl2sdk's public/gametrace.h struct layout — LIVE-VALIDATED by the ray-trace
// slice's live gate (the controller's job); a hl2sdk update that reorders CGameTrace's fields
// would need this struct (and the vendored header) regenerated/re-pinned.
// ---------------------------------------------------------------------------
struct S2TraceResultOut {
    int      didHit;
    float    fraction;
    float    endpos[3];
    float    normal[3];
    int      allSolid;
    int      hitEntHandle;   // GetRefEHandle().ToInt() of the hit entity, or -1
};

// Function-pointer type matching CNavPhysicsInterface::TraceShape's ABI (recon-confirmed against
// the reference project's disassembly-derived call site in src/raytrace.cpp):
//   bool TraceShape(void* /*this=nullptr*/, Ray_t& ray, Vector& start, Vector& end,
//                   CTraceFilter* filter, CGameTrace* trace);
using TraceShapeFn = bool (*)(void*, Ray_t&, Vector&, Vector&, CTraceFilter*, CGameTrace*);

// Perform one trace call through the resolved TraceShape function pointer. `fn` must be non-null
// and TraceShape-shaped (the caller — s2script_mm.cpp's Load() — resolves + validates it via
// s2vtable::GetVTableByName + the .text-membership check before ever handing it here). `ignoreEnt`
// may be null (no entity excluded). Builds a line trace when mins==maxs (Ray_t::Init collapses to
// RAY_TYPE_LINE), else a hull/box trace. Returns false (does not touch *out) only if `fn` is null;
// the underlying engine call itself has no failure return — a "miss" is a normal CGameTrace with
// DidHit()==false, which still produces `out->didHit == 0`.
bool RunTraceShape(TraceShapeFn fn,
                    const float start[3], const float end[3],
                    const float mins[3], const float maxs[3],
                    uint64_t interactsWith, uint64_t interactsExclude,
                    CEntityInstance* ignoreEnt,
                    S2TraceResultOut* out);

} // namespace s2trace

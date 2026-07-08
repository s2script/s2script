#include "trace.h"

namespace s2trace {

bool RunTraceShape(TraceShapeFn fn,
                    const float start[3], const float end[3],
                    const float mins[3], const float maxs[3],
                    uint64_t interactsWith, uint64_t interactsExclude,
                    CEntityInstance* ignoreEnt,
                    S2TraceResultOut* out) {
    if (!fn || !out) return false;

    Vector vMins(mins[0], mins[1], mins[2]);
    Vector vMaxs(maxs[0], maxs[1], maxs[2]);
    Ray_t ray;
    ray.Init(vMins, vMaxs);   // mins==maxs collapses to RAY_TYPE_LINE (Ray_t::Init); else a hull

    // Mutable copies: the engine call takes start/end BY REFERENCE (recon-confirmed ABI) and some
    // TraceShape implementations may write back into them; the caller's arrays must not alias.
    Vector vStart(start[0], start[1], start[2]);
    Vector vEnd(end[0], end[1], end[2]);

    CTraceFilterEx filter(interactsWith, interactsExclude, ignoreEnt);
    CGameTrace trace;

    fn(/*this=*/nullptr, ray, vStart, vEnd, &filter, &trace);

    out->didHit  = trace.DidHit() ? 1 : 0;
    out->fraction = trace.m_flFraction;
    out->endpos[0] = trace.m_vEndPos.x;
    out->endpos[1] = trace.m_vEndPos.y;
    out->endpos[2] = trace.m_vEndPos.z;
    out->normal[0] = trace.m_vHitNormal.x;
    out->normal[1] = trace.m_vHitNormal.y;
    out->normal[2] = trace.m_vHitNormal.z;
    out->allSolid  = trace.m_bStartInSolid ? 1 : 0;
    out->hitEntHandle = trace.m_pEnt ? trace.m_pEnt->GetRefEHandle().ToInt() : -1;
    return true;
}

} // namespace s2trace

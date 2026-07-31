// Entity lifecycle listeners slice — the ONLY TU that includes entity2/entitysystem.h (for
// IEntityListener). Mirrors ekv.cpp's isolation discipline: everything else in the shim treats the
// listener as an opaque void* (via S2_GetEntityListener()). We DERIVE the real IEntityListener so the
// vtable layout matches the SDK exactly (create/spawn/delete/parentChanged in that order); we do NOT
// call m_entityListeners.AddToTail here — registration is a sig-resolved AddListenerEntity call in
// s2script_mm.cpp, so this TU instantiates no heavy CUtlVector template.
#include <entity2/entitysystem.h>       // IEntityListener
// entitysystem.h only forward-declares CEntityInstance (via entityidentity.h); GetClassname()/
// GetRefEHandle() need the full definition — mirrors s2script_mm.cpp, which includes both headers
// explicitly for the same reason.
#include <entity2/entityinstance.h>     // CEntityInstance::GetClassname/GetRefEHandle
#include <cstdio>                     // printf — opt-in lifecycle trace
#include <cstdlib>                    // getenv
#include "s2script_core.h"              // s2script_core_dispatch_entity_event (Task 1 core export)

// deferred-dispatch-queue slice: the queue itself lives in s2script_mm.cpp (it owns the payload and
// the drain). This TU sees exactly one function of it — the same opaque-boundary discipline as
// S2_GetEntityListener below, so the entity listener never learns the queue's tagged union.
extern "C" void S2_DeferEntityEvent(const char* kind, const char* className, int handle);

namespace {
class S2EntityListener : public IEntityListener {
public:
    void OnEntityCreated(CEntityInstance* pEntity) override { fire("create", pEntity); }
    void OnEntitySpawned(CEntityInstance* pEntity) override { fire("spawn",  pEntity); }
    void OnEntityDeleted(CEntityInstance* pEntity) override { fire("delete", pEntity); }
    void OnEntityParentChanged(CEntityInstance*, CEntityInstance*) override {}  // not exposed to JS (YAGNI)
private:
    static void fire(const char* kind, CEntityInstance* pEntity) {
        if (!pEntity) return;
        const char* cls = pEntity->GetClassname();          // designer name; valid at create/spawn/delete
        int handle = pEntity->GetRefEHandle().ToInt();       // packed CEntityHandle — the shim's handle idiom
        // OPT-IN LIFECYCLE TRACE (S2_ENTITY_TRACE), for attributing a client fatal to a real entity.
        //
        // `CopyExistingEntity: missing client entity N` names an entity INDEX, and a plugin-side trace
        // can only ever log the entities that plugin creates — every crash index captured so far fell
        // in the gaps between those, so none of them could be attributed to anything. This logs EVERY
        // create/spawn/delete with index and classname, which is what turns an index into an occupant.
        //
        // ABOVE the dispatch, deliberately. Since the deferred-dispatch queue, a re-entrant event is
        // delivered to JS a frame LATE, so dispatch order no longer tracks engine order. Forensics
        // needs the moment the engine actually created or destroyed the entity — the index/serial
        // recycle we are chasing happens on the engine's clock, not on the drain's. Tracing here also
        // keeps the trace honest about events that defer, which are exactly the ones a plugin-side
        // trace could never see.
        //
        // Off unless the env var is set, and bounded, so a forgotten variable cannot flood a log.
        static const bool on = (getenv("S2_ENTITY_TRACE") != nullptr);
        static int left = 20000;
        if (on && left > 0) {
            --left;
            printf("[s2script] ENTLIFE %s index=%d class=%s\n", kind, handle & 0x7fff, cls ? cls : "?");
            if (left == 0) printf("[s2script] ENTLIFE budget spent — tracing off\n");
        }
        // A re-entrant dispatch (this entity was created/deleted BY a JS handler, e.g. a plugin's own
        // synchronous createEntity) delivers nothing — queue the scalars for the next GameFrame
        // instead of dropping it silently. GetClassname() points into engine memory that may be gone
        // by then, so the queue copies it; `handle` is a packed CEntityHandle, never a pointer.
        if (s2script_core_dispatch_entity_event(kind, cls ? cls : "", handle) == S2_DISPATCH_DEFERRED)
            S2_DeferEntityEvent(kind, cls ? cls : "", handle);
    }
};
S2EntityListener g_entityListener;   // static; lives for the process — its address is a stable IEntityListener*
}  // namespace

// Opaque accessor for s2script_mm.cpp (which never includes entitysystem.h): the IEntityListener* to
// pass to the sig-resolved AddListenerEntity/RemoveListenerEntity.
extern "C" void* S2_GetEntityListener() { return &g_entityListener; }

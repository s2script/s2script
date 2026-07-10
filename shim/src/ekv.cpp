// EKV slice — the ONLY TU in the shim that includes entity2/entitykeyvalues.h. Everything else
// (s2script_mm.cpp included) sees the CEntityKeyValues* it builds only as a void*, via ekv.h.
//
// GameEntitySystem(): entity2/entitysystem.h declares this `extern CGameEntitySystem*
// GameEntitySystem();` and expects the CONSUMER to define it (CSSharp defines its own). We bridge
// to the shim's existing per-call GetEntitySystem() resolver via a small non-static accessor added
// to s2script_mm.cpp (S2_EntitySystemBridge) — declared here, defined there. Null-safe: the
// EKV_ALLOCATOR_NORMAL paths this slice uses only ever CHECK it (see the ctor), never require it —
// a load-time (pre-map) EKV build/self-test works fine with it null.
#include "ekv.h"
#include <entity2/entitykeyvalues.h>
#include <cstdlib>
#include <cstring>

CGameEntitySystem* S2_EntitySystemBridge();

CGameEntitySystem* GameEntitySystem() { return S2_EntitySystemBridge(); }

void* S2EKV_Build(int count, const char* const* keys, const int* types, const char* const* values) {
    if (count < 0 || (count > 0 && (!keys || !types || !values))) return nullptr;
    CEntityKeyValues* kv = new CEntityKeyValues();   // NULL arena + EKV_ALLOCATOR_NORMAL (CSSharp shape)
    for (int i = 0; i < count; ++i) {
        if (!keys[i] || !keys[i][0] || !values[i]) { delete kv; return nullptr; }
        switch (types[i]) {
            case 0: kv->SetString(keys[i], values[i]); break;
            case 1: kv->SetInt(keys[i], (int)strtol(values[i], nullptr, 10)); break;
            case 2: kv->SetFloat(keys[i], strtof(values[i], nullptr)); break;
            case 3: kv->SetBool(keys[i], values[i][0] == '1'); break;
            default: delete kv; return nullptr;
        }
    }
    return kv;
}

void S2EKV_AddRef(void* ekv) { static_cast<CEntityKeyValues*>(ekv)->AddRef(); }

int S2EKV_ReleaseIfSafe(void* ekv) {
    CEntityKeyValues* kv = static_cast<CEntityKeyValues*>(ekv);
    if (kv->IsQueuedForSpawn()) return 0;   // engine still holds it — caller WARNs once + leaks by design
    kv->Release();                           // our AddRef held it at >=1; this hits 0 -> OUR delete, OUR heap
    return 1;
}

bool S2EKV_SelfTest() {
    CEntityKeyValues kv;                     // stack; dtor runs at scope exit
    kv.SetInt("s2_k1", 42);
    kv.SetString("s2_k2", "ekv");
    kv.SetBool("s2_k3", true);
    kv.SetFloat("s2_k4", 1.5f);
    return kv.GetInt("s2_k1") == 42 && strcmp(kv.GetString("s2_k2"), "ekv") == 0
        && kv.GetBool("s2_k3") && kv.GetFloat("s2_k4") == 1.5f;
}

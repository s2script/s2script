// Per-entity SDKHooks VP hooks — KHook::Virtual::Add, not process-wide detours.
//
// Touch family: `void (CEntityInstance *pOther)`.
// Lifecycle: this-void (Spawn/Think/PreThink/PostThink/VPhysicsUpdate/GroundEntChangedPost),
// Use `(CEntityInstance *activator, CEntityInstance *caller, int useType, float value)`,
// GetMaxHealth `int()`, ShouldCollide `bool(int,int)`, CanBeAutobalanced `bool()`.
// Slots are derived at Load from gamedata/sdkhooks signatures + vtable-member, never shipped
// as borrowed offsets. Missing/failed rows leave the type unconfigured: s2_sdkhook_vp_add
// returns 0 and SDKHook returns false.
#include "sdkhooks_vp.h"

#include "s2script_core.h"
#include "gamedata.h"
#include "sigscan.h"
#include "vtable.h"
#include "call_validate.h"
#include "engine_calls.h"

#include <ISmmPlugin.h>
#include <cstdio>
#include <entity2/entityinstance.h>

PLUGIN_GLOBALVARS();

#include <khook.hpp>
#include "khook_map.h"

#include <link.h>
#include <cstring>
#include <map>
#include <string>
#include <vector>

#include "../third_party/json.hpp"

// Wiki type names — the gamedata keys AND the strings core passes as `type`. Must stay
// quoted here so scripts/check-gamedata-owners.sh sees the extension owner name them.
static const char* kStartTouch             = "StartTouch";
static const char* kTouch                  = "Touch";
static const char* kEndTouch               = "EndTouch";
static const char* kBlocked                = "Blocked";
static const char* kSpawn                  = "Spawn";
static const char* kThink                  = "Think";
static const char* kPreThink               = "PreThink";
static const char* kPostThink              = "PostThink";
static const char* kUse                    = "Use";
static const char* kGetMaxHealth           = "GetMaxHealth";
static const char* kShouldCollide          = "ShouldCollide";
static const char* kVPhysicsUpdate         = "VPhysicsUpdate";
static const char* kGroundEntChangedPost   = "GroundEntChangedPost";
static const char* kCanBeAutobalanced      = "CanBeAutobalanced";

static KHook::Return<void> Hook_StartTouch(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_StartTouchPost(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_Touch(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_TouchPost(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_EndTouch(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_EndTouchPost(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_Blocked(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_BlockedPost(CEntityInstance* thisPtr, CEntityInstance* pOther);
static KHook::Return<void> Hook_Spawn(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_SpawnPost(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_Think(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_ThinkPost(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_PreThink(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_PreThinkPost(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_PostThink(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_PostThinkPost(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_VPhysicsUpdate(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_VPhysicsUpdatePost(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_GroundEntChangedPost(CEntityInstance* thisPtr);
static KHook::Return<void> Hook_Use(CEntityInstance* thisPtr, CEntityInstance* act,
                                      CEntityInstance* caller, int useType, float value);
static KHook::Return<void> Hook_UsePost(CEntityInstance* thisPtr, CEntityInstance* act,
                                          CEntityInstance* caller, int useType, float value);
static KHook::Return<int> Hook_GetMaxHealth(CEntityInstance* thisPtr);
static KHook::Return<bool> Hook_ShouldCollide(CEntityInstance* thisPtr, int collisionGroup,
                                             int contentsMask);
static KHook::Return<bool> Hook_CanBeAutobalanced(CEntityInstance* thisPtr);

namespace {
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch(
    &Hook_StartTouch, &Hook_StartTouchPost);
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkTouch(&Hook_Touch, &Hook_TouchPost);
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkEndTouch(
    &Hook_EndTouch, &Hook_EndTouchPost);
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkBlocked(
    &Hook_Blocked, &Hook_BlockedPost);
S2CheckedVirtual<CEntityInstance, void> g_hkSpawn(&Hook_Spawn, &Hook_SpawnPost);
S2CheckedVirtual<CEntityInstance, void> g_hkThink(&Hook_Think, &Hook_ThinkPost);
S2CheckedVirtual<CEntityInstance, void> g_hkPreThink(&Hook_PreThink, &Hook_PreThinkPost);
S2CheckedVirtual<CEntityInstance, void> g_hkPostThink(&Hook_PostThink, &Hook_PostThinkPost);
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*, CEntityInstance*, int, float> g_hkUse(
    &Hook_Use, &Hook_UsePost);
S2CheckedVirtual<CEntityInstance, int> g_hkGetMaxHealth(&Hook_GetMaxHealth, nullptr);
S2CheckedVirtual<CEntityInstance, bool, int, int> g_hkShouldCollide(&Hook_ShouldCollide, nullptr);
S2CheckedVirtual<CEntityInstance, void> g_hkVPhysicsUpdate(
    &Hook_VPhysicsUpdate, &Hook_VPhysicsUpdatePost);
S2CheckedVirtual<CEntityInstance, void> g_hkGroundEntChanged(nullptr, &Hook_GroundEntChangedPost);
S2CheckedVirtual<CEntityInstance, bool> g_hkCanBeAutobalanced(&Hook_CanBeAutobalanced, nullptr);

std::array<S2CheckedBindingOps*, 14> SdkhookBindingInventory() {
    return {{
        &g_hkStartTouch, &g_hkTouch, &g_hkEndTouch, &g_hkBlocked,
        &g_hkSpawn, &g_hkThink, &g_hkPreThink, &g_hkPostThink,
        &g_hkUse, &g_hkGetMaxHealth, &g_hkShouldCollide, &g_hkVPhysicsUpdate,
        &g_hkGroundEntChanged, &g_hkCanBeAutobalanced,
    }};
}

constexpr int kMaxVtableSlots = 512;

struct ModText {
    const uint8_t* text = nullptr;
    size_t         size = 0;
    const uint8_t* lo   = nullptr;
    const uint8_t* hi   = nullptr;
};

ModText FindModuleText(const char* soname) {
    struct Ctx { const char* name; size_t bestX; ModText out; } ctx{ soname, 0, {} };
    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto* c = static_cast<Ctx*>(data);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) return 0;
        size_t maxX = 0;
        const uint8_t* text = nullptr;
        ElfW(Addr) lo = ~static_cast<ElfW(Addr)>(0), hi = 0;
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type != PT_LOAD) continue;
            if ((ph.p_flags & PF_X) && ph.p_filesz > maxX) {
                maxX = ph.p_filesz;
                text = reinterpret_cast<const uint8_t*>(info->dlpi_addr + ph.p_vaddr);
            }
            if (ph.p_vaddr < lo) lo = ph.p_vaddr;
            if (ph.p_vaddr + ph.p_memsz > hi) hi = ph.p_vaddr + ph.p_memsz;
        }
        if (maxX > c->bestX) {
            c->bestX    = maxX;
            c->out.text = text;
            c->out.size = maxX;
            c->out.lo   = reinterpret_cast<const uint8_t*>(info->dlpi_addr + lo);
            c->out.hi   = reinterpret_cast<const uint8_t*>(info->dlpi_addr + hi);
        }
        return 0;
    }, &ctx);
    return ctx.out;
}

bool InModuleText(const ModText& mt, const void* fn) {
    if (!mt.text || !fn) return false;
    const uint8_t* p = static_cast<const uint8_t*>(fn);
    return p >= mt.text && p < mt.text + mt.size;
}

enum class Kind {
    StartTouch,
    Touch,
    EndTouch,
    Blocked,
    Spawn,
    Think,
    PreThink,
    PostThink,
    Use,
    GetMaxHealth,
    ShouldCollide,
    VPhysicsUpdate,
    GroundEntChanged,
    CanBeAutobalanced,
};
constexpr int kKindCount = static_cast<int>(Kind::CanBeAutobalanced) + 1;

bool ParseKind(const char* type, Kind* out) {
    if (!type || !out) return false;
    if (std::strcmp(type, kStartTouch) == 0)           { *out = Kind::StartTouch;        return true; }
    if (std::strcmp(type, kTouch) == 0)                { *out = Kind::Touch;             return true; }
    if (std::strcmp(type, kEndTouch) == 0)             { *out = Kind::EndTouch;          return true; }
    if (std::strcmp(type, kBlocked) == 0)             { *out = Kind::Blocked;           return true; }
    if (std::strcmp(type, kSpawn) == 0)                { *out = Kind::Spawn;             return true; }
    if (std::strcmp(type, kThink) == 0)                { *out = Kind::Think;             return true; }
    if (std::strcmp(type, kPreThink) == 0)             { *out = Kind::PreThink;          return true; }
    if (std::strcmp(type, kPostThink) == 0)            { *out = Kind::PostThink;          return true; }
    if (std::strcmp(type, kUse) == 0)                  { *out = Kind::Use;               return true; }
    if (std::strcmp(type, kGetMaxHealth) == 0)         { *out = Kind::GetMaxHealth;      return true; }
    if (std::strcmp(type, kShouldCollide) == 0)        { *out = Kind::ShouldCollide;     return true; }
    if (std::strcmp(type, kVPhysicsUpdate) == 0)        { *out = Kind::VPhysicsUpdate;     return true; }
    if (std::strcmp(type, kGroundEntChangedPost) == 0) { *out = Kind::GroundEntChanged;  return true; }
    if (std::strcmp(type, kCanBeAutobalanced) == 0)    { *out = Kind::CanBeAutobalanced; return true; }
    return false;
}

int s_slot[kKindCount] = {
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1
};

void ClearSlots() {
    for (int i = 0; i < kKindCount; i++) s_slot[i] = -1;
}

struct VpKey {
    void* ptr;
    Kind  kind;
    int   post;   // 0 pre, 1 post
    bool operator<(const VpKey& o) const {
        if (ptr != o.ptr) return ptr < o.ptr;
        if (kind != o.kind) return static_cast<int>(kind) < static_cast<int>(o.kind);
        return post < o.post;
    }
};
struct VpInst {
    int refcount = 0;
    int index = 0;
    int serial = 0;
};
std::map<VpKey, VpInst> g_installed;

int PackEnt(CEntityInstance* p) {
    return p ? static_cast<int>(p->GetRefEHandle().ToInt()) : -1;
}

int DispatchTouch(const char* wiki, int post, CEntityInstance* self, CEntityInstance* other) {
    if (!self) return 0;
    CEntityHandle h = self->GetRefEHandle();
    return s2script_core_dispatch_sdkhook_touch(
        h.GetEntryIndex(), h.GetSerialNumber(), PackEnt(other), post, wiki);
}

int DispatchThis(const char* wiki, int post, CEntityInstance* self) {
    if (!self) return 0;
    CEntityHandle h = self->GetRefEHandle();
    return s2script_core_dispatch_sdkhook_this(
        h.GetEntryIndex(), h.GetSerialNumber(), post, wiki);
}

int DispatchUse(const char* wiki, int post, CEntityInstance* self,
                CEntityInstance* act, CEntityInstance* caller, int useType, float value) {
    if (!self) return 0;
    CEntityHandle h = self->GetRefEHandle();
    return s2script_core_dispatch_sdkhook_use(
        h.GetEntryIndex(), h.GetSerialNumber(), PackEnt(act), PackEnt(caller),
        useType, value, post, wiki);
}

static S2HookReceipt VpAddThis(Kind kind, void* p) {
    auto* ent = static_cast<CEntityInstance*>(p);
    switch (kind) {
    case Kind::StartTouch:        return g_hkStartTouch.Add(ent);
    case Kind::Touch:             return g_hkTouch.Add(ent);
    case Kind::EndTouch:          return g_hkEndTouch.Add(ent);
    case Kind::Blocked:           return g_hkBlocked.Add(ent);
    case Kind::Spawn:             return g_hkSpawn.Add(ent);
    case Kind::Think:             return g_hkThink.Add(ent);
    case Kind::PreThink:          return g_hkPreThink.Add(ent);
    case Kind::PostThink:         return g_hkPostThink.Add(ent);
    case Kind::Use:               return g_hkUse.Add(ent);
    case Kind::GetMaxHealth:      return g_hkGetMaxHealth.Add(ent);
    case Kind::ShouldCollide:     return g_hkShouldCollide.Add(ent);
    case Kind::VPhysicsUpdate:    return g_hkVPhysicsUpdate.Add(ent);
    case Kind::GroundEntChanged:  return g_hkGroundEntChanged.Add(ent);
    case Kind::CanBeAutobalanced: return g_hkCanBeAutobalanced.Add(ent);
    }
    return {KHook::INVALID_HOOK, S2HookState::Failed, "unknown SDKHook kind"};
}

static void VpRemoveThis(Kind kind, void* p) {
    auto* ent = static_cast<CEntityInstance*>(p);
    switch (kind) {
    case Kind::StartTouch:        g_hkStartTouch.Remove(ent); break;
    case Kind::Touch:             g_hkTouch.Remove(ent); break;
    case Kind::EndTouch:          g_hkEndTouch.Remove(ent); break;
    case Kind::Blocked:           g_hkBlocked.Remove(ent); break;
    case Kind::Spawn:             g_hkSpawn.Remove(ent); break;
    case Kind::Think:             g_hkThink.Remove(ent); break;
    case Kind::PreThink:          g_hkPreThink.Remove(ent); break;
    case Kind::PostThink:         g_hkPostThink.Remove(ent); break;
    case Kind::Use:               g_hkUse.Remove(ent); break;
    case Kind::GetMaxHealth:      g_hkGetMaxHealth.Remove(ent); break;
    case Kind::ShouldCollide:     g_hkShouldCollide.Remove(ent); break;
    case Kind::VPhysicsUpdate:    g_hkVPhysicsUpdate.Remove(ent); break;
    case Kind::GroundEntChanged:  g_hkGroundEntChanged.Remove(ent); break;
    case Kind::CanBeAutobalanced: g_hkCanBeAutobalanced.Remove(ent); break;
    }
}

static bool KindLive(void* p, Kind kind) {
    return g_installed.find(VpKey{p, kind, 0}) != g_installed.end()
        || g_installed.find(VpKey{p, kind, 1}) != g_installed.end();
}

static bool OtherPhaseLive(void* p, Kind kind, int post) {
    return g_installed.find(VpKey{p, kind, post ? 0 : 1}) != g_installed.end();
}

}  // namespace

static KHook::Return<void> Hook_StartTouch(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkStartTouch.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    return S2_FromHookResult(DispatchTouch(kStartTouch, 0, thisPtr, pOther));
}
static KHook::Return<void> Hook_StartTouchPost(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkStartTouch.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchTouch("StartTouchPost", 1, thisPtr, pOther);
    return S2_Ignore();
}
static KHook::Return<void> Hook_Touch(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkTouch.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    return S2_FromHookResult(DispatchTouch(kTouch, 0, thisPtr, pOther));
}
static KHook::Return<void> Hook_TouchPost(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkTouch.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchTouch("TouchPost", 1, thisPtr, pOther);
    return S2_Ignore();
}
static KHook::Return<void> Hook_EndTouch(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkEndTouch.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    return S2_FromHookResult(DispatchTouch(kEndTouch, 0, thisPtr, pOther));
}
static KHook::Return<void> Hook_EndTouchPost(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkEndTouch.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchTouch("EndTouchPost", 1, thisPtr, pOther);
    return S2_Ignore();
}
static KHook::Return<void> Hook_Blocked(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkBlocked.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    return S2_FromHookResult(DispatchTouch(kBlocked, 0, thisPtr, pOther));
}
static KHook::Return<void> Hook_BlockedPost(CEntityInstance* thisPtr, CEntityInstance* pOther) {
    auto obs = g_hkBlocked.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchTouch("BlockedPost", 1, thisPtr, pOther);
    return S2_Ignore();
}

static KHook::Return<void> Hook_Spawn(CEntityInstance* thisPtr) {
    auto obs = g_hkSpawn.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    return S2_FromHookResult(DispatchThis(kSpawn, 0, thisPtr));
}
static KHook::Return<void> Hook_SpawnPost(CEntityInstance* thisPtr) {
    auto obs = g_hkSpawn.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis("SpawnPost", 1, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_Think(CEntityInstance* thisPtr) {
    auto obs = g_hkThink.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    return S2_FromHookResult(DispatchThis(kThink, 0, thisPtr));
}
static KHook::Return<void> Hook_ThinkPost(CEntityInstance* thisPtr) {
    auto obs = g_hkThink.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis("ThinkPost", 1, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_PreThink(CEntityInstance* thisPtr) {
    auto obs = g_hkPreThink.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis(kPreThink, 0, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_PreThinkPost(CEntityInstance* thisPtr) {
    auto obs = g_hkPreThink.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis("PreThinkPost", 1, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_PostThink(CEntityInstance* thisPtr) {
    auto obs = g_hkPostThink.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis(kPostThink, 0, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_PostThinkPost(CEntityInstance* thisPtr) {
    auto obs = g_hkPostThink.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis("PostThinkPost", 1, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_VPhysicsUpdate(CEntityInstance* thisPtr) {
    auto obs = g_hkVPhysicsUpdate.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis(kVPhysicsUpdate, 0, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_VPhysicsUpdatePost(CEntityInstance* thisPtr) {
    auto obs = g_hkVPhysicsUpdate.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis("VPhysicsUpdatePost", 1, thisPtr);
    return S2_Ignore();
}
static KHook::Return<void> Hook_GroundEntChangedPost(CEntityInstance* thisPtr) {
    auto obs = g_hkGroundEntChanged.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchThis(kGroundEntChangedPost, 1, thisPtr);
    return S2_Ignore();
}

static KHook::Return<void> Hook_Use(CEntityInstance* thisPtr, CEntityInstance* act,
                                      CEntityInstance* caller, int useType, float value) {
    auto obs = g_hkUse.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    return S2_FromHookResult(DispatchUse(kUse, 0, thisPtr, act, caller, useType, value));
}
static KHook::Return<void> Hook_UsePost(CEntityInstance* thisPtr, CEntityInstance* act,
                                          CEntityInstance* caller, int useType, float value) {
    auto obs = g_hkUse.Observe(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore();
    DispatchUse("UsePost", 1, thisPtr, act, caller, useType, value);
    return S2_Ignore();
}

static KHook::Return<int> Hook_GetMaxHealth(CEntityInstance* thisPtr) {
    auto obs = g_hkGetMaxHealth.Observe(thisPtr);
    int maxH = 0;
    if (!thisPtr) return S2_Ignore(maxH);
    maxH = g_hkGetMaxHealth.CallOriginal(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore(maxH);
    CEntityHandle h = thisPtr->GetRefEHandle();
    int hr = s2script_core_dispatch_sdkhook_getmaxhealth(
        h.GetEntryIndex(), h.GetSerialNumber(), &maxH);
    if (hr >= 2) return S2_Supersede(maxH);
    return S2_Ignore(maxH);
}

static KHook::Return<bool> Hook_ShouldCollide(CEntityInstance* thisPtr, int collisionGroup,
                                             int contentsMask) {
    auto obs = g_hkShouldCollide.Observe(thisPtr);
    bool orig = true;
    if (!thisPtr) return S2_Ignore(orig);
    orig = g_hkShouldCollide.CallOriginal(thisPtr, collisionGroup, contentsMask);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore(orig);
    CEntityHandle h = thisPtr->GetRefEHandle();
    int r = s2script_core_dispatch_sdkhook_shouldcollide(
        h.GetEntryIndex(), h.GetSerialNumber(), collisionGroup, contentsMask, orig ? 1 : 0);
    return S2_Supersede(r != 0);
}

static KHook::Return<bool> Hook_CanBeAutobalanced(CEntityInstance* thisPtr) {
    auto obs = g_hkCanBeAutobalanced.Observe(thisPtr);
    bool orig = true;
    if (!thisPtr) return S2_Ignore(orig);
    orig = g_hkCanBeAutobalanced.CallOriginal(thisPtr);
    if (!S2Hook_EnterDispatch(obs)) return S2_Ignore(orig);
    CEntityHandle h = thisPtr->GetRefEHandle();
    int r = s2script_core_dispatch_sdkhook_canbeautobalanced(
        h.GetEntryIndex(), h.GetSerialNumber(), orig ? 1 : 0);
    return S2_Supersede(r != 0);
}

static void Reconfigure(Kind kind, int slot) {
    switch (kind) {
    case Kind::StartTouch:        g_hkStartTouch.Configure(slot); break;
    case Kind::Touch:             g_hkTouch.Configure(slot); break;
    case Kind::EndTouch:          g_hkEndTouch.Configure(slot); break;
    case Kind::Blocked:           g_hkBlocked.Configure(slot); break;
    case Kind::Spawn:             g_hkSpawn.Configure(slot); break;
    case Kind::Think:             g_hkThink.Configure(slot); break;
    case Kind::PreThink:          g_hkPreThink.Configure(slot); break;
    case Kind::PostThink:         g_hkPostThink.Configure(slot); break;
    case Kind::Use:               g_hkUse.Configure(slot); break;
    case Kind::GetMaxHealth:      g_hkGetMaxHealth.Configure(slot); break;
    case Kind::ShouldCollide:     g_hkShouldCollide.Configure(slot); break;
    case Kind::VPhysicsUpdate:    g_hkVPhysicsUpdate.Configure(slot); break;
    case Kind::GroundEntChanged:  g_hkGroundEntChanged.Configure(slot); break;
    case Kind::CanBeAutobalanced: g_hkCanBeAutobalanced.Configure(slot); break;
    }
}

void S2SdkhooksVpLoad(const GameConfig& gd) {
    ClearSlots();
    g_installed.clear();

    struct Row { const char* name; Kind kind; };
    const Row rows[] = {
        { kStartTouch,           Kind::StartTouch },
        { kTouch,                Kind::Touch },
        { kEndTouch,             Kind::EndTouch },
        { kBlocked,             Kind::Blocked },
        { kSpawn,                Kind::Spawn },
        { kThink,                Kind::Think },
        { kPreThink,             Kind::PreThink },
        { kPostThink,            Kind::PostThink },
        { kUse,                  Kind::Use },
        { kGetMaxHealth,         Kind::GetMaxHealth },
        { kShouldCollide,        Kind::ShouldCollide },
        { kVPhysicsUpdate,        Kind::VPhysicsUpdate },
        { kGroundEntChangedPost, Kind::GroundEntChanged },
        { kCanBeAutobalanced,    Kind::CanBeAutobalanced },
    };

    s2validate::Ops vops;
    vops.vtable_by_name = &s2vtable::GetVTableByName;
    vops.original_virtual = &KHook::FindOriginalVirtual;

    for (const Row& row : rows) {
        auto it = gd.signatures.find(row.name);
        if (it == gd.signatures.end()) {
            // Not declared — do not FAIL the boot; vp_add returns 0 until a row lands.
            continue;
        }
        const SigSpec& sig = it->second;
        ModText mt = FindModuleText(sig.module.c_str());
        std::vector<int> pat = s2sig::ParsePattern(sig.pattern);
        if (!mt.text || pat.empty()) {
            S2GamedataResult(row.name, false, "module/pattern unavailable");
            continue;
        }
        int matches = s2sig::CountPattern(mt.text, mt.size, pat, 2);
        if (matches == 0) {
            S2GamedataResult(row.name, false, "signature NOT FOUND (moved — regenerate)");
            continue;
        }
        if (matches > 1) {
            S2GamedataResult(row.name, false, "signature AMBIGUOUS (>1 match — tighten it)");
            continue;
        }
        int64_t matchOff = s2sig::FindPattern(mt.text, mt.size, pat);
        int64_t targetOff = matchOff;
        if (sig.resolve == "ctor-body-xref") targetOff = s2sig::ResolveCtorXref(mt.text, mt.size, matchOff);
        else if (sig.resolve == "lea-disp")  targetOff = s2sig::ResolveLeaDisp(mt.text, mt.size, matchOff, 3, 7);
        if (targetOff == s2sig::kFail) {
            S2GamedataResult(row.name, false, "resolve step failed (xref/lea)");
            continue;
        }
        const void* fn = mt.text + targetOff;
        if (!InModuleText(mt, fn)) {
            S2GamedataResult(row.name, false, "resolved address is outside .text");
            continue;
        }
        s2validate::ModuleView mv;
        mv.text = mt.text; mv.textSize = mt.size; mv.lo = mt.lo; mv.hi = mt.hi;
        char reason[256] = "";
        if (!s2validate::Run(sig.validate.c_str(), mv, sig.module.c_str(), fn, vops, reason, (int)sizeof reason)) {
            S2GamedataResult(row.name, false, reason[0] ? reason : "validator failed");
            continue;
        }
        auto v = nlohmann::json::parse(sig.validate.empty() ? "{}" : sig.validate, nullptr, false);
        if (!v.is_object() || !v.contains("vtable-member") || !v["vtable-member"].is_string()) {
            S2GamedataResult(row.name, false, "validate.vtable-member class name required");
            continue;
        }
        const std::string cls = v["vtable-member"].get<std::string>();
        void** vt = s2vtable::GetVTableByName(sig.module.c_str(), cls.c_str());
        if (!vt) {
            S2GamedataResult(row.name, false, "class RTTI vtable not found");
            continue;
        }
        int slot = -1;
        for (int i = 0; i < kMaxVtableSlots; i++) {
            void* orig = KHook::FindOriginalVirtual(vt, i);
            if (!InModuleText(mt, orig)) break;
            if (orig == fn) { slot = i; break; }
        }
        if (slot < 0) {
            S2GamedataResult(row.name, false, "sig-resolved address is not a vtable slot");
            continue;
        }
        Reconfigure(row.kind, slot);
        s_slot[static_cast<int>(row.kind)] = slot;
        char ok[64];
        std::snprintf(ok, sizeof ok, "%s (slot %d)", row.name, slot);
        S2GamedataResult(ok, true, nullptr);
    }
}

bool S2SdkhooksVpCanUnloadSync(const S2HookTerminalPermit& p) {
    return S2HookInventoryCanRemoveSync(SdkhookBindingInventory(), p);
}

bool S2SdkhooksVpUnloadSync(const S2HookTerminalPermit& p) {
    if (!S2SdkhooksVpCanUnloadSync(p)) return false;
    while (!g_installed.empty()) {
        auto it = g_installed.begin();
        void* hooked = it->first.ptr;
        Kind kind = it->first.kind;
        g_installed.erase(it);
        if (!KindLive(hooked, kind)) VpRemoveThis(kind, hooked);
    }
    const bool ok = S2HookInventoryBeginRemoveSync(SdkhookBindingInventory(), p);
    ClearSlots();
    return ok && S2SdkhooksVpRemovalComplete();
}

bool S2SdkhooksVpRemovalComplete() {
    return S2HookInventoryRemovalComplete(SdkhookBindingInventory());
}

extern "C" int s2_sdkhook_vp_add(int index, int serial, const char* type, int post) {
    if (!S2Hook_AcceptingRegistrations()) return 0;
    Kind kind;
    if (!ParseKind(type, &kind)) return 0;
    if (s_slot[static_cast<int>(kind)] < 0) return 0;
    void* p = S2_ResolveEntity(index, serial);
    if (!p) return 0;
    const int phase = post ? 1 : 0;
    VpKey key{ p, kind, phase };
    auto it = g_installed.find(key);
    if (it != g_installed.end()) {
        it->second.refcount++;
        return 1;
    }
    if (!OtherPhaseLive(p, kind, phase)) {
        S2HookReceipt rec = VpAddThis(kind, p);
        if (!rec.Accepted()) {
            META_CONPRINTF("[s2script] SDKHook VP add FAILED (%s): %s\n",
                           type ? type : "?",
                           rec.reason.empty() ? "registration rejected" : rec.reason.c_str());
            return 0;
        }
    }
    g_installed[key] = VpInst{ 1, index, serial };
    return 1;
}

extern "C" int s2_sdkhook_vp_remove(int index, int serial, const char* type, int post) {
    Kind kind;
    if (!ParseKind(type, &kind)) return 0;
    void* p = S2_ResolveEntity(index, serial);
    const int phase = post ? 1 : 0;
    VpKey key{ p, kind, phase };
    auto it = g_installed.find(key);
    if (it == g_installed.end()) {
        for (auto jt = g_installed.begin(); jt != g_installed.end(); ++jt) {
            if (jt->first.kind == kind && jt->first.post == phase
                && jt->second.index == index && jt->second.serial == serial) {
                it = jt;
                break;
            }
        }
    }
    if (it == g_installed.end()) return 0;
    it->second.refcount--;
    if (it->second.refcount > 0) return 1;
    void* hooked = it->first.ptr;
    Kind k = it->first.kind;
    g_installed.erase(it);
    if (!KindLive(hooked, k)) {
        VpRemoveThis(k, hooked);
    }
    return 1;
}

extern "C" int s2_sdkhook_vp_drop(int index, int serial) {
    int n = 0;
    for (auto it = g_installed.begin(); it != g_installed.end(); ) {
        if (it->second.index == index && it->second.serial == serial) {
            void* hooked = it->first.ptr;
            Kind k = it->first.kind;
            it = g_installed.erase(it);
            n++;
            if (!KindLive(hooked, k)) {
                VpRemoveThis(k, hooked);
            }
        } else {
            ++it;
        }
    }
    return n > 0 ? 1 : 0;
}

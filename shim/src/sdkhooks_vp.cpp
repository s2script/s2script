// Per-entity SDKHooks VP hooks — SourceMod SH_ADD_MANUALHOOK, not process-wide detours.
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

SH_DECL_MANUALHOOK1_void(MHook_StartTouch, 0, 0, 0, CEntityInstance *);
SH_DECL_MANUALHOOK1_void(MHook_Touch,      0, 0, 0, CEntityInstance *);
SH_DECL_MANUALHOOK1_void(MHook_EndTouch,   0, 0, 0, CEntityInstance *);
SH_DECL_MANUALHOOK1_void(MHook_Blocked,    0, 0, 0, CEntityInstance *);
SH_DECL_MANUALHOOK0_void(MHook_Spawn, 0, 0, 0);
SH_DECL_MANUALHOOK0_void(MHook_Think, 0, 0, 0);
SH_DECL_MANUALHOOK0_void(MHook_PreThink, 0, 0, 0);
SH_DECL_MANUALHOOK0_void(MHook_PostThink, 0, 0, 0);
SH_DECL_MANUALHOOK4_void(MHook_Use, 0, 0, 0, CEntityInstance *, CEntityInstance *, int, float);
SH_DECL_MANUALHOOK0(MHook_GetMaxHealth, 0, 0, 0, int);
SH_DECL_MANUALHOOK2(MHook_ShouldCollide, 0, 0, 0, bool, int, int);
SH_DECL_MANUALHOOK0_void(MHook_VPhysicsUpdate, 0, 0, 0);
SH_DECL_MANUALHOOK0_void(MHook_GroundEntChanged, 0, 0, 0);
SH_DECL_MANUALHOOK0(MHook_CanBeAutobalanced, 0, 0, 0, bool);

namespace {

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
    if (std::strcmp(type, kBlocked) == 0)              { *out = Kind::Blocked;           return true; }
    if (std::strcmp(type, kSpawn) == 0)                { *out = Kind::Spawn;             return true; }
    if (std::strcmp(type, kThink) == 0)                { *out = Kind::Think;             return true; }
    if (std::strcmp(type, kPreThink) == 0)             { *out = Kind::PreThink;          return true; }
    if (std::strcmp(type, kPostThink) == 0)            { *out = Kind::PostThink;         return true; }
    if (std::strcmp(type, kUse) == 0)                  { *out = Kind::Use;               return true; }
    if (std::strcmp(type, kGetMaxHealth) == 0)         { *out = Kind::GetMaxHealth;      return true; }
    if (std::strcmp(type, kShouldCollide) == 0)        { *out = Kind::ShouldCollide;     return true; }
    if (std::strcmp(type, kVPhysicsUpdate) == 0)       { *out = Kind::VPhysicsUpdate;    return true; }
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
    int hook_id = 0;
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

}  // namespace

static void Hook_StartTouch(CEntityInstance* pOther) {
    int r = DispatchTouch(kStartTouch, 0, META_IFACEPTR(CEntityInstance), pOther);
    if (r >= 2) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
static void Hook_StartTouchPost(CEntityInstance* pOther) {
    DispatchTouch("StartTouchPost", 1, META_IFACEPTR(CEntityInstance), pOther);
    RETURN_META(MRES_IGNORED);
}
static void Hook_Touch(CEntityInstance* pOther) {
    int r = DispatchTouch(kTouch, 0, META_IFACEPTR(CEntityInstance), pOther);
    if (r >= 2) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
static void Hook_TouchPost(CEntityInstance* pOther) {
    DispatchTouch("TouchPost", 1, META_IFACEPTR(CEntityInstance), pOther);
    RETURN_META(MRES_IGNORED);
}
static void Hook_EndTouch(CEntityInstance* pOther) {
    int r = DispatchTouch(kEndTouch, 0, META_IFACEPTR(CEntityInstance), pOther);
    if (r >= 2) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
static void Hook_EndTouchPost(CEntityInstance* pOther) {
    DispatchTouch("EndTouchPost", 1, META_IFACEPTR(CEntityInstance), pOther);
    RETURN_META(MRES_IGNORED);
}
static void Hook_Blocked(CEntityInstance* pOther) {
    int r = DispatchTouch(kBlocked, 0, META_IFACEPTR(CEntityInstance), pOther);
    if (r >= 2) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
static void Hook_BlockedPost(CEntityInstance* pOther) {
    DispatchTouch("BlockedPost", 1, META_IFACEPTR(CEntityInstance), pOther);
    RETURN_META(MRES_IGNORED);
}

static void Hook_Spawn() {
    int r = DispatchThis(kSpawn, 0, META_IFACEPTR(CEntityInstance));
    if (r >= 2) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
static void Hook_SpawnPost() {
    DispatchThis("SpawnPost", 1, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_Think() {
    int r = DispatchThis(kThink, 0, META_IFACEPTR(CEntityInstance));
    if (r >= 2) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
static void Hook_ThinkPost() {
    DispatchThis("ThinkPost", 1, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_PreThink() {
    DispatchThis(kPreThink, 0, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_PreThinkPost() {
    DispatchThis("PreThinkPost", 1, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_PostThink() {
    DispatchThis(kPostThink, 0, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_PostThinkPost() {
    DispatchThis("PostThinkPost", 1, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_VPhysicsUpdate() {
    DispatchThis(kVPhysicsUpdate, 0, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_VPhysicsUpdatePost() {
    DispatchThis("VPhysicsUpdatePost", 1, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}
static void Hook_GroundEntChangedPost() {
    DispatchThis(kGroundEntChangedPost, 1, META_IFACEPTR(CEntityInstance));
    RETURN_META(MRES_IGNORED);
}

static void Hook_Use(CEntityInstance* act, CEntityInstance* caller, int useType, float value) {
    int r = DispatchUse(kUse, 0, META_IFACEPTR(CEntityInstance), act, caller, useType, value);
    if (r >= 2) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
}
static void Hook_UsePost(CEntityInstance* act, CEntityInstance* caller, int useType, float value) {
    DispatchUse("UsePost", 1, META_IFACEPTR(CEntityInstance), act, caller, useType, value);
    RETURN_META(MRES_IGNORED);
}

static int Hook_GetMaxHealth() {
    CEntityInstance* self = META_IFACEPTR(CEntityInstance);
    int maxH = 0;
    if (self) {
        maxH = SH_MCALL(self, MHook_GetMaxHealth)();
        CEntityHandle h = self->GetRefEHandle();
        int hr = s2script_core_dispatch_sdkhook_getmaxhealth(
            h.GetEntryIndex(), h.GetSerialNumber(), &maxH);
        if (hr >= 2) RETURN_META_VALUE(MRES_SUPERCEDE, maxH);
    }
    RETURN_META_VALUE(MRES_IGNORED, maxH);
}

static bool Hook_ShouldCollide(int collisionGroup, int contentsMask) {
    CEntityInstance* self = META_IFACEPTR(CEntityInstance);
    bool orig = true;
    if (self) {
        orig = SH_MCALL(self, MHook_ShouldCollide)(collisionGroup, contentsMask);
        CEntityHandle h = self->GetRefEHandle();
        int r = s2script_core_dispatch_sdkhook_shouldcollide(
            h.GetEntryIndex(), h.GetSerialNumber(), collisionGroup, contentsMask, orig ? 1 : 0);
        RETURN_META_VALUE(MRES_SUPERCEDE, r != 0);
    }
    RETURN_META_VALUE(MRES_IGNORED, orig);
}

static bool Hook_CanBeAutobalanced() {
    CEntityInstance* self = META_IFACEPTR(CEntityInstance);
    bool orig = true;
    if (self) {
        orig = SH_MCALL(self, MHook_CanBeAutobalanced)();
        CEntityHandle h = self->GetRefEHandle();
        int r = s2script_core_dispatch_sdkhook_canbeautobalanced(
            h.GetEntryIndex(), h.GetSerialNumber(), orig ? 1 : 0);
        RETURN_META_VALUE(MRES_SUPERCEDE, r != 0);
    }
    RETURN_META_VALUE(MRES_IGNORED, orig);
}

static int AddManual(Kind kind, void* p, int post) {
    switch (kind) {
    case Kind::StartTouch:
        return post ? SH_ADD_MANUALHOOK(MHook_StartTouch, p, SH_STATIC(Hook_StartTouchPost), true)
                    : SH_ADD_MANUALHOOK(MHook_StartTouch, p, SH_STATIC(Hook_StartTouch), false);
    case Kind::Touch:
        return post ? SH_ADD_MANUALHOOK(MHook_Touch, p, SH_STATIC(Hook_TouchPost), true)
                    : SH_ADD_MANUALHOOK(MHook_Touch, p, SH_STATIC(Hook_Touch), false);
    case Kind::EndTouch:
        return post ? SH_ADD_MANUALHOOK(MHook_EndTouch, p, SH_STATIC(Hook_EndTouchPost), true)
                    : SH_ADD_MANUALHOOK(MHook_EndTouch, p, SH_STATIC(Hook_EndTouch), false);
    case Kind::Blocked:
        return post ? SH_ADD_MANUALHOOK(MHook_Blocked, p, SH_STATIC(Hook_BlockedPost), true)
                    : SH_ADD_MANUALHOOK(MHook_Blocked, p, SH_STATIC(Hook_Blocked), false);
    case Kind::Spawn:
        return post ? SH_ADD_MANUALHOOK(MHook_Spawn, p, SH_STATIC(Hook_SpawnPost), true)
                    : SH_ADD_MANUALHOOK(MHook_Spawn, p, SH_STATIC(Hook_Spawn), false);
    case Kind::Think:
        return post ? SH_ADD_MANUALHOOK(MHook_Think, p, SH_STATIC(Hook_ThinkPost), true)
                    : SH_ADD_MANUALHOOK(MHook_Think, p, SH_STATIC(Hook_Think), false);
    case Kind::PreThink:
        return post ? SH_ADD_MANUALHOOK(MHook_PreThink, p, SH_STATIC(Hook_PreThinkPost), true)
                    : SH_ADD_MANUALHOOK(MHook_PreThink, p, SH_STATIC(Hook_PreThink), false);
    case Kind::PostThink:
        return post ? SH_ADD_MANUALHOOK(MHook_PostThink, p, SH_STATIC(Hook_PostThinkPost), true)
                    : SH_ADD_MANUALHOOK(MHook_PostThink, p, SH_STATIC(Hook_PostThink), false);
    case Kind::Use:
        return post ? SH_ADD_MANUALHOOK(MHook_Use, p, SH_STATIC(Hook_UsePost), true)
                    : SH_ADD_MANUALHOOK(MHook_Use, p, SH_STATIC(Hook_Use), false);
    case Kind::GetMaxHealth:
        return SH_ADD_MANUALHOOK(MHook_GetMaxHealth, p, SH_STATIC(Hook_GetMaxHealth), false);
    case Kind::ShouldCollide:
        return SH_ADD_MANUALHOOK(MHook_ShouldCollide, p, SH_STATIC(Hook_ShouldCollide), false);
    case Kind::VPhysicsUpdate:
        return post ? SH_ADD_MANUALHOOK(MHook_VPhysicsUpdate, p, SH_STATIC(Hook_VPhysicsUpdatePost), true)
                    : SH_ADD_MANUALHOOK(MHook_VPhysicsUpdate, p, SH_STATIC(Hook_VPhysicsUpdate), false);
    case Kind::GroundEntChanged:
        return SH_ADD_MANUALHOOK(MHook_GroundEntChanged, p, SH_STATIC(Hook_GroundEntChangedPost), true);
    case Kind::CanBeAutobalanced:
        return SH_ADD_MANUALHOOK(MHook_CanBeAutobalanced, p, SH_STATIC(Hook_CanBeAutobalanced), false);
    }
    return 0;
}

static void Reconfigure(Kind kind, int slot) {
    switch (kind) {
    case Kind::StartTouch:        SH_MANUALHOOK_RECONFIGURE(MHook_StartTouch, slot, 0, 0); break;
    case Kind::Touch:             SH_MANUALHOOK_RECONFIGURE(MHook_Touch, slot, 0, 0); break;
    case Kind::EndTouch:          SH_MANUALHOOK_RECONFIGURE(MHook_EndTouch, slot, 0, 0); break;
    case Kind::Blocked:           SH_MANUALHOOK_RECONFIGURE(MHook_Blocked, slot, 0, 0); break;
    case Kind::Spawn:             SH_MANUALHOOK_RECONFIGURE(MHook_Spawn, slot, 0, 0); break;
    case Kind::Think:             SH_MANUALHOOK_RECONFIGURE(MHook_Think, slot, 0, 0); break;
    case Kind::PreThink:          SH_MANUALHOOK_RECONFIGURE(MHook_PreThink, slot, 0, 0); break;
    case Kind::PostThink:         SH_MANUALHOOK_RECONFIGURE(MHook_PostThink, slot, 0, 0); break;
    case Kind::Use:               SH_MANUALHOOK_RECONFIGURE(MHook_Use, slot, 0, 0); break;
    case Kind::GetMaxHealth:      SH_MANUALHOOK_RECONFIGURE(MHook_GetMaxHealth, slot, 0, 0); break;
    case Kind::ShouldCollide:     SH_MANUALHOOK_RECONFIGURE(MHook_ShouldCollide, slot, 0, 0); break;
    case Kind::VPhysicsUpdate:    SH_MANUALHOOK_RECONFIGURE(MHook_VPhysicsUpdate, slot, 0, 0); break;
    case Kind::GroundEntChanged:  SH_MANUALHOOK_RECONFIGURE(MHook_GroundEntChanged, slot, 0, 0); break;
    case Kind::CanBeAutobalanced: SH_MANUALHOOK_RECONFIGURE(MHook_CanBeAutobalanced, slot, 0, 0); break;
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
        { kBlocked,              Kind::Blocked },
        { kSpawn,                Kind::Spawn },
        { kThink,                Kind::Think },
        { kPreThink,             Kind::PreThink },
        { kPostThink,            Kind::PostThink },
        { kUse,                  Kind::Use },
        { kGetMaxHealth,         Kind::GetMaxHealth },
        { kShouldCollide,        Kind::ShouldCollide },
        { kVPhysicsUpdate,       Kind::VPhysicsUpdate },
        { kGroundEntChangedPost, Kind::GroundEntChanged },
        { kCanBeAutobalanced,    Kind::CanBeAutobalanced },
    };

    s2validate::Ops vops;
    vops.vtable_by_name = &s2vtable::GetVTableByName;

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
            if (!InModuleText(mt, vt[i])) break;
            if (vt[i] == fn) { slot = i; break; }
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

void S2SdkhooksVpUnload() {
    for (auto& kv : g_installed) {
        if (kv.second.hook_id) SH_REMOVE_HOOK_ID(kv.second.hook_id);
    }
    g_installed.clear();
    ClearSlots();
}

extern "C" int s2_sdkhook_vp_add(int index, int serial, const char* type, int post) {
    Kind kind;
    if (!ParseKind(type, &kind)) return 0;
    if (s_slot[static_cast<int>(kind)] < 0) return 0;
    void* p = S2_ResolveEntity(index, serial);
    if (!p) return 0;
    VpKey key{ p, kind, post ? 1 : 0 };
    auto it = g_installed.find(key);
    if (it != g_installed.end()) {
        it->second.refcount++;
        return 1;
    }
    int hid = AddManual(kind, p, post ? 1 : 0);
    if (hid <= 0) return 0;
    g_installed[key] = VpInst{ hid, 1, index, serial };
    return 1;
}

extern "C" int s2_sdkhook_vp_remove(int index, int serial, const char* type, int post) {
    Kind kind;
    if (!ParseKind(type, &kind)) return 0;
    void* p = S2_ResolveEntity(index, serial);
    VpKey key{ p, kind, post ? 1 : 0 };
    auto it = g_installed.find(key);
    if (it == g_installed.end() && p) {
        // Stale resolve: search by index/serial.
        it = g_installed.end();
    }
    if (it == g_installed.end()) {
        for (auto jt = g_installed.begin(); jt != g_installed.end(); ++jt) {
            if (jt->first.kind == kind && jt->first.post == (post ? 1 : 0)
                && jt->second.index == index && jt->second.serial == serial) {
                it = jt;
                break;
            }
        }
    }
    if (it == g_installed.end()) return 0;
    it->second.refcount--;
    if (it->second.refcount > 0) return 1;
    if (it->second.hook_id) SH_REMOVE_HOOK_ID(it->second.hook_id);
    g_installed.erase(it);
    return 1;
}

extern "C" int s2_sdkhook_vp_drop(int index, int serial) {
    int n = 0;
    for (auto it = g_installed.begin(); it != g_installed.end(); ) {
        if (it->second.index == index && it->second.serial == serial) {
            if (it->second.hook_id) SH_REMOVE_HOOK_ID(it->second.hook_id);
            it = g_installed.erase(it);
            n++;
        } else {
            ++it;
        }
    }
    return n > 0 ? 1 : 0;
}

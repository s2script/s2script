// Per-entity SDKHooks VP hooks — SourceMod SH_ADD_MANUALHOOK, not process-wide detours.
//
// Touch / StartTouch / EndTouch / Blocked share the ABI `void (CEntityInstance *pOther)`.
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
static const char* kStartTouch = "StartTouch";
static const char* kTouch      = "Touch";
static const char* kEndTouch   = "EndTouch";
static const char* kBlocked    = "Blocked";

SH_DECL_MANUALHOOK1_void(MHook_StartTouch, 0, 0, 0, CEntityInstance *);
SH_DECL_MANUALHOOK1_void(MHook_Touch,      0, 0, 0, CEntityInstance *);
SH_DECL_MANUALHOOK1_void(MHook_EndTouch,   0, 0, 0, CEntityInstance *);
SH_DECL_MANUALHOOK1_void(MHook_Blocked,    0, 0, 0, CEntityInstance *);

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

enum class Kind { StartTouch, Touch, EndTouch, Blocked };

bool ParseKind(const char* type, Kind* out) {
    if (!type || !out) return false;
    if (std::strcmp(type, kStartTouch) == 0) { *out = Kind::StartTouch; return true; }
    if (std::strcmp(type, kTouch)      == 0) { *out = Kind::Touch;      return true; }
    if (std::strcmp(type, kEndTouch)   == 0) { *out = Kind::EndTouch;   return true; }
    if (std::strcmp(type, kBlocked)    == 0) { *out = Kind::Blocked;    return true; }
    return false;
}

int s_slot[4] = { -1, -1, -1, -1 };   // indexed by Kind; -1 = not configured

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

int DispatchTouch(const char* wiki, int post, CEntityInstance* self, CEntityInstance* other) {
    if (!self) return 0;
    CEntityHandle h = self->GetRefEHandle();
    int otherH = other ? static_cast<int>(other->GetRefEHandle().ToInt()) : -1;
    return s2script_core_dispatch_sdkhook_touch(
        h.GetEntryIndex(), h.GetSerialNumber(), otherH, post, wiki);
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
    }
    return 0;
}

void S2SdkhooksVpLoad(const GameConfig& gd) {
    s_slot[0] = s_slot[1] = s_slot[2] = s_slot[3] = -1;
    g_installed.clear();

    struct Row { const char* name; Kind kind; };
    const Row rows[] = {
        { kStartTouch, Kind::StartTouch },
        { kTouch,      Kind::Touch },
        { kEndTouch,   Kind::EndTouch },
        { kBlocked,    Kind::Blocked },
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
        switch (row.kind) {
        case Kind::StartTouch: SH_MANUALHOOK_RECONFIGURE(MHook_StartTouch, slot, 0, 0); break;
        case Kind::Touch:      SH_MANUALHOOK_RECONFIGURE(MHook_Touch,      slot, 0, 0); break;
        case Kind::EndTouch:   SH_MANUALHOOK_RECONFIGURE(MHook_EndTouch,   slot, 0, 0); break;
        case Kind::Blocked:    SH_MANUALHOOK_RECONFIGURE(MHook_Blocked,    slot, 0, 0); break;
        }
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
    s_slot[0] = s_slot[1] = s_slot[2] = s_slot[3] = -1;
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

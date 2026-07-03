// sdk_stubs/network_connection.pb.h (our stub, found first via CMake include path) must
// be resolved before eiface.h tries to include it. CMakeLists.txt lists sdk_stubs
// ahead of ${HL2SDK}/public, so the search succeeds without running protoc.
#include "s2script_mm.h"
#include "s2script_core.h"
#include "gamedata.h"

// Pull in ISource2Server (and the typedef IServerGameDLL = ISource2Server)
// from the HL2SDK.  The stub sdk_stubs/network_connection.pb.h satisfies the
// one missing generated include that eiface.h unconditionally pulls in.
#include <eiface.h>

// SchemaSystem: ISchemaSystem + the type-scope / class-info / field-data layout used by the
// schema-offset engine-op (recon Q1/Q2; include paths mirror shim/CMakeLists.txt).
#include <schemasystem/schemasystem.h>

// Entity system: CGameEntitySystem, CEntitySystem, CConcreteEntityList, CEntityIdentity,
// CEntityHandle, CEntityIndex, MAX_ENTITIES_IN_LIST, MAX_ENTITY_LISTS (recon Q3/Q4).
// Requires sdk_stubs/entitydatainstantiator.h (see shim/src/sdk_stubs/ stub — recon gap).
#include <entity2/entitysystem.h>
// CEntityInstance, NetworkStateChangedData (recon Q6).
#include <entity2/entityinstance.h>

// ConCommand, FnCommandCallback_t, ConCommandCallbackInfo_t, CCommandContext, CCommand (recon Q7).
#include <convar.h>

// IGameEvent, IGameEventListener2, IGameEventManager2, GameEventKeySymbol_t (= CKV3MemberName)
// (Slice 5D.1). CKV3MemberName is in tier1/keyvalues3.h (pulled by igameevents.h).
#include <igameevents.h>

#include <dlfcn.h>    // dladdr
#include <libgen.h>   // dirname
#include <cstring>
#include <cstdio>
#include <set>
#include <string>
#include <unordered_set>
#include <vector>

// SourceHook hook declaration: 3 void-return parameters (bool, bool, bool).
// ISource2Server is confirmed at eiface.h:384; GameFrame at eiface.h:407.
// IServerGameDLL (used in the s2_sample_mm reference) is a typedef to the same class.
SH_DECL_HOOK3_void(ISource2Server, GameFrame, SH_NOATTRIB, 0, bool, bool, bool);

S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

// ---------------------------------------------------------------------------
// Logging callback (Rust core -> Metamod console)
// ---------------------------------------------------------------------------
static void s2_logger([[maybe_unused]] int level, const char* msg) {
    META_CONPRINTF("[s2script] %s\n", msg);
}

// ---------------------------------------------------------------------------
// SchemaSystem — acquired in Load() (recon Q2), queried by the schema-offset
// engine-op below.  File-scope so the C-ABI callback can reach it; null when
// acquisition failed (schema natives then degrade to a miss, never crash).
// ---------------------------------------------------------------------------
static ISchemaSystem* s_pSchemaSystem = nullptr;

// ---------------------------------------------------------------------------
// Entity system — IGameResourceService* and the gamedata byte-offset are cached
// at Load().  The CGameEntitySystem* itself is NOT cached at Load (the map and
// entity-system don't exist yet at that point); instead GetEntitySystem() reads
// it fresh on every entity-native call so it becomes valid once the map is live.
// ---------------------------------------------------------------------------
static void* s_pGameResourceService   = nullptr;
static int   s_gameEntitySystemOffset = -1;

/// Read CGameEntitySystem* fresh from the IGameResourceService* on each call.
/// Returns nullptr when the service pointer or offset is not yet available,
/// or when the field hasn't been written yet (e.g. before the first map load).
static CGameEntitySystem* GetEntitySystem() {
    if (!s_pGameResourceService || s_gameEntitySystemOffset < 0) return nullptr;
    return *reinterpret_cast<CGameEntitySystem**>(
        reinterpret_cast<uintptr_t>(s_pGameResourceService)
        + static_cast<size_t>(s_gameEntitySystemOffset));
}

// ---------------------------------------------------------------------------
// Engine-op: resolve a schema field's flattened byte offset within a class via
// the live SchemaSystem (recon Q1).  C-ABI, called by the Rust core through the
// S2EngineOps table; `cls`/`field` are opaque strings the JS @s2script/cs2 layer
// supplies.  Returns -1 (degrade-never-crash) when the SchemaSystem is missing or
// the scope / class / field can't be resolved.
// ---------------------------------------------------------------------------
// Recursively resolve a field's flattened byte offset on a class, walking base classes.
// A field such as m_iHealth is defined on a base (e.g. CBaseEntity), not on the leaf pawn, so
// m_pFields (a class's OWN fields only) won't list it — we descend into m_pBaseClasses. For
// single inheritance the base sits at m_nOffset 0, so the recursion returns the flattened offset.
static int schema_find_field(CSchemaClassInfo* info, const char* field, int depth = 0) {
    // Depth cap: C++ inheritance graphs are acyclic + shallow, but a corrupt schema table could
    // cycle; bound the recursion so a bad pointer degrades to a miss instead of overflowing the stack.
    if (!info || depth > 32) return -1;
    for (int i = 0; i < info->m_nFieldCount; ++i) {
        const SchemaClassFieldData_t& f = info->m_pFields[i];
        if (f.m_pszName && strcmp(f.m_pszName, field) == 0) {
            return f.m_nSingleInheritanceOffset;  // THE offset getter (recon Q1)
        }
    }
    for (int b = 0; b < info->m_nBaseClassCount; ++b) {
        const SchemaBaseClassInfoData_t& base = info->m_pBaseClasses[b];
        int sub = schema_find_field(base.m_pClass, field, depth + 1);
        if (sub >= 0) return static_cast<int>(base.m_nOffset) + sub;
    }
    return -1;
}

static int s2_schema_offset(const char* cls, const char* field) {
    if (!s_pSchemaSystem || !cls || !field) return -1;

    // Resolve the class in the server-module scope, then the global scope (a class may be in either).
    // "libserver.so" is the CS2 Linux server module SONAME (recon Q1 [LC]; confirmed live in the T7 gate).
    // TODO: gamedata key if the module name ever varies across games/platforms.
    CSchemaSystemTypeScope* srvScope = s_pSchemaSystem->FindTypeScopeForModule("libserver.so");
    CSchemaClassInfo* info = srvScope ? srvScope->FindRawClassBinding(cls) : nullptr;
    if (!info) {
        CSchemaSystemTypeScope* gScope = s_pSchemaSystem->GlobalTypeScope();
        if (gScope) info = gScope->FindRawClassBinding(cls);
    }
    if (!info) return -1;  // class not found → degrade (core emits a named WARN once per key)

    // Walk base classes: m_iHealth (and most fields) are inherited, not on the leaf class directly.
    return schema_find_field(info, field);
}

// ---------------------------------------------------------------------------
// Engine-op: resolve entity by index → CEntityInstance* (opaque void*, or null).
// Uses the signature-free manual chunk walk (recon Q3) — no gamedata signature needed
// beyond the CGameEntitySystem* anchor already loaded from the interface offset.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static void* s2_ent_by_index(int idx) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return nullptr;
    if (idx < 0 || idx >= MAX_TOTAL_ENTITIES) return nullptr;

    int chunk = idx / MAX_ENTITIES_IN_LIST;
    int slot  = idx % MAX_ENTITIES_IN_LIST;

    // Guard: the chunk pointer may be null for unallocated (sparse) chunks.
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
    if (!chunk_base) return nullptr;

    CEntityIdentity* id = &chunk_base[slot];
    // EF_IS_INVALID_EHANDLE: identity slot is free/unallocated (recon Q3 [HC]).
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;

    return id->m_pInstance;  // may still be null (entity removed in progress); caller checks
}

// ---------------------------------------------------------------------------
// Engine-op: resolve a packed entity handle (u32) → CEntityInstance* or null.
// Signature-free chunk walk (recon Q4): mirrors s2_ent_by_index but adds serial
// validation via CEntityIdentity::GetRefEHandle() (inline, entityidentity.h:74).
// Does NOT call CEntitySystem::GetEntityIdentity(CEntityHandle const&) — that
// non-inline method is not exported by any CS2 module (confirmed via nm; dlopen
// blocker).  All helpers used here are inline or field accesses.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static void* s2_deref_handle(unsigned int handle) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return nullptr;

    CEntityHandle h(static_cast<uint32>(handle));
    // GetEntryIndex() is inline (entityhandle.h:106); returns -1 for INVALID_EHANDLE_INDEX.
    int idx = h.GetEntryIndex();
    if (idx < 0 || idx >= MAX_TOTAL_ENTITIES) return nullptr;

    int chunk = idx / MAX_ENTITIES_IN_LIST;
    int slot  = idx % MAX_ENTITIES_IN_LIST;

    // Guard: the chunk pointer may be null for unallocated (sparse) chunks.
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
    if (!chunk_base) return nullptr;

    CEntityIdentity* id = &chunk_base[slot];
    // EF_IS_INVALID_EHANDLE: identity slot is free/unallocated (recon Q3 [HC]).
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;
    // Serial validation: GetRefEHandle() is inline (entityidentity.h:74); it returns
    // the identity's stored handle, adjusted for EF_IS_INVALID_EHANDLE so a free slot
    // never matches a live handle.  If index+serial differ from h → stale → null.
    if (!(id->GetRefEHandle() == h)) return nullptr;

    return id->m_pInstance;  // may still be null (entity removed in progress); caller checks
}

// ---------------------------------------------------------------------------
// Engine-op: mark an entity field dirty for network transmission (recon Q6).
// Calls CEntityInstance::NetworkStateChanged(NetworkStateChangedData(offset))
// via the vtable — no export needed; null-guards the entity pointer.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static void s2_ent_state_changed(void* ent, int offset) {
    if (!ent) return;
    // SAFETY: the Rust caller holds a block-scoped entity pointer obtained from
    // s2_ent_by_index or s2_deref_handle and must not cross an await boundary.
    static_cast<CEntityInstance*>(ent)->NetworkStateChanged(
        NetworkStateChangedData(static_cast<uint32>(offset)));
}

// ---------------------------------------------------------------------------
// IGameEventManager2 + IGameEventListener2 support (Slice 5D.1).
//
// s_pGameEventManager: acquired in Load() via the engine factory; null if
//   acquisition failed (subscribe/unsubscribe become no-ops, accessors return defaults).
// s_currentEvent: set by FireGameEvent before calling core dispatch and restored after
//   (re-entrancy: a nested FireGameEvent during dispatch sees the inner event, then the
//   outer is restored when the inner call returns).
// s_subscribedNames: tracks which event names the JS layer has subscribed to so that
//   AddListener is only called once per unique name (AddListener is idempotent on some
//   versions of the SDK but we track it explicitly to be safe).  RemoveListener removes
//   the listener from all events at once (it's an all-names call per the SDK), so we
//   don't need to iterate names on teardown.
// ---------------------------------------------------------------------------
static IGameEventManager2* s_pGameEventManager = nullptr;
static IGameEvent*         s_currentEvent      = nullptr;
static std::set<std::string> s_subscribedNames;

class S2ScriptEventListener : public IGameEventListener2 {
public:
    void FireGameEvent(IGameEvent* ev) override {
        if (!ev) return;
        // Shim-side diagnostic: confirm the listener fires before core dispatch.
        Msg("[s2script] event fired: %s\n", ev->GetName());
        // Save previous (re-entrancy: if dispatch triggers another FireGameEvent, the inner
        // call will see its own event in s_currentEvent; we restore ours on return).
        IGameEvent* prev = s_currentEvent;
        s_currentEvent = ev;
        s2script_core_dispatch_game_event(ev->GetName());
        s_currentEvent = prev;  // restore
    }
};
static S2ScriptEventListener s_eventListener;

// ---------------------------------------------------------------------------
// Event engine-ops (Slice 5D.1).  C-ABI, called by the Rust core through the
// S2EngineOps table.  All degrade-never-crash: null manager / null event / null key
// → safe default.
// ---------------------------------------------------------------------------

static int s2_event_subscribe(const char* name) {
    if (!s_pGameEventManager || !name) return -1;
    // Track per-name: AddListener only the first time JS subscribes to this event name.
    if (s_subscribedNames.insert(name).second) {
        s_pGameEventManager->AddListener(&s_eventListener, name, /*bServerSide=*/true);
    }
    return 0;
}

static int s2_event_unsubscribe(const char* name) {
    // We intentionally do NOT erase `name` from s_subscribedNames here.
    // s_subscribedNames tracks "names ever registered with the engine via AddListener" so
    // that a later re-subscribe to the same name sees insert().second == false and skips
    // the second AddListener call.  Erasing on unsubscribe would break that guard: a
    // subscribe → unsubscribe → re-subscribe sequence would call AddListener twice for the
    // same (listener, name) pair, risking double-fire of the event.
    //
    // IGameEventManager2::RemoveListener is an all-names call (it cannot remove a single
    // name), so we leave the listener registered with the engine.  Any engine deliveries
    // for a name that has no active JS subscriber are silently dropped by core's empty
    // subscriber list — no JS handler runs.  The single RemoveListener + clear() happen in
    // Unload() only.
    (void)name;
    return 0;
}

static int s2_event_get_int(const char* k) {
    return (s_currentEvent && k) ? s_currentEvent->GetInt(CKV3MemberName(k), 0) : 0;
}

static float s2_event_get_float(const char* k) {
    return (s_currentEvent && k) ? s_currentEvent->GetFloat(CKV3MemberName(k), 0.0f) : 0.0f;
}

static int s2_event_get_bool(const char* k) {
    return (s_currentEvent && k)
        ? (s_currentEvent->GetBool(CKV3MemberName(k), false) ? 1 : 0)
        : 0;
}

static const char* s2_event_get_string(const char* k) {
    return (s_currentEvent && k) ? s_currentEvent->GetString(CKV3MemberName(k), "") : "";
}

// Returns uint64_t.  The SDK's uint64 is unsigned long long on Linux (tier0/platform.h:303);
// uint64_t (from <stdint.h> via s2script_core.h) resolves to the same underlying type.
static uint64_t s2_event_get_uint64(const char* k) {
    return (s_currentEvent && k)
        ? static_cast<uint64_t>(s_currentEvent->GetUint64(CKV3MemberName(k), 0))
        : 0u;
}

// Returns the CPlayerSlot index as int (-1 means absent/invalid).
// GetPlayerSlot() has no default-value parameter in the SDK — if the key is absent the
// returned CPlayerSlot value is implementation-defined; .Get() on it is the right call
// (T5 live gate confirms the fallback value; expected -1 on absence in CS2).
static int s2_event_get_player_slot(const char* k) {
    return (s_currentEvent && k)
        ? s_currentEvent->GetPlayerSlot(CKV3MemberName(k)).Get()
        : -1;
}

// ---------------------------------------------------------------------------
// ConCommand support (recon Q7).
//
// s_concommands: persistent storage for heap-allocated ConCommand objects.  The
// ConCommand constructor self-registers into the cvar system; the destructor calls
// Destroy() to unregister.  Objects live for the plugin lifetime.
// TODO(teardown): iterate s_concommands in Unload() and delete each one (ledger item).
// ---------------------------------------------------------------------------
[[maybe_unused]] static std::vector<ConCommand*> s_concommands;

// ONE shared trampoline for every registered ConCommand.  Source 2 puts the command
// name at Arg(0); ArgS() is everything after it.  Reads the name, slot, and args, then
// calls back into the Rust core via C-ABI so the registered JS function is invoked.
static void s2_concommand_trampoline(const CCommandContext& ctx, const CCommand& cmd) {
    const char* name = cmd.Arg(0);   // command name is always arg 0 in Source 2
    int slot         = ctx.GetPlayerSlot().Get();  // -1 for server-console invocations
    const char* args = cmd.ArgS();   // everything after the command name
    s2script_core_dispatch_concommand(name, slot, args ? args : "");
}

// Engine-op: register a ConCommand with the shared trampoline.
// Called by the Rust core's __s2_concommand native (through the S2EngineOps table).
// C-ABI; degrade-never-crash if name is null.
//
// NEUTRALIZED: ConCommand::Create is NOT exported by any CS2 module (confirmed via nm;
// it was a dlopen blocker).  This function logs a WARN and returns without registering.
// The trampoline, the s_concommands vector, and this wiring in S2EngineOps are kept
// intact so the full ConCommand machinery assembles cleanly for the Slice-5 command
// framework, which will route through an exported/vtable path.
// TODO(slice-5): replace this body with the Slice-5 command-registration path once the
//               correct exported or vtable-routed ConCommand::Create equivalent is identified.
static void s2_concommand_register(const char* name) {
    if (!name) return;
    META_CONPRINTF("[s2script] WARN: ConCommand registration unavailable on this build "
                   "(ConCommand::Create not exported by CS2); command '%s' not registered "
                   "— console-command support arrives with the Slice-5 command framework\n", name);
}

// ---------------------------------------------------------------------------
// Schema enumeration engine-op (5B.1).
//
// Spike-confirmed recipe (2026-07-01-slice-5b1-spike-findings.md):
//   • Iterate scope->m_ClassBindings (public CUtlTSHash) via Count/GetElements/Element.
//   • Per class: name=m_pszName; parent=m_pBaseClasses[0].m_pClass->m_pszName (guard bases/null).
//   • Per field: name=m_pszName, offset=m_nSingleInheritanceOffset, type=m_pType (NOT m_pSchemaType).
//   • CHandle detection: SCHEMA_TYPE_ATOMIC + SCHEMA_ATOMIC_T + m_sTypeName starts with "CHandle";
//     m_pAtomicInfo is NULL live — do NOT rely on it.  Inner class from m_pTemplateType.
// ---------------------------------------------------------------------------

/// Map a CSchemaType → the catalog kind string + optional type_name/inner pointer (spike §Step 3).
/// kind stays "unknown" for unmapped categories; the raw type name is still forwarded so core
/// records {kind:"unknown", name:...} rather than dropping the field.
static void schema_type_to_kind(CSchemaType* t, const char** kind,
                                const char** type_name, const char** inner) {
    *kind = "unknown"; *type_name = nullptr; *inner = nullptr;
    if (!t) return;
    switch (t->m_eTypeCategory) {
        case SCHEMA_TYPE_BUILTIN:
            *kind = "atomic"; *type_name = t->m_sTypeName.Get(); break;
        case SCHEMA_TYPE_DECLARED_CLASS:
            *kind = "class";  *type_name = t->m_sTypeName.Get(); break;
        case SCHEMA_TYPE_DECLARED_ENUM:
            *kind = "enum";   *type_name = t->m_sTypeName.Get(); break;
        case SCHEMA_TYPE_POINTER: {
            *kind = "ptr";
            auto* p = static_cast<CSchemaType_Ptr*>(t);
            if (p->m_pObjectType) *inner = p->m_pObjectType->m_sTypeName.Get();
            break;
        }
        case SCHEMA_TYPE_ATOMIC: {
            const char* full = t->m_sTypeName.Get();
            // CHandle<T>: SCHEMA_ATOMIC_T + type name starts with "CHandle".
            // m_pAtomicInfo is NULL live — detect by m_sTypeName prefix only (spike CRITICAL finding).
            // Inner class name from m_pTemplateType->m_sTypeName.
            if (t->m_eAtomicCategory == SCHEMA_ATOMIC_T && full && strncmp(full, "CHandle", 7) == 0) {
                *kind = "handle";
                auto* at = static_cast<CSchemaType_Atomic_T*>(t);
                if (at->m_pTemplateType) *inner = at->m_pTemplateType->m_sTypeName.Get();
                break;
            }
            *kind = "atomic"; *type_name = full; break;   // CUtlString, CUtlVector<...>, ...
        }
        default:  // BITFIELD, FIXED_ARRAY, INVALID
            *type_name = t->m_sTypeName.Get(); break;     // kind stays "unknown"
    }
}

/// Schema enumeration engine-op (5B.1). Walks the server type scope's declared classes via the SDK
/// and streams each class/field to core via the C-ABI callbacks. Also unions GlobalTypeScope so
/// parent classes declared outside the server module are present in the catalog (Delta 3).
/// Degrade-never-crash: null system/scope → return 0.
static int schema_enumerate(void* ctx, s2_emit_class_fn emit_class, s2_emit_field_fn emit_field) noexcept {
    if (!s_pSchemaSystem) return 0;
    CSchemaSystemTypeScope* scope = s_pSchemaSystem->FindTypeScopeForModule("libserver.so");
    if (!scope) scope = s_pSchemaSystem->GlobalTypeScope();
    if (!scope) return 0;

    // Helper: emit one class (parent-guarded) and its own fields.
    auto emit_one = [&](CSchemaClassInfo* ci) {
        if (!ci || !ci->m_pszName) return;
        const char* parent = (ci->m_nBaseClassCount > 0 && ci->m_pBaseClasses
                              && ci->m_pBaseClasses[0].m_pClass)
                             ? ci->m_pBaseClasses[0].m_pClass->m_pszName : nullptr;
        emit_class(ctx, ci->m_pszName, parent);
        if (ci->m_nFieldCount > 0 && !ci->m_pFields) return;   // degrade: skip a class with a null field array
        for (int j = 0; j < ci->m_nFieldCount; ++j) {
            const SchemaClassFieldData_t& f = ci->m_pFields[j];
            if (!f.m_pszName) continue;
            const char* kind = "unknown"; const char* type_name = nullptr; const char* inner = nullptr;
            schema_type_to_kind(f.m_pType, &kind, &type_name, &inner);
            emit_field(ctx, ci->m_pszName, f.m_pszName, f.m_nSingleInheritanceOffset,
                       kind, type_name, inner);
        }
    };

    // Pass 1: iterate the server module scope; track emitted class names to avoid field duplication.
    std::unordered_set<std::string> emitted;
    int n = scope->m_ClassBindings.Count();
    std::vector<UtlTSHashHandle_t> handles((size_t)n);
    int got = scope->m_ClassBindings.GetElements(0, n, handles.data());
    for (int i = 0; i < got; ++i) {
        CSchemaClassInfo* ci = scope->m_ClassBindings.Element(handles[i]);
        if (!ci || !ci->m_pszName) continue;
        emit_one(ci);
        emitted.insert(ci->m_pszName);
    }

    // Pass 2 (Delta 3 / completeness): union GlobalTypeScope so base classes registered outside
    // the server module scope (e.g. CBaseEntity, CEntityInstance from a different module) are also
    // present in the catalog. Skip classes already emitted from the server scope to prevent
    // field duplication (add_field appends, so a second emit of the same class would double fields).
    CSchemaSystemTypeScope* gScope = s_pSchemaSystem->GlobalTypeScope();
    if (gScope && gScope != scope) {
        int gn = gScope->m_ClassBindings.Count();
        std::vector<UtlTSHashHandle_t> ghandles((size_t)gn);
        int ggot = gScope->m_ClassBindings.GetElements(0, gn, ghandles.data());
        for (int i = 0; i < ggot; ++i) {
            CSchemaClassInfo* ci = gScope->m_ClassBindings.Element(ghandles[i]);
            if (!ci || !ci->m_pszName) continue;
            if (emitted.count(ci->m_pszName)) continue;  // already emitted from server scope
            emit_one(ci);
        }
    }

    return 1;
}

// ---------------------------------------------------------------------------
// GamedataPath: resolve the gamedata file relative to the plugin .so via
// dladdr so the path works regardless of the server's working directory.
// Expected layout: addons/s2script/bin/linuxsteamrt64/s2script.so
//   dirname ×1 → .../bin/linuxsteamrt64  → bin
//   dirname ×2 → .../bin                 → s2script addon dir
//   dirname ×3 → .../s2script            → addons/s2script
//   + /gamedata/core.gamedata.jsonc
// ---------------------------------------------------------------------------
static std::string GamedataPath() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&GamedataPath), &info) && info.dli_fname) {
        char buf[4096];
        // dirname mutates the buffer in-place; copy each time.
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir + "/gamedata/core.gamedata.jsonc";
    }
    // Fallback: relative to the server's cwd (mirrors the Slice-0 behaviour).
    return "addons/s2script/gamedata/core.gamedata.jsonc";
}

// ---------------------------------------------------------------------------
// Cs2JsPath: resolve pawn.js relative to the plugin .so via dladdr (mirrors
// GamedataPath).  Expected layout (three dirname steps from the .so):
//   addons/s2script/bin/linuxsteamrt64/s2script.so
//     dirname ×1 → bin/linuxsteamrt64
//     dirname ×2 → bin
//     dirname ×3 → s2script addon root
//   + /js/pawn.js
// ---------------------------------------------------------------------------
static std::string Cs2JsPath() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&Cs2JsPath), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir + "/js/pawn.js";
    }
    // Fallback: relative to the server's cwd (mirrors the GamedataPath fallback).
    return "addons/s2script/js/pawn.js";
}

// ---------------------------------------------------------------------------
// PluginsDir: resolve the plugins directory relative to the plugin .so via dladdr
// (mirrors Cs2JsPath / GamedataPath).  Expected layout:
//   addons/s2script/bin/linuxsteamrt64/s2script.so
//     dirname ×1 → bin/linuxsteamrt64
//     dirname ×2 → bin
//     dirname ×3 → s2script addon root
//   + /plugins
// ---------------------------------------------------------------------------
static std::string PluginsDir() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&PluginsDir), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir + "/plugins";
    }
    // Fallback: relative to the server's cwd.
    return "addons/s2script/plugins";
}

// ---------------------------------------------------------------------------
// Hook-request callback: invoked by the Rust core to install/remove the
// SourceHook detour.  Called while the core holds an internal borrow —
// MUST NOT call back into the core (no eval/dispatch/shutdown).
// ---------------------------------------------------------------------------
static void s2_request_hook(const char* descriptor, int enable) {
    if (strcmp(descriptor, "OnGameFrame") != 0) return;

    if (enable && !g_S2ScriptPlugin.m_frameHookInstalled && g_S2ScriptPlugin.m_server) {
        SH_ADD_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                    SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePre),  false);
        SH_ADD_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                    SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePost), true);
        g_S2ScriptPlugin.m_frameHookInstalled = true;
    } else if (!enable && g_S2ScriptPlugin.m_frameHookInstalled) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                       SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePre),  false);
        SH_REMOVE_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                       SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePost), true);
        g_S2ScriptPlugin.m_frameHookInstalled = false;
    }
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------
bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();  // sets g_SHPtr = ismm->GetSHPtr() — required by SH_ADD_HOOK

    // --- Interface acquisition (data-driven, degrade-never-crash) ---
    std::string gdError;
    auto versions = LoadInterfaceVersions(GamedataPath(), gdError);
    if (!gdError.empty()) {
        META_CONPRINTF("[s2script] WARN: %s — skipping interface acquisition\n", gdError.c_str());
    } else {
        CreateInterfaceFn serverFactory = ismm->GetServerFactory(false);
        CreateInterfaceFn engineFactory = ismm->GetEngineFactory(false);

        // Acquire and store ISource2Server* — needed for the SourceHook detour.
        {
            auto it = versions.find("Source2Server");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : INTERFACEVERSION_SERVERGAMEDLL;
            int ret = 0;
            m_server = serverFactory
                ? reinterpret_cast<ISource2Server*>(serverFactory(verStr, &ret))
                : nullptr;
            if (m_server && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: Source2Server (%s)\n", verStr);
            } else {
                META_CONPRINTF("[s2script] WARN: interface MISSING: Source2Server (%s)\n", verStr);
            }
        }

        // Log other interfaces (not stored — acquired as needed in later slices).
        auto tryGet = [&](const char* key, CreateInterfaceFn factory) {
            auto it = versions.find(key);
            if (it == versions.end()) {
                META_CONPRINTF("[s2script] WARN: no version string for %s in gamedata\n", key);
                return;
            }
            int ret = 0;
            void* iface = factory ? factory(it->second.c_str(), &ret) : nullptr;
            if (iface && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: %s (%s)\n", key, it->second.c_str());
            } else {
                META_CONPRINTF("[s2script] WARN: interface MISSING: %s (%s)\n", key, it->second.c_str());
            }
        };
        tryGet("EngineCvar",           engineFactory);
        tryGet("NetworkServerService", engineFactory);

        // Acquire and store ISchemaSystem* — backs the schema-offset engine-op (recon Q2).
        // Reuse the engine-factory path (as for EngineCvar/NetworkServerService); the community
        // CS2 pattern resolves SchemaSystem_001 through the engine factory even though it lives in
        // libschemasystem.so.  Degrade-never-crash: leave the pointer null on any failure.
        {
            auto it = versions.find("SchemaSystem");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : SCHEMASYSTEM_INTERFACE_VERSION;
            int ret = 0;
            s_pSchemaSystem = engineFactory
                ? reinterpret_cast<ISchemaSystem*>(engineFactory(verStr, &ret))
                : nullptr;
            if (s_pSchemaSystem && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: SchemaSystem (%s)\n", verStr);
            } else {
                s_pSchemaSystem = nullptr;  // do not keep a partially-resolved pointer
                META_CONPRINTF("[s2script] WARN: interface MISSING: SchemaSystem (%s) — schema natives degrade\n", verStr);
                // TODO(T7): if the engine factory can't resolve SchemaSystem_001 live, fall back to
                // dlopen/dlsym of libschemasystem.so's own CreateInterface (recon Q2 fallback).
            }
        }

        // Acquire IGameResourceService* and derive CGameEntitySystem* at a gamedata-provided
        // offset (recon Q3).  The offset is DATA, never hardcoded here; a wrong value degrades
        // (null entity system → entity natives return null), never crashes.
        {
            auto it = versions.find("GameResourceService");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : "GameResourceServiceServerV001";
            int ret = 0;
            void* pGameResSvc = engineFactory
                ? engineFactory(verStr, &ret)
                : nullptr;

            if (pGameResSvc && ret == 0) {
                // Read entity-system offset from gamedata (layout-is-data, never hardcoded).
                std::string offsetError;
                auto offsets = LoadOffsets(GamedataPath(), "linuxsteamrt64", offsetError);
                if (!offsetError.empty()) {
                    META_CONPRINTF("[s2script] WARN: %s — entity-system offset unavailable\n",
                                   offsetError.c_str());
                }
                auto oit = offsets.find("GameEntitySystem");
                if (oit != offsets.end() && oit->second >= 0) {
                    int entSysOffset = oit->second;
                    // Cache the service pointer and offset; do NOT read CGameEntitySystem* here.
                    // The entity-system field is null at Load (the map doesn't exist yet); we read
                    // it fresh on each entity-native call via GetEntitySystem() so it becomes valid
                    // once a map loads.  A null at Load is expected and not a WARN.
                    s_pGameResourceService   = pGameResSvc;
                    s_gameEntitySystemOffset = entSysOffset;
                    META_CONPRINTF("[s2script] interface OK: GameResourceService (%s, entity-system offset=%d cached; resolved per-call)\n",
                                   verStr, entSysOffset);
                } else {
                    META_CONPRINTF("[s2script] WARN: GameEntitySystem offset not in gamedata — entity natives degrade\n");
                }
            } else {
                META_CONPRINTF("[s2script] WARN: GameResourceService interface MISSING (%s) — entity natives degrade\n",
                               verStr);
            }
        }
        // Acquire IGameEventManager2* via the engine factory (Slice 5D.1).
        // Community-confirmed interface string: "GAMEEVENTSMANAGER002" on CS2.
        // Degrade-never-crash: leave null on any failure → event ops become no-ops.
        {
            auto it = versions.find("GameEventManager");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : "GAMEEVENTSMANAGER002";
            int ret = 0;
            s_pGameEventManager = engineFactory
                ? reinterpret_cast<IGameEventManager2*>(engineFactory(verStr, &ret))
                : nullptr;
            if (s_pGameEventManager && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: GameEventManager (%s)\n", verStr);
            } else {
                s_pGameEventManager = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: GameEventManager (%s)"
                               " — game-event natives degrade\n", verStr);
            }
        }
    }
    // --- end interface acquisition ---

    META_CONPRINTF("[s2script] Load(): initializing V8 core\n");

    // Assemble the engine-ops table for the core.  Task 3 wired schema_offset; Task 4 adds the
    // three entity ops below.  Task 5 fills concommand_register.  A null field (or a null
    // backing pointer inside the helper) degrades the matching native to a miss.
    // The core copies this struct by value at init, so the stack-local is safe to let die when
    // Load() returns.
    S2EngineOps ops = {};
    ops.schema_offset      = &s2_schema_offset;
    ops.ent_by_index       = &s2_ent_by_index;
    ops.deref_handle       = &s2_deref_handle;
    ops.ent_state_changed  = &s2_ent_state_changed;
    ops.concommand_register = &s2_concommand_register;
    ops.schema_enumerate   = &schema_enumerate;  // 5B.1: walks SchemaSystem, streams classes/fields to core
    // Game-event ops (Slice 5D.1): order MUST match S2EngineOps in s2script_core.h + Rust v8host.rs.
    ops.event_subscribe      = &s2_event_subscribe;
    ops.event_unsubscribe    = &s2_event_unsubscribe;
    ops.event_get_int        = &s2_event_get_int;
    ops.event_get_float      = &s2_event_get_float;
    ops.event_get_bool       = &s2_event_get_bool;
    ops.event_get_string     = &s2_event_get_string;
    ops.event_get_uint64     = &s2_event_get_uint64;
    ops.event_get_player_slot = &s2_event_get_player_slot;

    // Pass both callbacks + the engine-ops table; the core calls s2_request_hook("OnGameFrame", 1)
    // to lazily install the SourceHook detour once a script subscribes.
    if (s2script_core_init(&s2_logger, &s2_request_hook, &ops) != 0) {
        META_CONPRINTF("[s2script] ERROR: V8 core init failed (plugin stays loaded for diagnosis)\n");
        return true; // degrade, do not fail the load (spec §7)
    }

    // Register the @s2script/cs2 package (pawn.js) with the core so each plugin context
    // gets the game API injected at creation.  CS2 names live in the file, never in core.
    // Degrade-never-crash: a missing or unreadable pawn.js logs a WARN and continues;
    // require("@s2script/cs2") will return null in plugin contexts until it is registered.
    {
        std::string cs2JsPath = Cs2JsPath();
        FILE* f = fopen(cs2JsPath.c_str(), "rb");
        if (f) {
            fseek(f, 0, SEEK_END);
            long sz = ftell(f);
            fseek(f, 0, SEEK_SET);
            if (sz > 0) {
                std::string js(static_cast<size_t>(sz), '\0');
                size_t n = fread(&js[0], 1, static_cast<size_t>(sz), f);
                fclose(f);
                if (n == static_cast<size_t>(sz)) {
                    s2script_core_register_package("@s2script/cs2", js.c_str());
                    META_CONPRINTF("[s2script] @s2script/cs2 registered (%ld bytes from %s)\n",
                                   sz, cs2JsPath.c_str());
                } else {
                    META_CONPRINTF("[s2script] WARN: short read for %s (%zu/%ld bytes)"
                                   " — @s2script/cs2 not registered\n",
                                   cs2JsPath.c_str(), n, sz);
                }
            } else {
                fclose(f);
                META_CONPRINTF("[s2script] WARN: %s is empty — @s2script/cs2 not registered\n",
                               cs2JsPath.c_str());
            }
        } else {
            META_CONPRINTF("[s2script] WARN: could not open %s — @s2script/cs2 not registered\n",
                           cs2JsPath.c_str());
        }
    }

    // Set the plugins directory so the per-frame .s2sp watcher knows where to look.
    // Real plugins are loaded from .s2sp archives placed under addons/s2script/plugins/.
    s2script_core_set_plugins_dir(PluginsDir().c_str());
    META_CONPRINTF("[s2script] plugins dir: %s\n", PluginsDir().c_str());

    return true;
}

// ---------------------------------------------------------------------------
// Unload
// ---------------------------------------------------------------------------
bool S2ScriptPlugin::Unload(char* error, size_t maxlen) {
    META_CONPRINTF("[s2script] Unload(): shutting down V8 core\n");

    // Remove hooks before shutdown so no in-flight dispatch can reach a
    // freed core.  SH_REMOVE_HOOK is a no-op if the hook was never added.
    if (m_frameHookInstalled && m_server) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFramePre),  false);
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFramePost), true);
        m_frameHookInstalled = false;
    }

    // Unregister the game-event listener before core shutdown (Slice 5D.1).
    // RemoveListener is an all-names call per the SDK — one call removes the listener
    // from every subscribed event.  Degrade-never-crash: null manager → skip.
    if (s_pGameEventManager) {
        s_pGameEventManager->RemoveListener(&s_eventListener);
        s_pGameEventManager = nullptr;
    }
    s_subscribedNames.clear();

    s2script_core_shutdown();
    return true;
}

// ---------------------------------------------------------------------------
// SourceHook hook handlers
// ---------------------------------------------------------------------------
void S2ScriptPlugin::Hook_GameFramePre(bool simulating, bool first, bool last) {
    s2script_core_dispatch_game_frame(0, static_cast<int>(simulating),
                                      static_cast<int>(first), static_cast<int>(last));
    RETURN_META(MRES_IGNORED);
}

void S2ScriptPlugin::Hook_GameFramePost(bool simulating, bool first, bool last) {
    s2script_core_dispatch_game_frame(1, static_cast<int>(simulating),
                                      static_cast<int>(first), static_cast<int>(last));
    RETURN_META(MRES_IGNORED);
}

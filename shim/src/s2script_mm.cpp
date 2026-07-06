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
#include <playerslot.h>   // CPlayerSlot — IVEngineServer2::ClientPrintf target (Slice 6.1b)
#include <inetchannel.h>  // NetChannelBufType_t / BUF_RELIABLE (Slice 6.1c PostEventAbstract)
#include <inetchannelinfo.h>  // INetChannelInfo::GetAddress — client_address (ban-reason sub-project 2)
#include <networksystem/netmessage.h>            // CNetMessage::AsProto (Slice 6.1c)
#include <google/protobuf/message.h>             // Message/Reflection (Slice 6.1c SayText2 reflection)
#include <google/protobuf/descriptor.h>          // Descriptor::FindFieldByName (Slice 6.1c)

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

// ICvar (VEngineCvar007) — ConCommand registration via RegisterConCommand vtable call (Slice 6.1).
// icvar.h already pulls convar.h, so the explicit <convar.h> above remains for clarity.
#include <icvar.h>

// IGameEventSystem + INetworkMessages — needed for client_print chat plumbing (Slice 6.1).
#include <engine/igameeventsystem.h>
#include <networksystem/inetworkmessages.h>

// INetworkGameServer + CGlobalVars (via edict.h) — the held game-server pointer is cast to
// INetworkGameServer* for the server-info ops (reservedslots+basetriggers): GetMaxClients /
// GetMapName / GetGlobals()->curtime (typed vtable calls; the compiler derives the index).
#include <iserver.h>

#include <dlfcn.h>    // dladdr
#include <libgen.h>   // dirname
#include <link.h>       // dl_iterate_phdr, ElfW
#include "sigscan.h"
#include "detour.h"   // Slice 6.6: the self-contained inline detour (damage hook)
#include <cstring>
#include <cstdio>
#include <cstdlib>   // getenv — the S2_DAMAGE_SELFTEST opt-in gate
#include <fstream>
#include <sstream>
#include <filesystem>
#include <map>
#include <set>
#include <string>
#include <unordered_set>
#include <vector>

// SourceHook hook declaration: 3 void-return parameters (bool, bool, bool).
// ISource2Server is confirmed at eiface.h:384; GameFrame at eiface.h:407.
// IServerGameDLL (used in the s2_sample_mm reference) is a typedef to the same class.
SH_DECL_HOOK3_void(ISource2Server, GameFrame, SH_NOATTRIB, 0, bool, bool, bool);

// FireEvent(IGameEvent*, bool bDontBroadcast) -> bool (Slice 5D.3). Pre hook only.
SH_DECL_HOOK2(IGameEventManager2, FireEvent, SH_NOATTRIB, 0, bool, IGameEvent*, bool);

// ISource2GameClients::ClientCommand(CPlayerSlot, const CCommand&) -> void (Slice 6.11c). Pre hook: the
// engine's "client typed a command at the console" callback (eiface.h:594). The CSSharp/ModSharp mechanism
// for player CONSOLE commands — a clean (slot, CCommand), no low-level detour.
SH_DECL_HOOK2_void(ISource2GameClients, ClientCommand, SH_NOATTRIB, 0, CPlayerSlot, const CCommand&);

// (The Slice-6.18 ClientConnect reject SourceHook was removed in sub-project 3: ban enforcement moved to
// the JS onConnect event [basebans], which admits the client then shows the reason + kicks. The core
// s2script_core_ban_check export is retained as an available synchronous primitive but is no longer called.)

// Client lifecycle notify-hooks (@s2script/clients sub-project) — six post-hooks on the same
// m_gameClients interface. Signatures verbatim from eiface.h (:567/:578/:582/:584/:587/:599);
// each forwards to s2script_core_dispatch_client_event and RETURN_META(MRES_IGNORED) (never alters flow).
// `uint64` here matches the shim's Valve typedef; the Hook_* decls in the header use `unsigned long long`
// (== uint64 on Linux) because META_NO_HL2SDK keeps HL2SDK basetypes out of the header.
SH_DECL_HOOK6_void(ISource2GameClients, OnClientConnected, SH_NOATTRIB, 0, CPlayerSlot, const char*, uint64, const char*, const char*, bool);      // :567
SH_DECL_HOOK4_void(ISource2GameClients, ClientPutInServer, SH_NOATTRIB, 0, CPlayerSlot, const char*, int, uint64);                                 // :578
SH_DECL_HOOK4_void(ISource2GameClients, ClientActive, SH_NOATTRIB, 0, CPlayerSlot, bool, const char*, uint64);                                     // :582
SH_DECL_HOOK1_void(ISource2GameClients, ClientFullyConnect, SH_NOATTRIB, 0, CPlayerSlot);                                                          // :584
SH_DECL_HOOK5_void(ISource2GameClients, ClientDisconnect, SH_NOATTRIB, 0, CPlayerSlot, ENetworkDisconnectionReason, const char*, uint64, const char*); // :587
SH_DECL_HOOK1_void(ISource2GameClients, ClientSettingsChanged, SH_NOATTRIB, 0, CPlayerSlot);                                                       // :599

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
// s_pGameEventManager: acquired in Load() via the sig-scan of libserver.so (Slice 5D.2 — the
//   legacy manager is not CreateInterface-exported in CS2); null if the scan failed
//   (subscribe/unsubscribe become no-ops, accessors return defaults).
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

// Slice 6.6 (Stage 1): the CBaseEntity::DispatchTraceAttack detour. g_origDTA is the trampoline to the
// original (relocated prologue + jump back). The handler is READ-ONLY here — it logs candidate m_flDamage
// reads to prove the hook fires + identify the CTakeDamageInfo arg, then always calls the original.
typedef int64_t (*DispatchTraceAttack_t)(void* thisptr, void* a2, void* a3, void* a4);
static DispatchTraceAttack_t g_origDTA = nullptr;
static void* s_currentDamageInfo = nullptr;    // the CTakeDamageInfo* for the in-flight damage dispatch (block-scoped)
static void* s_currentDamageVictim = nullptr;  // the victim CEntityInstance* (detour `this`) for the same dispatch

static const uintptr_t kDtaSelfTest = 0xD2A7E57ULL;  // sentinel `this` for the install-time diversion self-test

static int64_t Detour_DispatchTraceAttack(void* thisptr, void* a2, void* a3, void* a4) {
    // Stage-1 self-test: prove the detour DIVERTS execution to our handler on the live binary (combat is
    // un-generatable on the maxplayers gate). Short-circuit BEFORE touching the dummy args + never run the
    // original (its dummy pointers would fault). Reaching this line == the patch physically diverts.
    if (reinterpret_cast<uintptr_t>(thisptr) == kDtaSelfTest) {
        META_CONPRINTF("[s2script] DTA self-test fired — detour diverts execution on the live binary (mechanism proven)\n");
        return 0;
    }
    // Real damage: expose the CTakeDamageInfo to Damage.onPre subscribers, then call the original with any
    // in-place modifications applied. a2 (rsi) is the most likely info arg (the prologue saves rdi/rsi/rdx);
    // the candidate log reveals which arg holds a plausible m_flDamage@68 once real damage flows in.
    auto rd = [](void* p) -> float {
        return (p && reinterpret_cast<uintptr_t>(p) > 0x10000) ? *reinterpret_cast<float*>(reinterpret_cast<char*>(p) + 68) : -1.0f;
    };
    META_CONPRINTF("[s2script] DTA fired: this=%p a2.dmg=%.1f a3.dmg=%.1f\n", thisptr, rd(a2), rd(a3));
    s_currentDamageInfo = a2;                     // block-scoped: valid only across this dispatch
    s_currentDamageVictim = thisptr;              // the victim entity (this)
    s2script_core_dispatch_damage();              // run Damage.onPre (read/modify the live info in place)
    s_currentDamageInfo = nullptr;
    s_currentDamageVictim = nullptr;
    return g_origDTA ? g_origDTA(thisptr, a2, a3, a4) : 0;  // original uses any modified damage
}

// Slice 5D.3: Events.fire creates an event and retargets s_currentEvent to it (save/restore on
// create/fire) so the same set* ops serve both pre-hook modify and fire-building. Nests correctly.
static IGameEvent* s_pendingFireEvent  = nullptr;
static IGameEvent* s_savedCurrentEvent = nullptr;  // s_currentEvent saved by event_create, restored by event_fire

class S2ScriptEventListener : public IGameEventListener2 {
public:
    void FireGameEvent(IGameEvent* ev) override {
        if (!ev) return;
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
// Event write + fire ops (Slice 5D.3).  C-ABI, called by the Rust core through the
// S2EngineOps table.  All degrade-never-crash: null event / null key → no-op.
// The setters write s_currentEvent; event_create saves + retargets it to the created
// event; event_fire restores (nests: a fire inside a pre-hook is safe).
// ---------------------------------------------------------------------------

static void s2_event_set_int(const char* k, int v)           { if (s_currentEvent && k) s_currentEvent->SetInt(CKV3MemberName(k), v); }
static void s2_event_set_float(const char* k, float v)       { if (s_currentEvent && k) s_currentEvent->SetFloat(CKV3MemberName(k), v); }
static void s2_event_set_bool(const char* k, int v)          { if (s_currentEvent && k) s_currentEvent->SetBool(CKV3MemberName(k), v != 0); }
static void s2_event_set_string(const char* k, const char* v){ if (s_currentEvent && k) s_currentEvent->SetString(CKV3MemberName(k), v ? v : ""); }
static void s2_event_set_uint64(const char* k, uint64_t v)   { if (s_currentEvent && k) s_currentEvent->SetUint64(CKV3MemberName(k), v); }

static int s2_event_create(const char* name) {
    if (!s_pGameEventManager || !name) return 0;
    IGameEvent* e = s_pGameEventManager->CreateEvent(name, /*bForce=*/true);
    if (!e) return 0;
    s_savedCurrentEvent = s_currentEvent;  // save (nest: a fire inside a pre-hook)
    s_pendingFireEvent  = e;
    s_currentEvent      = e;               // retarget set* to the created event
    return 1;
}
static int s2_event_fire(int dontBroadcast) {
    if (!s_pGameEventManager || !s_pendingFireEvent) return 0;
    IGameEvent* e = s_pendingFireEvent;
    s_pendingFireEvent  = nullptr;
    s_currentEvent      = s_savedCurrentEvent;  // restore the write target
    s_savedCurrentEvent = nullptr;
    // FireEvent flows through our own Hook_FireEventPre (SM parity: fired events are hookable).
    return s_pGameEventManager->FireEvent(e, dontBroadcast != 0) ? 1 : 0;
}

// ---------------------------------------------------------------------------
// Engine-identity: INetworkServerService -> INetworkGameServer -> CServerSideClient[]
// (Slice 5D.2). s_pNetworkServerService acquired in Load() (was log-only); offsets
// from gamedata (layout-is-data). Degrade-never-crash: any null/bad-offset returns safe miss.
// ---------------------------------------------------------------------------
static void* s_pNetworkServerService = nullptr;
static int s_offGameServer  = -1, s_offClientCount = -1, s_offClientElems = -1;
static int s_offSscName     = -1, s_offSscSignon   = -1, s_offSscUserId   = -1;
static const int kSignonConnected = 2;  // SIGNONSTATE_CONNECTED; >=2 = connected (incl. pawnless). Pin on gate.

// ---------------------------------------------------------------------------
// EngineCvar (ICvar*) — stored at Load() for ConCommand registration (Slice 6.1).
// s_concommandRefs: name → ConCommandRef, serves as both the name-lifetime store
// (map key = persistent std::string; m_pszName points into it) and the idempotency
// guard (reload: JS handler updated in-place; existing ConCommand still trampolines).
// TODO(teardown): iterate s_concommandRefs in Unload() and call
//   s_pCvar->UnregisterConCommandCallbacks(ref) per entry.
// ---------------------------------------------------------------------------
static ICvar* s_pCvar = nullptr;
static std::map<std::string, ConCommandRef> s_concommandRefs;

// IVEngineServer2 (Source2EngineToServer001) — used by client_print's bot guard: GetPlayerNetInfo(slot)
// returns null for a fake client (bot, no netchannel), so we skip it (sending to a bot can crash / is
// pointless). Acquired at Load(); the chat SEND itself is the SayText2 user message (Slice 6.1c).
static IVEngineServer2* s_pEngine = nullptr;

// ---------------------------------------------------------------------------
// IGameEventSystem + INetworkMessages — stored at Load() for client_print (Slice 6.1).
// The CS2 chat path: FindNetworkMessage("CUserMessageSayText2") → AllocateMessage()
// → ToPB<CUserMessageSayText2>() → field setters → PostEventAbstract.
// NOTE: the CUserMessageSayText2 protobuf type is NOT in the vendored hl2sdk headers;
//       s2_client_print is a degrade-safe stub until the type is available.
// ---------------------------------------------------------------------------
static IGameEventSystem* s_pGameEventSystem = nullptr;
static INetworkMessages*  s_pNetworkMessages  = nullptr;

static void* S2_ClientAt(int slot) {
    if (!s_pNetworkServerService || s_offGameServer < 0) return nullptr;
    void* gs = *reinterpret_cast<void**>(reinterpret_cast<char*>(s_pNetworkServerService) + s_offGameServer);
    if (!gs || s_offClientCount < 0 || s_offClientElems < 0) return nullptr;
    int n = *reinterpret_cast<int*>(reinterpret_cast<char*>(gs) + s_offClientCount);
    if (slot < 0 || slot >= n) return nullptr;
    void** elems = *reinterpret_cast<void***>(reinterpret_cast<char*>(gs) + s_offClientElems);
    return elems ? elems[slot] : nullptr;
}
static int s2_client_signon(int slot) {
    void* c = S2_ClientAt(slot);
    return (c && s_offSscSignon >= 0)
        ? *reinterpret_cast<int*>(reinterpret_cast<char*>(c) + s_offSscSignon) : -1;
}
static int s2_client_userid(int slot) {
    void* c = S2_ClientAt(slot);
    if (!c || s_offSscUserId < 0) return -1;
    return static_cast<int>(*reinterpret_cast<int16_t*>(reinterpret_cast<char*>(c) + s_offSscUserId));
}
static int s2_client_valid(int slot) {
    int s = s2_client_signon(slot);
    return (s >= kSignonConnected) ? 1 : 0;
}
static const char* s2_client_name(int slot) {
    void* c = S2_ClientAt(slot);
    if (!c || s_offSscName < 0) return nullptr;
    return *reinterpret_cast<const char**>(reinterpret_cast<char*>(c) + s_offSscName);  // core copies now
}
static int s2_client_find_by_userid(int id) {
    if (!s_pNetworkServerService || s_offGameServer < 0) return -1;
    void* gs = *reinterpret_cast<void**>(reinterpret_cast<char*>(s_pNetworkServerService) + s_offGameServer);
    if (!gs || s_offClientCount < 0) return -1;
    int n = *reinterpret_cast<int*>(reinterpret_cast<char*>(gs) + s_offClientCount);
    for (int slot = 0; slot < n; slot++) {
        if (s2_client_valid(slot) && s2_client_userid(slot) == id) return slot;
    }
    return -1;
}

// ---------------------------------------------------------------------------
// ConCommand support (Slice 6.1).
//
// Registration uses ICvar::RegisterConCommand (vtable call on s_pCvar acquired at Load).
// ConCommand::Create was NOT exported by CS2 modules (dlopen blocker, confirmed earlier);
// the ICvar interface vtable path is the correct CS2 approach.
//
// s_concommandRefs (declared above): maps command name → ConCommandRef.  The map key
// (a persistent std::string) is the name-lifetime anchor — m_pszName points into it.
// It also provides idempotency (reload-safe: the existing trampoline still routes to
// the core whose JS handler is updated in-place after a hot-reload).
//
// TODO(teardown): in Unload(), iterate s_concommandRefs and call
//   s_pCvar->UnregisterConCommandCallbacks(ref) for each entry.
// ---------------------------------------------------------------------------

// ONE shared trampoline for every registered ConCommand.  Source 2 puts the command
// name at Arg(0); ArgS() is everything after it.  Reads the name, slot, and args, then
// calls back into the Rust core via C-ABI so the registered JS function is invoked.
static void s2_concommand_trampoline(const CCommandContext& ctx, const CCommand& cmd) {
    const char* name = cmd.Arg(0);   // command name is always arg 0 in Source 2
    int slot         = ctx.GetPlayerSlot().Get();  // -1 for server-console invocations
    const char* args = cmd.ArgS();   // everything after the command name
    s2script_core_dispatch_concommand(name, slot, args ? args : "");
}

// Engine-op: register a ConCommand with the shared trampoline via ICvar::RegisterConCommand.
// Called by the Rust core's __s2_concommand native (through the S2EngineOps table).
// C-ABI; degrade-never-crash: null name or null s_pCvar logs + returns.
static void s2_concommand_register(const char* name) {
    if (!name) {
        META_CONPRINTF("[s2script] WARN: ConCommand registration called with null name — skipped\n");
        return;
    }
    if (!s_pCvar) {
        META_CONPRINTF("[s2script] WARN: ConCommand '%s' not registered — ICvar not acquired "
                       "(EngineCvar interface missing at Load)\n", name);
        return;
    }
    // Idempotency: if name is already in the ref map, skip registration.
    // On plugin hot-reload the core replaces the JS handler; the existing ConCommand
    // still trampolines through s2_concommand_trampoline to the updated core handler.
    if (s_concommandRefs.count(name)) {
        META_CONPRINTF("[s2script] ConCommand '%s' already registered — skipping (reload-safe)\n", name);
        return;
    }
    // Insert into the map first so the std::string key owns the name's storage.
    // m_pszName will point into this persistent key for the lifetime of the plugin.
    auto result = s_concommandRefs.emplace(name, ConCommandRef{});
    const std::string& persistedName = result.first->first;

    ConCommandCreation_t setup;
    setup.m_pszName       = persistedName.c_str();  // points into persistent map key
    setup.m_pszHelpString = "s2script command";
    // FCVAR_CLIENT_CAN_EXECUTE (1<<25): the CS2 engine REJECTS a client-typed command that lacks this
    // flag ("Client %s(%d) tried to execute command ... but it is not marked FCVAR_CLIENT_CAN_EXECUTE").
    // Every s2script command is registered client-executable (SourceMod parity — players run sm_* from
    // their console/chat); authorization is enforced by the registerAdmin flag gate at dispatch, NOT by
    // hiding the command from clients. Value is self-resolving from our pinned third_party/hl2sdk convar.h.
    setup.m_nFlags        = FCVAR_CLIENT_CAN_EXECUTE;
    setup.m_CBInfo        = ConCommandCallbackInfo_t(&s2_concommand_trampoline);
    // setup.m_CompletionCBInfo left default-constructed (no completion callback)

    ConCommandRef ref = s_pCvar->RegisterConCommand(setup);
    result.first->second = ref;

    if (ref.IsValidRef()) {
        META_CONPRINTF("[s2script] ConCommand '%s' registered (accessIdx=%u)\n",
                       name, (unsigned)ref.GetAccessIndex());
    } else {
        META_CONPRINTF("[s2script] WARN: ConCommand '%s' — RegisterConCommand returned invalid ref "
                       "(name conflict or ICvar internal error)\n", name);
        // Entry stays in the map with an invalid ref to prevent retry loops on re-register.
    }
}

// ---------------------------------------------------------------------------
// Chat print: client_print (Slice 6.1) — a CUserMessageSayText2 user message via protobuf reflection.
//
// Sends messagename VERBATIM (dumb pipe; color is caller content via the @s2script/chat `color` prefix).
// entityindex = the recipient's controller (slot+1) — a valid player entity; 0/worldspawn is DROPPED.
// nClientCount=64 (an iteration-count over slots; the mask bit selects). Renders reliably.
// KNOWN LIMITATION: entityindex=player makes CS2 render this as PLAYER chat, so it is TEAM-colored and
// leading color codes are muted. The game's own ClientPrint fn would render true custom colors, but calling
// it faulted on the controller/send path — deferred (the signature is confirmed; the controller resolution
// needs careful offline RE). Degrade-never-crash: every null-path is a no-op.
// ---------------------------------------------------------------------------
// The game's broadcast chat-print (SourceMod PrintToChatAll). No controller → true custom color, no crash.
typedef void (*ClientPrintAll_t)(int hudDest, const char* msg, const char* p1, const char* p2, const char* p3, const char* p4);
static ClientPrintAll_t g_ClientPrintAll = nullptr;
static const int kHudDestChat = 3;   // HudDestination::Chat (from CSSharp's HudDestination enum)

static void s2_client_print(int slot, const char* msg) {
    if (!msg) return;
    // slot < 0 = BROADCAST to all: use the game's UTIL_ClientPrintAll. It renders true custom color (a
    // leading \x04 = green, NOT team-colored) and takes NO controller, so it can't hit the per-controller
    // crash. This is what Chat.toAll routes to.
    if (slot < 0) {
        if (g_ClientPrintAll) g_ClientPrintAll(kHudDestChat, msg, nullptr, nullptr, nullptr, nullptr);
        return;
    }
    // slot >= 0 = a single client: SayText2 (renders, but team-colored — see the KNOWN LIMITATION above).
    if (slot >= 64) return;
    if (!s_pEngine || !s_pGameEventSystem || !s_pNetworkMessages) {
        static bool s_warned = false;
        if (!s_warned) { s_warned = true;
            META_CONPRINTF("[s2script] client_print: interfaces not acquired — chat not delivered\n"); }
        return;
    }
    // Skip bots / clients with no netchannel — a print to a fake client can crash (SM's fake-client skip).
    if (!s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot))) return;
    INetworkMessageInternal* pInfo = s_pNetworkMessages->FindNetworkMessagePartial("SayText2");
    if (!pInfo) {
        static bool s_warnedNoMsg = false;
        if (!s_warnedNoMsg) { s_warnedNoMsg = true;
            META_CONPRINTF("[s2script] client_print: SayText2 not found — chat not delivered\n"); }
        return;
    }
    // Per-recipient colored chat (proven live via a field-combo probe): entityindex = the recipient's
    // controller (slot+1) so it renders (entityindex=0 is DROPPED), and chat = FALSE so it renders as a
    // SERVER message (NOT player chat) — which means it is NOT team-colored and RESPECTS inline color codes
    // (the caller embeds ChatColors bytes, same as the UTIL_ClientPrintAll broadcast path). messagename =
    // the message VERBATIM (dumb pipe; color is caller content).
    CNetMessage* pData = pInfo->AllocateMessage();
    if (!pData) return;
    google::protobuf::Message* m = reinterpret_cast<google::protobuf::Message*>(pData->AsProto());
    if (m) {
        const google::protobuf::Descriptor* d = m->GetDescriptor();
        const google::protobuf::Reflection*  r = m->GetReflection();
        if (d && r) {
            if (const auto* f = d->FindFieldByName("entityindex")) r->SetInt32(m, f, slot + 1);
            if (const auto* f = d->FindFieldByName("chat"))        r->SetBool(m, f, false);
            if (const auto* f = d->FindFieldByName("messagename")) r->SetString(m, f, msg);
        }
    }
    uint64 clients = (1ull << static_cast<uint64>(slot));
    s_pGameEventSystem->PostEventAbstract(-1, false, 64, &clients, pInfo, pData, 0, BUF_RELIABLE);
}

// ---------------------------------------------------------------------------
// Client SteamID64 engine-op (Slice 6.2).
//
// Returns the SteamID64 of the client in `slot` as a decimal string in a static buffer.
// Returns "0" for bots, unauthenticated clients, or out-of-range slots.
// Via IVEngineServer2::GetClientXUID (already acquired in Load for client_print).
// ---------------------------------------------------------------------------
static std::string s_steamidBuf;
static const char* s2_client_steamid(int slot) {
    if (!s_pEngine || slot < 0 || slot >= 64) { s_steamidBuf = "0"; return s_steamidBuf.c_str(); }
    uint64 xuid = s_pEngine->GetClientXUID(CPlayerSlot(slot));   // 0 for bots / unauthenticated
    s_steamidBuf = std::to_string(xuid);
    return s_steamidBuf.c_str();
}

// ---------------------------------------------------------------------------
// client_kick (Slice 6.3) — disconnect a client via IVEngineServer2::KickClient.
// No-op for a null engine or an out-of-range slot (degrade-never-crash).
// ---------------------------------------------------------------------------
static void s2_client_kick(int slot, const char* reason) {
    if (!s_pEngine || slot < 0 || slot >= 64) return;
    s_pEngine->KickClient(CPlayerSlot(slot), reason ? reason : "Kicked by admin", NETWORK_DISCONNECT_KICKED);
}

// ---------------------------------------------------------------------------
// client_console_print (ban-reason sub-project 2) — print one line to a client's
// developer console via IVEngineServer2::ClientPrintf (eiface.h:238, proven live-safe in 6.1b).
// The bot-skip guard (GetPlayerNetInfo == null) is MANDATORY — a print to a null-netchannel
// fake client segfaults (mirrors the s2_client_print guard at :606).
// ---------------------------------------------------------------------------
static void s2_client_console_print(int slot, const char* msg) {
    if (!s_pEngine || slot < 0 || slot >= 64) return;
    if (!s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot))) return;   // bot / no netchannel — skip (would segfault)
    s_pEngine->ClientPrintf(CPlayerSlot(slot), msg ? msg : "");
}

// ---------------------------------------------------------------------------
// client_address (ban-reason sub-project 2) — the client's "IP:port" via
// GetPlayerNetInfo(slot)->GetAddress(). "" for a bot / no netchannel (mirrors the
// s_steamidBuf static-string pattern at :642). Valid until the next call.
// ---------------------------------------------------------------------------
static std::string s_addressBuf;
static const char* s2_client_address(int slot) {
    s_addressBuf = "";
    if (s_pEngine && slot >= 0 && slot < 64) {
        INetChannelInfo* nci = s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot));
        if (nci) { const char* a = nci->GetAddress(); if (a) s_addressBuf = a; }
    }
    return s_addressBuf.c_str();
}

// ---------------------------------------------------------------------------
// server_command / server_map_valid (Slice 6.4) — IVEngineServer2 passthroughs. Null/no-engine safe.
// ---------------------------------------------------------------------------
static void s2_server_command(const char* cmd) {
    if (!s_pEngine || !cmd) return;
    s_pEngine->ServerCommand(cmd);
}
static int s2_server_map_valid(const char* map) {
    if (!s_pEngine || !map) return 0;
    return s_pEngine->IsMapValid(map) ? 1 : 0;
}

// ---------------------------------------------------------------------------
// Server-info ops (reservedslots+basetriggers) — typed vtable calls on the SAME
// game-server pointer the 5D.2 client-list code dereferences (INetworkServerService +
// s_offGameServer). We cast that void* to INetworkGameServer* and call the typed methods
// (GetMaxClients / GetMapName / GetGlobals()->curtime) so the compiler derives the vtable
// index from iserver.h — no manual index math. Degrade-never-crash: null → 0 / "" / 0.
// ---------------------------------------------------------------------------
static INetworkGameServer* S2_GameServer() {
    if (!s_pNetworkServerService || s_offGameServer < 0) return nullptr;
    void* gs = *reinterpret_cast<void**>(reinterpret_cast<char*>(s_pNetworkServerService) + s_offGameServer);
    return reinterpret_cast<INetworkGameServer*>(gs);
}
static int s2_server_max_clients() {
    INetworkGameServer* gs = S2_GameServer();
    return gs ? gs->GetMaxClients() : 0;
}
static std::string s_mapNameBuf;
static const char* s2_server_map_name() {
    s_mapNameBuf = "";
    INetworkGameServer* gs = S2_GameServer();
    if (gs) { const char* m = gs->GetMapName(); if (m) s_mapNameBuf = m; }
    return s_mapNameBuf.c_str();
}
static float s2_server_game_time() {
    INetworkGameServer* gs = S2_GameServer();
    if (!gs) return 0.0f;
    CGlobalVars* g = gs->GetGlobals();
    return g ? g->curtime : 0.0f;
}

// ---------------------------------------------------------------------------
// cvar_get (Slice 6.7) — a cvar's current value as a string, TIER1-FREE. The clean SDK accessors
// (ConVarData::ValueOrDefault, ConVarRefAbstract::GetString→CUtlString) are NON-inline → they'd
// reintroduce the tier1/dlopen cascade (5D.1). Instead: FindConVar+GetConVarData (vtable on s_pCvar) +
// GetType (inline) + a direct read of m_Values (ConVarData's LAST field → offset = sizeof(ConVarData) -
// sizeof(CVValue_t)*MAX_SPLITSCREEN_CLIENTS). The pinned-SDK layout is live-verified against a known cvar.
// ---------------------------------------------------------------------------
static char s_cvarBuf[512];
static const char* s2_cvar_get(const char* name) {
    s_cvarBuf[0] = '\0';
    if (!s_pCvar || !name) return s_cvarBuf;
    ConVarRef ref = s_pCvar->FindConVar(name, false);
    if (!ref.IsValidRef()) return s_cvarBuf;
    ConVarData* data = s_pCvar->GetConVarData(ref);
    if (!data) return s_cvarBuf;
    const size_t VOFF = sizeof(ConVarData) - sizeof(CVValue_t) * MAX_SPLITSCREEN_CLIENTS;
    CVValue_t* v = reinterpret_cast<CVValue_t*>(reinterpret_cast<char*>(data) + VOFF);
    switch (data->GetType()) {
        case EConVarType_Bool:    snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%d", v->m_bValue ? 1 : 0); break;
        case EConVarType_Int16:   snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%d", (int)v->m_i16Value); break;
        case EConVarType_UInt16:  snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%u", (unsigned)v->m_u16Value); break;
        case EConVarType_Int32:   snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%d", v->m_i32Value); break;
        case EConVarType_UInt32:  snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%u", v->m_u32Value); break;
        case EConVarType_Int64:   snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%lld", (long long)v->m_i64Value); break;
        case EConVarType_UInt64:  snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%llu", (unsigned long long)v->m_u64Value); break;
        case EConVarType_Float32: snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%g", v->m_fl32Value); break;
        case EConVarType_Float64: snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%g", v->m_fl64Value); break;
        case EConVarType_String: { const char* sv = v->m_StringValue.Get();
                                   snprintf(s_cvarBuf, sizeof(s_cvarBuf), "%s", sv ? sv : ""); break; }
        default: snprintf(s_cvarBuf, sizeof(s_cvarBuf), "<type%d>", (int)data->GetType()); break;
    }
    return s_cvarBuf;
}

// ---------------------------------------------------------------------------
// pawn_commit_suicide (Slice 6.14) — kill a pawn via CBasePlayerPawn::CommitSuicide. The failed Slice-6.8
// branch (a085d5a) reached it by the borrowed ModSharp VTABLE INDEX (400 — wrong on our build; it's 819
// here), which broke live. Per the RE doctrine we resolve it by a DIRECT prologue SIGNATURE self-scanned
// on OUR libserver.so (s_pCommitSuicide, loaded in Load), NOT a borrowed index. GUARDED: the pawn is
// reconstructed from (idx, serial) + serial-gated (s2_deref_handle → null if stale), and the resolved fn
// ptr must point into libserver's .text (a null/out-of-range ptr degrades to a logged no-op, not a crash).
// Signature: void CBasePlayerPawn::CommitSuicide(bool bExplode /*esi*/, bool bForce /*edx*/).
// ---------------------------------------------------------------------------
typedef void (*CommitSuicide_t)(void* thisptr, bool bExplode, bool bForce);
static CommitSuicide_t s_pCommitSuicide = nullptr;       // sig-resolved fn ptr (loaded in Load)
static const uint8_t*  s_serverText     = nullptr;       // libserver.so .text range for the call-site guard
static size_t          s_serverTextSize = 0;
static void s2_pawn_commit_suicide(int idx, int serial) {
    if (!s_pCommitSuicide) return;                        // signature unresolved -> no-op
    // Reconstruct the packed CEntityHandle from (index, serial) using the SDK bitfield layout
    // (m_EntityIndex:15, m_Serial:17) — no magic constants — then serial-gate via s2_deref_handle.
    CEntityHandle h(idx, serial);
    void* pawn = s2_deref_handle(static_cast<unsigned int>(h.ToInt()));  // null if stale/free slot
    if (!pawn) return;
    const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pCommitSuicide);
    if (!s_serverText || f < s_serverText || f >= s_serverText + s_serverTextSize) {
        META_CONPRINTF("[s2script] CommitSuicide fn %p out of libserver .text — no-op\n", (const void*)f);
        return;
    }
    s_pCommitSuicide(pawn, /*bExplode=*/false, /*bForce=*/true);
}

// ---------------------------------------------------------------------------
// Damage-info accessors (Slice 6.6 Stage 2). Read/write a field of the CURRENT CTakeDamageInfo
// (s_currentDamageInfo, set by the DispatchTraceAttack detour) at a schema-resolved byte offset.
// Valid only during a damage dispatch; null-guarded. The raw pointer never crosses to JS.
// ---------------------------------------------------------------------------
static float s2_damage_read_float(int offset) {
    if (!s_currentDamageInfo || offset < 0 || offset > 4096) return 0.0f;
    return *reinterpret_cast<float*>(reinterpret_cast<char*>(s_currentDamageInfo) + offset);
}
static int s2_damage_read_int(int offset) {
    if (!s_currentDamageInfo || offset < 0 || offset > 4096) return 0;
    return *reinterpret_cast<int*>(reinterpret_cast<char*>(s_currentDamageInfo) + offset);
}
static void s2_damage_write_float(int offset, float value) {
    if (!s_currentDamageInfo || offset < 0 || offset > 4096) return;
    *reinterpret_cast<float*>(reinterpret_cast<char*>(s_currentDamageInfo) + offset) = value;
}
// The victim's raw CEntityHandle from the detour `this` (CEntityInstance::GetRefEHandle().ToInt() — inline,
// == the raw m_Index the JS handle-decode expects). -1 when absent. The raw pointer never crosses to JS.
static int s2_damage_victim() {
    if (!s_currentDamageVictim) return -1;
    return static_cast<CEntityInstance*>(s_currentDamageVictim)->GetRefEHandle().ToInt();
}

// ---------------------------------------------------------------------------
// Host_Say detour (Slice 6.11b): player chat triggers (!cmd / /cmd).
//
// CS2 fires no usable player_chat game event, so chat is intercepted by detouring Host_Say (the chat
// broadcast fn — the CSSharp/ModSharp approach). SysV arg layout (prologue saves rsi->r15, r8->r12):
//   rdi = CBaseEntity* pController (the speaker's controller)   rsi = CCommand& args (the chat text)
//   rdx = bool teamonly                                          r8  = const char* (unused by us)
// The speaker's slot = controller entity index - 1 (CS2 pre-allocates all 64 player controllers at
// entity indices 1..64; confirmed live: slot 2 <-> controller index 3). The message is CCommand::Arg(1)
// (clean, unquoted — ArgS() wraps it in quotes). core dispatch_chat parses the !cmd / /cmd trigger and
// returns 1 to SUPPRESS the broadcast (a matched silent `/`); we then skip the original. Every deref is
// pointer-guarded; degrade-never-crash (a resolve failure just falls through to the original broadcast).
typedef void (*HostSay_t)(void* pController, void* pCmd, bool teamonly, int a4, const char* a5);
static HostSay_t g_origHostSay = nullptr;

static void Detour_HostSay(void* pController, void* pCmd, bool teamonly, int a4, const char* a5) {
    int slot = -1;
    if (pController && reinterpret_cast<uintptr_t>(pController) > 0x10000) {
        int idx = static_cast<CEntityInstance*>(pController)->GetRefEHandle().GetEntryIndex();
        if (idx >= 1) slot = idx - 1;
    }
    const char* msg = nullptr;
    if (pCmd && reinterpret_cast<uintptr_t>(pCmd) > 0x10000) {
        msg = reinterpret_cast<const CCommand*>(pCmd)->Arg(1);   // the raw chat message, unquoted
    }
    int suppress = 0;
    if (slot >= 0 && msg && msg[0]) {
        suppress = s2script_core_dispatch_chat(slot, msg, teamonly ? 1 : 0); // trigger/dispatch + raw subs + suppress?
    }
    // suppress (a matched silent `/` trigger) -> skip the original so the message is NOT broadcast.
    if (!suppress && g_origHostSay) g_origHostSay(pController, pCmd, teamonly, a4, a5);
}

// ---------------------------------------------------------------------------
// (Player console commands are handled by the ISource2GameClients::ClientCommand SourceHook — see
// Hook_ClientCommand. An earlier attempt to detour the low-level CServerSideClient::ExecuteStringCommand
// was the wrong layer and was removed: ClientCommand gives a clean (slot, CCommand), which is exactly how
// CSSharp/ModSharp implement console commands.)
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
// ConfigPath: resolve addons/s2script/configs/<sanitized id>.json via dladdr
// (mirrors PluginsDir).  Non-[A-Za-z0-9._-] chars in `id` are replaced with '_'.
// ---------------------------------------------------------------------------
static std::string ConfigPath(const char* id) {
    // Sanitize id: non-[A-Za-z0-9._-] → '_' (matches the CLI's .s2sp id sanitization).
    std::string safe_id;
    for (const char* p = id; *p; ++p) {
        char c = *p;
        if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
            || c == '.' || c == '_' || c == '-') {
            safe_id += c;
        } else {
            safe_id += '_';
        }
    }
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&ConfigPath), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir + "/configs/" + safe_id + ".json";
    }
    // Fallback: relative to the server's cwd.
    return "addons/s2script/configs/" + safe_id + ".json";
}

// ---------------------------------------------------------------------------
// Config ops (Slice 5E.2): read/auto-write the admin override file.
// ---------------------------------------------------------------------------
static std::string s_configReadBuf;
static const char* s2_config_read(const char* id) {
    if (!id) return nullptr;
    std::ifstream f(ConfigPath(id));
    if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf();
    s_configReadBuf = ss.str();
    return s_configReadBuf.c_str();
}
static int s2_config_write(const char* id, const char* content) {
    if (!id || !content) return 0;
    std::string path = ConfigPath(id);
    std::error_code ec; std::filesystem::create_directories(std::filesystem::path(path).parent_path(), ec);
    std::ofstream f(path); if (!f) return 0; f << content; return f.good() ? 1 : 0;
}

// ---------------------------------------------------------------------------
// Hook-request callback: invoked by the Rust core to install/remove the
// SourceHook detour.  Called while the core holds an internal borrow —
// MUST NOT call back into the core (no eval/dispatch/shutdown).
// ---------------------------------------------------------------------------
static void s2_request_hook(const char* descriptor, int enable) {
    if (strcmp(descriptor, "OnGameFrame") == 0) {
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
        return;
    }
    if (strcmp(descriptor, "GameEvent") == 0) {
        if (enable && !g_S2ScriptPlugin.m_eventHookInstalled && s_pGameEventManager) {
            SH_ADD_HOOK(IGameEventManager2, FireEvent, s_pGameEventManager,
                        SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_FireEventPre), false);
            g_S2ScriptPlugin.m_eventHookInstalled = true;
        } else if (!enable && g_S2ScriptPlugin.m_eventHookInstalled) {
            SH_REMOVE_HOOK(IGameEventManager2, FireEvent, s_pGameEventManager,
                           SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_FireEventPre), false);
            g_S2ScriptPlugin.m_eventHookInstalled = false;
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// FindModuleText (Slice 5D.2): locate the largest executable segment of a loaded module by soname
// substring. Returns {nullptr, 0} if not found. Live-only (dl_iterate_phdr); the pure
// match/extract is sigscan.
// ---------------------------------------------------------------------------
struct ModText { const uint8_t* text; size_t size; };
static ModText FindModuleText(const char* soname) {
    // Pick the LARGEST executable segment across ALL loaded modules whose soname contains `soname`.
    // Why "all + largest" and not "first match": Metamod:Source inserts its own thin libserver.so
    // proxy (csgo/addons/metamod/.../libserver.so, ~95 KB) via the gameinfo SearchPath, whose path
    // ALSO contains the "libserver.so" substring. Stopping at the first substring match grabbed that
    // proxy (no game code) instead of the real ~25 MB game module. The real game module's .text
    // dwarfs the proxy's, so largest-PF_X-segment-wins selects it robustly (found live, de_inferno).
    struct Ctx { const char* name; ModText out; } ctx{ soname, { nullptr, 0 } };
    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto* c = static_cast<Ctx*>(data);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) return 0;  // not a match; keep scanning
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type == PT_LOAD && (ph.p_flags & PF_X) && ph.p_filesz > c->out.size) {
                c->out.text = reinterpret_cast<const uint8_t*>(info->dlpi_addr + ph.p_vaddr);
                c->out.size = ph.p_filesz;                       // largest PF_X seg across all matches
            }
        }
        return 0;   // keep scanning ALL modules — the metamod proxy must not shadow the real game module
    }, &ctx);
    return ctx.out;
}

// ---------------------------------------------------------------------------
// Gamedata validation report (Slice 6.9). Every engine fact resolved against the LIVE binary records a
// pass/fail here so a version mismatch / stale signature is LOUD at boot, not a silent no-op (the sm_slay
// class of bug). See docs/re-strategy.md. Reset at each Load; a banner is emitted after resolution.
// ---------------------------------------------------------------------------
static int s_gdOk = 0, s_gdFail = 0;
static void GamedataResult(const char* name, bool ok, const char* reason) {
    if (ok) { s_gdOk++;  META_CONPRINTF("[s2script]   gamedata OK    %s\n", name); }
    else    { s_gdFail++; META_CONPRINTF("[s2script]   gamedata FAIL  %s — %s\n", name, reason ? reason : "?"); }
}
// Resolve a "direct"/"ctor-body-xref"/"lea-disp" signature AND verify it matches UNIQUELY (Rule 2): 0 = the
// pattern moved (stale), >1 = ambiguous. Records the result and returns the resolved module offset, or kFail.
static int64_t ResolveSigValidated(const char* name, const SigSpec& sig) {
    ModText mt = FindModuleText(sig.module.c_str());
    std::vector<int> pat = s2sig::ParsePattern(sig.pattern);
    if (!mt.text || pat.empty()) { GamedataResult(name, false, "module/pattern unavailable"); return s2sig::kFail; }
    int matches = s2sig::CountPattern(mt.text, mt.size, pat, 2);
    if (matches == 0) { GamedataResult(name, false, "signature NOT FOUND (moved — regenerate)"); return s2sig::kFail; }
    if (matches > 1)  { GamedataResult(name, false, "signature AMBIGUOUS (>1 match — tighten it)"); return s2sig::kFail; }
    int64_t matchOff = s2sig::FindPattern(mt.text, mt.size, pat);
    int64_t targetOff = matchOff;   // "direct": the match IS the target
    if (sig.resolve == "ctor-body-xref") targetOff = s2sig::ResolveCtorXref(mt.text, mt.size, matchOff);
    else if (sig.resolve == "lea-disp")  targetOff = s2sig::ResolveLeaDisp(mt.text, mt.size, matchOff, 3, 7);
    if (targetOff == s2sig::kFail) { GamedataResult(name, false, "resolve step failed (xref/lea)"); return s2sig::kFail; }
    GamedataResult(name, true, nullptr);
    return targetOff;
}
static void GamedataBanner() {
    META_CONPRINTF("[s2script] === GAMEDATA VALIDATION: %d ok, %d FAILED%s ===\n", s_gdOk, s_gdFail,
                   s_gdFail ? "  (STALE for this CS2 build — regenerate; see docs/re-strategy.md)" : "");
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------
bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();  // sets g_SHPtr = ismm->GetSHPtr() — required by SH_ADD_HOOK
    s_gdOk = 0; s_gdFail = 0;   // reset the gamedata validation report for this Load

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

        // Acquire ISource2GameClients + install the ClientCommand hook (Slice 6.11c): PLAYER CONSOLE
        // commands. The CSSharp/ModSharp mechanism — the engine's "client typed a command at the console"
        // callback, a clean (slot, CCommand). Version Source2GameClients001. Degrade-never-crash.
        {
            auto it = versions.find("Source2GameClients");
            const char* verStr = (it != versions.end()) ? it->second.c_str() : INTERFACEVERSION_SERVERGAMECLIENTS;
            int ret = 0;
            m_gameClients = serverFactory
                ? reinterpret_cast<ISource2GameClients*>(serverFactory(verStr, &ret)) : nullptr;
            if (m_gameClients && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: Source2GameClients (%s)\n", verStr);
                SH_ADD_HOOK(ISource2GameClients, ClientCommand, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientCommand), false);
                m_clientCmdHookInstalled = true;
                META_CONPRINTF("[s2script] ClientCommand hook installed (player console commands)\n");
                // @s2script/clients: six notify lifecycle hooks -> s2script_core_dispatch_client_event.
                SH_ADD_HOOK(ISource2GameClients, OnClientConnected, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_OnClientConnected), false);
                SH_ADD_HOOK(ISource2GameClients, ClientPutInServer, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientPutInServer), false);
                SH_ADD_HOOK(ISource2GameClients, ClientActive, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientActive), false);
                SH_ADD_HOOK(ISource2GameClients, ClientFullyConnect, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientFullyConnect), false);
                SH_ADD_HOOK(ISource2GameClients, ClientDisconnect, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientDisconnect), false);
                SH_ADD_HOOK(ISource2GameClients, ClientSettingsChanged, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientSettingsChanged), false);
                m_clientLifecycleHooksInstalled = true;
                META_CONPRINTF("[s2script] client lifecycle hooks installed (6 notify)\n");
            } else {
                m_gameClients = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: Source2GameClients (%s) — console commands off\n", verStr);
            }
        }

        // Acquire and STORE ICvar* (Slice 6.1 ConCommand registration via vtable).
        // ConCommand::Create was NOT exported by CS2; ICvar::RegisterConCommand is.
        // Degrade-never-crash: null s_pCvar → s2_concommand_register logs + skips.
        {
            auto it = versions.find("EngineCvar");
            const char* verStr = (it != versions.end()) ? it->second.c_str() : "VEngineCvar007";
            int ret = 0;
            s_pCvar = engineFactory
                ? reinterpret_cast<ICvar*>(engineFactory(verStr, &ret))
                : nullptr;
            if (s_pCvar && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: EngineCvar (%s)\n", verStr);
            } else {
                s_pCvar = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: EngineCvar (%s) — ConCommand registration degrades\n", verStr);
            }
        }
        // Acquire and STORE IVEngineServer2* (Slice 6.1b — client_print via ClientPrintf).
        {
            auto it = versions.find("EngineToServer");
            const char* verStr = (it != versions.end()) ? it->second.c_str() : "Source2EngineToServer001";
            int ret = 0;
            s_pEngine = engineFactory
                ? reinterpret_cast<IVEngineServer2*>(engineFactory(verStr, &ret))
                : nullptr;
            if (s_pEngine && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: EngineToServer (%s)\n", verStr);
            } else {
                s_pEngine = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: EngineToServer (%s) — client_print degrades\n", verStr);
            }
        }
        // Acquire and STORE IGameEventSystem* (Slice 6.1 client_print chat plumbing).
        {
            auto it = versions.find("GameEventSystem");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : GAMEEVENTSYSTEM_INTERFACE_VERSION;
            int ret = 0;
            s_pGameEventSystem = engineFactory
                ? reinterpret_cast<IGameEventSystem*>(engineFactory(verStr, &ret))
                : nullptr;
            if (s_pGameEventSystem && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: GameEventSystem (%s)\n", verStr);
            } else {
                s_pGameEventSystem = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: GameEventSystem (%s) — client_print chat degrades\n", verStr);
            }
        }
        // Acquire and STORE INetworkMessages* (Slice 6.1 client_print chat plumbing).
        {
            auto it = versions.find("NetworkMessages");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : NETWORKMESSAGES_INTERFACE_VERSION;
            int ret = 0;
            s_pNetworkMessages = engineFactory
                ? reinterpret_cast<INetworkMessages*>(engineFactory(verStr, &ret))
                : nullptr;
            if (s_pNetworkMessages && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: NetworkMessages (%s)\n", verStr);
            } else {
                s_pNetworkMessages = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: NetworkMessages (%s) — client_print chat degrades\n", verStr);
            }
        }
        // Acquire + STORE INetworkServerService* (Slice 5D.2 engine identity; was log-only).
        {
            auto it = versions.find("NetworkServerService");
            const char* verStr = (it != versions.end()) ? it->second.c_str() : "NetworkServerService_001";
            int ret = 0;
            s_pNetworkServerService = engineFactory ? engineFactory(verStr, &ret) : nullptr;
            if (s_pNetworkServerService && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: NetworkServerService (%s)\n", verStr);
            } else {
                s_pNetworkServerService = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: NetworkServerService (%s) — identity natives degrade\n", verStr);
            }
        }

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
        // Acquire IGameEventManager2* via signature scan (Slice 5D.2). GAMEEVENTSMANAGER002 is NOT a
        // registered interface in CS2 (in zero modules), so the global is resolved from libserver.so
        // by pattern. Signature + module are gamedata (layout-is-data). Degrade-never-crash: any
        // failure leaves s_pGameEventManager null -> event ops no-op.
        {
            std::string sigErr;
            auto sigs = LoadSignatures(GamedataPath(), "linuxsteamrt64", sigErr);
            if (!sigErr.empty()) {
                META_CONPRINTF("[s2script] WARN: %s — GameEventManager sig unavailable\n", sigErr.c_str());
            }
            // Slice 6.9: resolve + VALIDATE (unique match) via the gamedata gate, so a stale/moved sig is loud.
            auto it = sigs.find("GameEventManager");
            if (it == sigs.end()) {
                GamedataResult("GameEventManager", false, "signature absent from gamedata");
            } else {
                int64_t targetOff = ResolveSigValidated("GameEventManager", it->second);
                ModText mt = FindModuleText(it->second.module.c_str());
                if (targetOff != s2sig::kFail && mt.text) {
                    s_pGameEventManager = reinterpret_cast<IGameEventManager2*>(
                        const_cast<uint8_t*>(mt.text) + targetOff);
                    META_CONPRINTF("[s2script] interface OK: GameEventManager (%p)\n", (void*)s_pGameEventManager);
                } else {
                    s_pGameEventManager = nullptr;   // ResolveSigValidated already recorded the failure reason
                }
            }
            // Slice 6.6 (Stage 1): resolve CBaseEntity::DispatchTraceAttack (the damage entry) by direct
            // prologue signature and install the read-only detour. Degrade-never-crash: any failure leaves
            // the game unhooked (no damage callback), never a crash.
            auto dit = sigs.find("DispatchTraceAttack");
            if (dit == sigs.end()) {
                GamedataResult("DispatchTraceAttack", false, "signature absent from gamedata");
            } else {
                int64_t dOff = ResolveSigValidated("DispatchTraceAttack", dit->second);
                ModText dmt = FindModuleText(dit->second.module.c_str());
                if (dOff != s2sig::kFail && dmt.text) {  // resolve=="direct": the (unique) match IS the function start
                    void* dtaAddr = const_cast<uint8_t*>(dmt.text) + dOff;
                    if (s2detour::Install(dtaAddr, reinterpret_cast<void*>(&Detour_DispatchTraceAttack),
                                          reinterpret_cast<void**>(&g_origDTA))) {
                        META_CONPRINTF("[s2script] DispatchTraceAttack hooked @%p (read-only)\n", dtaAddr);
                        // Self-test: call the now-patched function with the sentinel `this` — the handler
                        // short-circuits (never runs the original), proving the detour diverts on the live binary.
                        reinterpret_cast<DispatchTraceAttack_t>(dtaAddr)(
                            reinterpret_cast<void*>(kDtaSelfTest), nullptr, nullptr, nullptr);
                    } else {
                        META_CONPRINTF("[s2script] WARN: DispatchTraceAttack detour install failed — damage hook off\n");
                    }
                }   // dOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Slice 6.11b (Stage 1): resolve + detour Host_Say (the chat entry) for player chat triggers.
            // Same direct-prologue + inline-detour pattern as DispatchTraceAttack. Degrade-never-crash:
            // any failure leaves chat unhooked (no triggers), never a crash.
            auto hsit = sigs.find("HostSay");
            if (hsit == sigs.end()) {
                GamedataResult("HostSay", false, "signature absent from gamedata");
            } else {
                int64_t hOff = ResolveSigValidated("HostSay", hsit->second);
                ModText hmt = FindModuleText(hsit->second.module.c_str());
                if (hOff != s2sig::kFail && hmt.text) {  // resolve=="direct": the unique match IS the function start
                    void* hsAddr = const_cast<uint8_t*>(hmt.text) + hOff;
                    if (s2detour::Install(hsAddr, reinterpret_cast<void*>(&Detour_HostSay),
                                          reinterpret_cast<void**>(&g_origHostSay))) {
                        META_CONPRINTF("[s2script] HostSay hooked @%p (chat triggers !cmd / /cmd)\n", hsAddr);
                    } else {
                        META_CONPRINTF("[s2script] WARN: HostSay detour install failed — chat triggers off\n");
                    }
                }   // hOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Slice 6.1d: resolve UTIL_ClientPrintAll (broadcast colored chat). A plain function pointer we
            // call from s2_client_print(slot<0). Degrade-never-crash: unresolved -> Chat.toAll no-op.
            auto cait = sigs.find("ClientPrintAll");
            if (cait == sigs.end()) {
                GamedataResult("ClientPrintAll", false, "signature absent from gamedata");
            } else {
                int64_t caOff = ResolveSigValidated("ClientPrintAll", cait->second);
                ModText camt = FindModuleText(cait->second.module.c_str());
                if (caOff != s2sig::kFail && camt.text) {
                    g_ClientPrintAll = reinterpret_cast<ClientPrintAll_t>(const_cast<uint8_t*>(camt.text) + caOff);
                    META_CONPRINTF("[s2script] ClientPrintAll resolved @%p (broadcast colored chat)\n", reinterpret_cast<void*>(g_ClientPrintAll));
                }   // caOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Slice 6.14: resolve CBasePlayerPawn::CommitSuicide (the lethal-kill entry; sm_slay). A DIRECT
            // prologue signature self-resolved on OUR libserver.so (NOT the ModSharp vtable index, which was
            // version-wrong on the pinned build). Store the fn ptr + libserver's .text range for the call-site
            // guard. Degrade-never-crash: unresolved -> pawn_commit_suicide no-op.
            auto csit = sigs.find("CommitSuicide");
            if (csit == sigs.end()) {
                GamedataResult("CommitSuicide", false, "signature absent from gamedata");
            } else {
                int64_t csOff = ResolveSigValidated("CommitSuicide", csit->second);
                ModText csmt = FindModuleText(csit->second.module.c_str());
                if (csOff != s2sig::kFail && csmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pCommitSuicide = reinterpret_cast<CommitSuicide_t>(const_cast<uint8_t*>(csmt.text) + csOff);
                    s_serverText = csmt.text; s_serverTextSize = csmt.size;   // .text range for the call guard
                    META_CONPRINTF("[s2script] CommitSuicide resolved @%p (sm_slay; libserver .text=%p+%zu)\n",
                                   reinterpret_cast<void*>(s_pCommitSuicide), (const void*)s_serverText, s_serverTextSize);
                }   // csOff == kFail: ResolveSigValidated already recorded the reason
            }
        }
        // Load the engine-identity offsets (Slice 5D.2). Absent/typoed keys stay -1 -> degrade.
        {
            std::string offErr;
            auto offs = LoadOffsets(GamedataPath(), "linuxsteamrt64", offErr);
            auto pick = [&](const char* k) { auto i = offs.find(k); return i != offs.end() ? i->second : -1; };
            s_offGameServer  = pick("NetworkServerService.gameServer");
            s_offClientCount = pick("NetworkGameServer.clientCount");
            s_offClientElems = pick("NetworkGameServer.clientElems");
            s_offSscName     = pick("ServerSideClient.name");
            s_offSscSignon   = pick("ServerSideClient.signon");
            s_offSscUserId   = pick("ServerSideClient.userId");
            META_CONPRINTF("[s2script] identity offsets: gs=%d cnt=%d elems=%d name=%d signon=%d uid=%d\n",
                           s_offGameServer, s_offClientCount, s_offClientElems,
                           s_offSscName, s_offSscSignon, s_offSscUserId);
            // Slice 6.9: record offset presence in the gamedata report (a -1 = a missing/typo'd key). A
            // deeper deref-sanity check (does the offset point to a sane value) is a follow-up per re-strategy.
            for (auto kv : { std::make_pair("NetworkServerService.gameServer", s_offGameServer),
                             std::make_pair("NetworkGameServer.clientCount",   s_offClientCount),
                             std::make_pair("NetworkGameServer.clientElems",   s_offClientElems),
                             std::make_pair("ServerSideClient.name",           s_offSscName),
                             std::make_pair("ServerSideClient.signon",         s_offSscSignon),
                             std::make_pair("ServerSideClient.userId",         s_offSscUserId) })
                GamedataResult(kv.first, kv.second >= 0, "offset key absent from gamedata");
        }
        GamedataBanner();   // Slice 6.9: loud pass/fail summary — a version mismatch screams here, not later.
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
    // Engine-identity ops (Slice 5D.2): order MUST match S2EngineOps in s2script_core.h + Rust v8host.rs.
    ops.client_valid          = &s2_client_valid;
    ops.client_userid         = &s2_client_userid;
    ops.client_signon         = &s2_client_signon;
    ops.client_name           = &s2_client_name;
    ops.client_find_by_userid = &s2_client_find_by_userid;
    // Event write/fire ops (Slice 5D.3): order MUST match S2EngineOps in s2script_core.h + Rust v8host.rs.
    ops.event_set_int    = &s2_event_set_int;
    ops.event_set_float  = &s2_event_set_float;
    ops.event_set_bool   = &s2_event_set_bool;
    ops.event_set_string = &s2_event_set_string;
    ops.event_set_uint64 = &s2_event_set_uint64;
    ops.event_create     = &s2_event_create;
    ops.event_fire       = &s2_event_fire;
    // Config ops (Slice 5E.2): order MUST match S2EngineOps in s2script_core.h + Rust v8host.rs.
    ops.config_read  = &s2_config_read;
    ops.config_write = &s2_config_write;
    // Chat messaging (Slice 6.1): APPENDED after config ops; order MUST match S2EngineOps.
    ops.client_print = &s2_client_print;
    // Client SteamID (Slice 6.2): APPENDED after client_print; order MUST match S2EngineOps.
    ops.client_steamid = &s2_client_steamid;
    // Client kick (Slice 6.3): APPENDED after client_steamid; order MUST match S2EngineOps.
    ops.client_kick = &s2_client_kick;
    // Server command + map-validity (Slice 6.4): APPENDED after client_kick; order MUST match S2EngineOps.
    ops.server_command   = &s2_server_command;
    ops.server_map_valid = &s2_server_map_valid;
    ops.damage_read_float  = &s2_damage_read_float;
    ops.damage_read_int    = &s2_damage_read_int;
    ops.damage_write_float = &s2_damage_write_float;
    ops.damage_victim      = &s2_damage_victim;
    ops.cvar_get           = &s2_cvar_get;
    // Pawn suicide (Slice 6.14): APPENDED after cvar_get; order MUST match S2EngineOps.
    ops.pawn_commit_suicide = &s2_pawn_commit_suicide;
    // Console print + client address (ban-reason sub-project 2): APPENDED after pawn_commit_suicide; order MUST match S2EngineOps.
    ops.client_console_print = &s2_client_console_print;
    ops.client_address       = &s2_client_address;
    // Server-info ops (reservedslots+basetriggers): APPENDED after client_address; order MUST match S2EngineOps.
    ops.server_max_clients = &s2_server_max_clients;
    ops.server_map_name    = &s2_server_map_name;
    ops.server_game_time   = &s2_server_game_time;

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

    // Remove the FireEvent pre-hook (Slice 5D.3) before tearing down the event listener.
    if (m_eventHookInstalled && s_pGameEventManager) {
        SH_REMOVE_HOOK(IGameEventManager2, FireEvent, s_pGameEventManager,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_FireEventPre), false);
        m_eventHookInstalled = false;
    }

    // Remove the ClientCommand hook (Slice 6.11c).
    if (m_clientCmdHookInstalled && m_gameClients) {
        SH_REMOVE_HOOK(ISource2GameClients, ClientCommand, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientCommand), false);
        m_clientCmdHookInstalled = false;
    }

    // Remove the six client lifecycle notify-hooks (@s2script/clients).
    if (m_clientLifecycleHooksInstalled && m_gameClients) {
        SH_REMOVE_HOOK(ISource2GameClients, OnClientConnected, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_OnClientConnected), false);
        SH_REMOVE_HOOK(ISource2GameClients, ClientPutInServer, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientPutInServer), false);
        SH_REMOVE_HOOK(ISource2GameClients, ClientActive, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientActive), false);
        SH_REMOVE_HOOK(ISource2GameClients, ClientFullyConnect, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientFullyConnect), false);
        SH_REMOVE_HOOK(ISource2GameClients, ClientDisconnect, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientDisconnect), false);
        SH_REMOVE_HOOK(ISource2GameClients, ClientSettingsChanged, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientSettingsChanged), false);
        m_clientLifecycleHooksInstalled = false;
    }

    // Slice 6.6: restore the DispatchTraceAttack prologue (removes the damage detour) before core teardown.
    s2detour::RemoveAll();

    // Unregister the game-event listener before core shutdown (Slice 5D.1).
    // RemoveListener is an all-names call per the SDK — one call removes the listener
    // from every subscribed event.  Degrade-never-crash: null manager → skip.
    if (s_pGameEventManager) {
        s_pGameEventManager->RemoveListener(&s_eventListener);
        s_pGameEventManager = nullptr;
    }
    s_subscribedNames.clear();

    // Unregister our ConCommands before core shutdown (Slice 6.1). Metamod dlclose's s2script.so,
    // unmapping s2_concommand_trampoline — but the engine's ICvar still holds m_CBInfo pointing at it,
    // so invoking a ghost command post-unload would call into freed .text (UAF/crash). Parity with the
    // event-listener RemoveListener above. Degrade-never-crash: null ICvar → skip.
    if (s_pCvar) {
        for (auto& kv : s_concommandRefs) {
            if (kv.second.IsValidRef()) s_pCvar->UnregisterConCommandCallbacks(kv.second);
        }
    }
    s_concommandRefs.clear();

    s2script_core_shutdown();
    return true;
}

// ---------------------------------------------------------------------------
// SourceHook hook handlers
// ---------------------------------------------------------------------------
void S2ScriptPlugin::Hook_GameFramePre(bool simulating, bool first, bool last) {
    // Slice 6.6 Stage-2 self-test: fire a synthetic damage dispatch over a fake CTakeDamageInfo
    // (m_flDamage@68 = 42) to prove detour->core mux->JS handler->schema read end-to-end (combat is
    // un-generatable on the bots-only gate). GATED OFF by default: it fires plugins' Damage.onPre handlers
    // with FAKE data, so it must NOT run in production — set S2_DAMAGE_SELFTEST=1 to opt in for verification.
    // Fired at a few LATER frames (frame 1 caught the plugin mid boot-reload with no live subscriber).
    static bool s_dmgSelfTestOn = (getenv("S2_DAMAGE_SELFTEST") != nullptr);
    static long s_frameNo = 0;
    ++s_frameNo;
    if (s_dmgSelfTestOn && (s_frameNo == 300 || s_frameNo == 900 || s_frameNo == 1800) && g_origDTA) {
        static char fakeInfo[256];
        memset(fakeInfo, 0, sizeof(fakeInfo));
        *reinterpret_cast<float*>(fakeInfo + 68) = 42.0f;   // CTakeDamageInfo::m_flDamage
        s_currentDamageInfo = fakeInfo;
        void* victimEnt = nullptr;                          // scan for a REAL entity (idx 1+) -> proves the victim path
        for (int i = 1; i < 128 && !victimEnt; ++i) victimEnt = s2_ent_by_index(i);
        s_currentDamageVictim = victimEnt;
        META_CONPRINTF("[s2script] damage self-test (frame %ld): synthetic damage (m_flDamage=42, victim=%p, raw=%d)\n",
                       s_frameNo, victimEnt, s2_damage_victim());
        s2script_core_dispatch_damage();
        s_currentDamageInfo = nullptr;
        s_currentDamageVictim = nullptr;
    }
    s2script_core_dispatch_game_frame(0, static_cast<int>(simulating),
                                      static_cast<int>(first), static_cast<int>(last));
    RETURN_META(MRES_IGNORED);
}

void S2ScriptPlugin::Hook_GameFramePost(bool simulating, bool first, bool last) {
    s2script_core_dispatch_game_frame(1, static_cast<int>(simulating),
                                      static_cast<int>(first), static_cast<int>(last));
    RETURN_META(MRES_IGNORED);
}

// FireEvent Pre hook: run pre-subscribers (they may getX/setX + return a HookResult); if they collapse
// to "suppress broadcast", re-call the original with bDontBroadcast=true and SUPERCEDE.
bool S2ScriptPlugin::Hook_FireEventPre(IGameEvent* ev, [[maybe_unused]] bool bDontBroadcast) {
    if (!ev) RETURN_META_VALUE(MRES_IGNORED, true);
    IGameEvent* prev = s_currentEvent;
    s_currentEvent = ev;                                       // mutable during the pre-dispatch
    int suppress = s2script_core_dispatch_game_event_pre(ev->GetName());
    s_currentEvent = prev;                                     // restore (re-entrancy)
    if (suppress) {
        bool ret = SH_CALL(s_pGameEventManager, &IGameEventManager2::FireEvent)(ev, true);
        RETURN_META_VALUE(MRES_SUPERCEDE, ret);                // we fired it ourselves with broadcast off
    }
    RETURN_META_VALUE(MRES_IGNORED, true);                     // original runs; any set* mods already applied
}

// Slice 6.11c: a player typed a command at the console. Dispatch the matching registered s2script command
// (console runs the SAME command as chat/rcon). If handled, SUPERCEDE so the engine doesn't also process
// it. Not one of ours -> IGNORE (the engine handles it normally). Clean (slot, CCommand) — no detour.
void S2ScriptPlugin::Hook_ClientCommand(CPlayerSlot slot, const CCommand& args) {
    const char* name = args.Arg(0);
    if (!name || !name[0]) RETURN_META(MRES_IGNORED);
    const char* argStr = args.ArgS();
    if (s2script_core_dispatch_client_command(slot.Get(), name, argStr ? argStr : "")) {
        META_CONPRINTF("[s2script] console command '%s' by slot=%d\n", name, slot.Get());
        RETURN_META(MRES_SUPERCEDE);   // ours → engine won't also handle it (no "Unknown command" server-side)
    }
    RETURN_META(MRES_IGNORED);         // not ours → the engine handles it normally
}

// Client lifecycle notify-hooks (@s2script/clients sub-project). Each forwards the player slot to the
// Task-1 dispatch (runs the JS Clients.on(name) subscribers) and RETURN_META(MRES_IGNORED) — notify-only,
// never alters flow. The `uint64` param types match the SH_DECL_HOOK above (== the header's `unsigned long
// long` on Linux). Post-hooks (added `false`).
void S2ScriptPlugin::Hook_OnClientConnected(CPlayerSlot slot, const char*, uint64, const char*, const char*, bool) {
    s2script_core_dispatch_client_event("connect", slot.Get());
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientPutInServer(CPlayerSlot slot, const char*, int, uint64) {
    s2script_core_dispatch_client_event("putinserver", slot.Get());
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientActive(CPlayerSlot slot, bool, const char*, uint64) {
    s2script_core_dispatch_client_event("active", slot.Get());
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientFullyConnect(CPlayerSlot slot) {
    s2script_core_dispatch_client_event("fullyconnect", slot.Get());
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientDisconnect(CPlayerSlot slot, ENetworkDisconnectionReason, const char*, uint64, const char*) {
    s2script_core_dispatch_client_event("disconnect", slot.Get());
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientSettingsChanged(CPlayerSlot slot) {
    s2script_core_dispatch_client_event("settingschanged", slot.Get());
    RETURN_META(MRES_IGNORED);
}

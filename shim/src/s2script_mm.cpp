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
#include <iservernetworkable.h>  // CCheckTransmitInfo (m_pTransmitEntity @0) — checktransmit slice
#include <playerslot.h>   // CPlayerSlot — IVEngineServer2::ClientPrintf target (Slice 6.1b)
#include <inetchannel.h>  // NetChannelBufType_t / BUF_RELIABLE (Slice 6.1c PostEventAbstract)
#include <irecipientfilter.h>   // Sound slice: the modern 4-method IRecipientFilter + CPlayerBitVec
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
#include <sys/mman.h>   // mprotect — Sound slice: patch the CGameRulesGameSystem vtable slot (precache)
#include <sys/stat.h>   // stat/mkdir — crash-reporter slice: gamedata mtime + the crash-spool dir
#include <errno.h>      // errno/EEXIST — crash-reporter slice: CrashSpoolDir's mkdir race tolerance
#include <unistd.h>     // sysconf(_SC_PAGESIZE) — the mprotect page span
#include "sigscan.h"
#include "detour.h"   // Slice 6.6: the self-contained inline detour (damage hook)
#include "vtable.h"   // Ray-trace slice: RTTI vtable-by-name resolution
#include "trace.h"    // Ray-trace slice: Ray_t/CTraceFilterEx/CGameTrace + the TraceShape call
#include "ekv.h"      // EKV slice: S2EKV_Build/AddRef/ReleaseIfSafe/SelfTest (the void*-only surface)
#include "crash_handler.h"  // Crash-reporter slice: S2CrashArm/S2CrashDisarm (Breakpad native fault path)
#include <cstring>
#include <cstdio>
#include <ctime>    // Voice-control slice: time()/time_t for the per-slot ClientVoice notify throttle
#include <cstdlib>   // getenv — the S2_DAMAGE_SELFTEST opt-in gate
#include <ctime>     // clock_gettime — CheckTransmit hot-path timing (checktransmit slice)
#include <fstream>
#include <sstream>
#include <filesystem>
#include <map>
#include <set>
#include <string>
#include <unordered_map>   // the CheckTransmit rule table (checktransmit slice)
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

// Voice-control slice. ClientVoice (eiface.h:619 "TERROR: A player sent a voice packet") = the 7th
// sibling notify hook on m_gameClients — fires PER VOICE PACKET, throttled in the handler before the
// core dispatch. SetClientListening (eiface.h:330) = the CSSharp/Swiftly voice-mute mechanism: a PRE
// hook on s_pEngine that rewrites bListen->false for a muted sender. CAUTION: it sits in a
// HAND-PATCHED eiface.h region ('#if 0 Don't really match the binary' + unk301/302) — behaviorally
// validated at runtime (first-fire sanity + a Get/Set round-trip), named-degrade on mismatch.
SH_DECL_HOOK1_void(ISource2GameClients, ClientVoice, SH_NOATTRIB, 0, CPlayerSlot);                       // :619
SH_DECL_HOOK3(IVEngineServer2, SetClientListening, SH_NOATTRIB, 0, bool, CPlayerSlot, CPlayerSlot, bool); // :330

// GameSessionConfiguration_t is only FORWARD-DECLARED across the whole pinned SDK (iserver.h:43,
// eiface.h:88, iloopmode.h:107, igamesystem.h:43; the one body at iloopmode.h:109 is commented out),
// and the SH_DECL_HOOK3_void macro below applies __SH_GPI(tt) = { sizeof(tt), ... } (sourcehook.h:1081)
// to EVERY param type — so `sizeof(const GameSessionConfiguration_t&)` (the size of the referent)
// requires a COMPLETE type or the shim will not compile at the sniper step. An empty stub definition
// (following the forward decls) makes it complete and is ABI-safe: StartupServer takes it ONLY by
// const-reference, SourceHook passes a ByRef param as a pointer (PassInfo.size is not used to copy the
// referent), and our Hook_StartupServer body never names or dereferences it — no GameSessionConfiguration_t
// is ever constructed, sized-into, or copied in this TU, so there is no interaction with the engine's real
// type. (CCommand-by-ref at :81 compiles only because convar.h makes CCommand complete; this is the first
// hook to pass a forward-declared class by reference.)
class GameSessionConfiguration_t {};

// INetworkServerService::StartupServer (clientlist-fakeconvar-onmapstart slice) — the CSSharp OnMapStart
// mechanism (mm_plugin.cpp:82), verbatim. POST hook only. Signature confirmed against OUR iserver.h:221.
SH_DECL_HOOK3_void(INetworkServerService, StartupServer, SH_NOATTRIB, 0, const GameSessionConfiguration_t&, ISource2WorldSession*, const char*);

// ISource2GameEntities::CheckTransmit (checktransmit slice) — per-client entity visibility. POST
// hook: the game has filled each client's transmit bitvec; we clear bits per the core-pushed rule
// table. Signature verbatim from OUR eiface.h:500 (7 args; the two CBitVec<16384>& are complete
// via bitvec.h, which eiface.h includes; Entity2Networkable_t stays an incomplete pointee — fine,
// SourceHook only sizeof's the pointer). SwiftlyS2 hooks this with the identical declared
// signature (their entrypoint.cpp:74) — corroboration; the vtable index comes from OUR pinned
// hl2sdk at compile time, exactly like the seven ISource2GameClients hooks.
SH_DECL_HOOK7_void(ISource2GameEntities, CheckTransmit, SH_NOATTRIB, 0, CCheckTransmitInfo**, int,
                   CBitVec<16384>&, CBitVec<16384>&, const Entity2Networkable_t**, const uint16*, int);

// UserMessage-interception slice. The 8-arg PostEventAbstract overload — the EXACT method our send
// path calls live (s2_client_print :961 / s2_user_message_send :1066), so the vendored-header vtable
// slot is transitively proven against our binary. Param 7 is `unsigned long` exactly (ABI). SourceHook
// disambiguates from the 6-arg IRecipientFilter overload by the parameter type list — no numeric index.
SH_DECL_HOOK8_void(IGameEventSystem, PostEventAbstract, SH_NOATTRIB, 0,
    CSplitScreenSlot, bool, int, const uint64*,
    INetworkMessageInternal*, const CNetMessage*, unsigned long, NetChannelBufType_t);

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

// ---------------------------------------------------------------------------
// CheckTransmit (checktransmit slice) — the per-entity visibility rule table + layout validation.
// Rules are pushed by the core (transmit_set/transmit_clear ops; AND-merged per entity across
// plugins core-side); the POST hook applies them to each client's transmit bitvec with ZERO JS in
// the hot path. The one non-SDK layout fact (which client an info is for) is a gamedata offset
// validated at FIRST FIRE (the info structs exist only inside a live snapshot build, so boot-time
// validation is impossible): fail-closed — no bit is touched until validation passes. Validation
// decides by EXCLUSIVE WITNESSES (see TransmitValidateLayout): -1/hard-fail ONLY on a range
// violation (garbage read = wrong offset); odd-but-legitimate infos are non-evidence, skipped;
// snapshots with NO tracked clients don't burn the attempt budget (a late-loaded shim stays
// pending until a connect provides data). A persistent failure disables the descriptor with a
// named gamedata FAIL (degrade, never crash).
// ---------------------------------------------------------------------------
struct TransmitEntry { int serial; uint64_t mask; };
static std::unordered_map<int, TransmitEntry> s_transmitTable;   // entindex -> merged rule
static const size_t kTransmitTableCap = 4096;
static int  s_ctiClientOff = -1;   // CCheckTransmitInfo which-client int32 (gamedata; hint +576)
// Layout state: 0 = pending (observe only), 1 = validated, -1 = FAILED (descriptor disabled).
static int  s_transmitLayoutState = 0;
static bool s_transmitClientIsEntIndex = false;  // +off semantics: false = slot, true = entindex (slot+1)
static int  s_transmitValidateAttempts = 0;
static const int kTransmitValidateMaxAttempts = 512;  // EVIDENCING snapshots before FAILED (tracked-client snapshots only)
// Stats out[5]: snapshots, entries (read live), bitsCleared, nsLast, nsMax.
static uint64_t s_transmitSnapshots = 0, s_transmitBitsCleared = 0;
static uint64_t s_transmitNsLast = 0, s_transmitNsMax = 0;

/// Read CGameEntitySystem* fresh from the IGameResourceService* on each call.
/// Returns nullptr when the service pointer or offset is not yet available,
/// or when the field hasn't been written yet (e.g. before the first map load).
static CGameEntitySystem* GetEntitySystem() {
    if (!s_pGameResourceService || s_gameEntitySystemOffset < 0) return nullptr;
    return *reinterpret_cast<CGameEntitySystem**>(
        reinterpret_cast<uintptr_t>(s_pGameResourceService)
        + static_cast<size_t>(s_gameEntitySystemOffset));
}

// EKV slice: non-static bridge so ekv.cpp (the SDK-including TU) can define GameEntitySystem()
// without itself needing s_pGameResourceService/s_gameEntitySystemOffset (file-scope statics here).
CGameEntitySystem* S2_EntitySystemBridge() { return GetEntitySystem(); }

// Entity lifecycle listeners slice: the isolated entity_listener.cpp TU owns the IEntityListener; we
// register/unregister it here via the sig-resolved (this, IEntityListener*) member fns. void* keeps
// entitysystem.h out of this TU.
extern "C" void* S2_GetEntityListener();
using AddRemoveListenerFn = void (*)(void* gameEntitySystem, void* listener);
static AddRemoveListenerFn s_pAddListenerEntity    = nullptr;   // sig-resolved in Load
static AddRemoveListenerFn s_pRemoveListenerEntity = nullptr;   // sig-resolved in Load (best-effort)
static bool               s_wantEntityListener     = false;     // set true by the install op

// Idempotent register: AddListenerEntity guards Find, so re-asserting each map (StartupServer) is safe
// whether the entity system persists across a changelevel or is recreated with a fresh listener list.
static void EnsureEntityListenerRegistered() {
    if (!s_wantEntityListener || !s_pAddListenerEntity) return;
    CGameEntitySystem* es = GetEntitySystem();     // fresh; null before the first map
    if (es) s_pAddListenerEntity(es, S2_GetEntityListener());
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
// Engine-op: find every entity whose designer-name (class) == className (exact,
// case-sensitive — designer-names are canonical). Iterates the entity-identity
// list (the s2_ent_by_index chunk walk), reads CEntityIdentity::m_designerName
// (a CUtlSymbolLarge; String() is inline, utlsymbollarge.h), writes (index,serial)
// for the first maxCount matches, and returns the TOTAL match count (so the caller
// can detect truncation when returned > maxCount). Engine-generic.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static int s2_entity_find_by_class(const char* className, int* outIndices, int* outSerials, int maxCount) {
    if (!className || !outIndices || !outSerials) return 0;
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return 0;
    int found = 0;
    for (int idx = 0; idx < MAX_TOTAL_ENTITIES; ++idx) {
        int chunk = idx / MAX_ENTITIES_IN_LIST;
        int slot  = idx % MAX_ENTITIES_IN_LIST;
        CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
        if (!chunk_base) continue;
        CEntityIdentity* id = &chunk_base[slot];
        if (id->m_flags & EF_IS_INVALID_EHANDLE) continue;
        if (!id->m_pInstance) continue;
        const char* dn = id->m_designerName.String();
        if (!dn || strcmp(dn, className) != 0) continue;
        if (found < maxCount) {
            CEntityHandle h = id->GetRefEHandle();
            outIndices[found] = h.GetEntryIndex();
            outSerials[found] = h.GetSerialNumber();
        }
        ++found;
    }
    return found;
}

// ---------------------------------------------------------------------------
// Engine-op: read an entity's targetname (CEntityIdentity::m_name, a CUtlSymbolLarge;
// String() inline, utlsymbollarge.h). Serial-gated: resolves the identity at `index`,
// validates the captured `serial` via GetRefEHandle(), returns m_name.String() ("" if
// unnamed) or nullptr if stale/invalid/removed. Sibling of s2_entity_find_by_class
// (which reads m_designerName on the same identity). Engine-generic.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static const char* s2_entity_name(int index, int serial) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return nullptr;
    if (index < 0 || index >= MAX_TOTAL_ENTITIES) return nullptr;
    int chunk = index / MAX_ENTITIES_IN_LIST;
    int slot  = index % MAX_ENTITIES_IN_LIST;
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
    if (!chunk_base) return nullptr;
    CEntityIdentity* id = &chunk_base[slot];
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;
    if (!id->m_pInstance) return nullptr;
    if (id->GetRefEHandle().GetSerialNumber() != serial) return nullptr;  // stale slot reuse
    return id->m_name.String();  // "" if the entity has no targetname
}

// ---------------------------------------------------------------------------
// E1 engine-op: resolve (index, engine_serial) -> CEntityInstance*, validating ENTIRELY
// in the system-owned identity chunk (the s2_deref_handle idiom, by pair instead of by
// packed handle). Instance memory is NEVER read to decide liveness — the exact inversion
// of the retired core-side entity_resolve_ptr (which read the serial through the
// instance it was about to return: a use-after-free deciding UAF-safety).
// ---------------------------------------------------------------------------
static void* s2_ent_resolve(int index, int serial) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return nullptr;
    if (index < 0 || index >= MAX_TOTAL_ENTITIES) return nullptr;
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[index / MAX_ENTITIES_IN_LIST];
    if (!chunk_base) return nullptr;
    CEntityIdentity* id = &chunk_base[index % MAX_ENTITIES_IN_LIST];
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;
    if (id->GetRefEHandle().GetSerialNumber() != serial) return nullptr;  // stale slot reuse
    return id->m_pInstance;   // may be null (removal in progress) — caller treats null as not-live
}

// E1 engine-op: identity m_flags read from the SLOT (never instance+0x10). -1 = stale/absent.
// Backs pawn.isValid's EF_IN_STAGING_LIST check without touching instance memory; the flag's
// bit value stays in the game package (engine-generic: raw flags cross the ABI).
static long long s2_ent_identity_flags(int index, int serial) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return -1;
    if (index < 0 || index >= MAX_TOTAL_ENTITIES) return -1;
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[index / MAX_ENTITIES_IN_LIST];
    if (!chunk_base) return -1;
    CEntityIdentity* id = &chunk_base[index % MAX_ENTITIES_IN_LIST];
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return -1;
    if (id->GetRefEHandle().GetSerialNumber() != serial) return -1;
    return (long long)(unsigned int)id->m_flags;
}

// E1 engine-op: books repair sweep — every live identity slot's (index, serial); the
// s2_entity_find_by_class walk minus the class filter. Returns the TOTAL found (the
// caller detects truncation when total > cap). System-owned chunk memory only.
static int s2_ent_snapshot(int* outIndices, int* outSerials, int cap) {
    if (!outIndices || !outSerials || cap <= 0) return 0;
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return 0;
    int found = 0;
    for (int idx = 0; idx < MAX_TOTAL_ENTITIES; ++idx) {
        CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[idx / MAX_ENTITIES_IN_LIST];
        if (!chunk_base) continue;
        CEntityIdentity* id = &chunk_base[idx % MAX_ENTITIES_IN_LIST];
        if (id->m_flags & EF_IS_INVALID_EHANDLE) continue;
        if (!id->m_pInstance) continue;
        if (found < cap) {
            CEntityHandle h = id->GetRefEHandle();
            outIndices[found] = h.GetEntryIndex();
            outSerials[found] = h.GetSerialNumber();
        }
        ++found;
    }
    return found;
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
// trace_shape op (ray-trace slice, Task 1): CNavPhysicsInterface::TraceShape, resolved via RTTI
// (s2vtable::GetVTableByName — CS2 does not export game vtables/symbols). s_pTraceShape is set in
// Load() only after BOTH the RTTI resolve AND a .text-membership validation succeed; null here
// means "unavailable on this binary" and the op degrades (returns 0, *out untouched).
// ---------------------------------------------------------------------------
static s2trace::TraceShapeFn s_pTraceShape = nullptr;

static int s2_trace_shape(const float* start, const float* end, const float* mins, const float* maxs,
                           unsigned long long interactsWith, unsigned long long interactsExclude,
                           int ignoreEntIdx, int ignoreEntSerial, S2TraceResult* out) {
    if (!s_pTraceShape || !start || !end || !mins || !maxs || !out) return 0;

    // Resolve the ignore entity from (idx, serial) via the EXISTING serial-gated chunk-walk
    // (s2_deref_handle) — a raw pointer never crosses to JS; a stale/reused (idx, serial) pair
    // degrades to "no ignore entity" (never a dangling deref), exactly like the damage-victim and
    // pawn-suicide ops.
    CEntityInstance* ignoreEnt = nullptr;
    if (ignoreEntIdx >= 0 && ignoreEntSerial >= 0) {
        CEntityHandle h(ignoreEntIdx, ignoreEntSerial);
        ignoreEnt = static_cast<CEntityInstance*>(s2_deref_handle(static_cast<unsigned int>(h.ToInt())));
    }

    s2trace::S2TraceResultOut r{};
    if (!s2trace::RunTraceShape(s_pTraceShape, start, end, mins, maxs,
                                 static_cast<uint64_t>(interactsWith), static_cast<uint64_t>(interactsExclude),
                                 ignoreEnt, &r)) {
        return 0;
    }
    out->didHit  = r.didHit;
    out->fraction = r.fraction;
    out->endpos[0] = r.endpos[0]; out->endpos[1] = r.endpos[1]; out->endpos[2] = r.endpos[2];
    out->normal[0] = r.normal[0]; out->normal[1] = r.normal[1]; out->normal[2] = r.normal[2];
    out->allSolid  = r.allSolid;
    out->hitEntHandle = r.hitEntHandle;
    return 1;
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
// Engine-identity: TYPED SDK VIRTUALS (clientlist-fakeconvar-onmapstart slice — replaces the 5D.2
// hand-offset walk that went stale on 2000870). s_pNetworkServerService is acquired in Load(); the
// client ops that consume it (and s_pEngine) are defined below, AFTER s_pEngine's declaration.
// Degrade-never-crash: any null -> safe miss.
// ---------------------------------------------------------------------------
static void* s_pNetworkServerService = nullptr;
// Slice menu: GetLegacyGameEventListener(int slot) -> IGameEventListener2* — the CS2 engine helper that
// returns a client's per-client legacy game-event listener (a frameless leaf that indexes a global array
// by slot). Sig-resolved on OUR libserver.so via a DIRECT prologue signature (NOT a CServerSideClient
// offset cast — the earlier offset guess was wrong; CSSharp reaches the listener through THIS function).
// event_fire_to_client calls it, then IGameEventListener2::FireGameEvent. Unresolved -> nullptr ->
// s2_event_fire_to_client degrades to a hard miss (no-op), never a garbage vtable call.
typedef IGameEventListener2* (*GetLegacyListener_t)(int slot);
static GetLegacyListener_t s_pGetLegacyListener = nullptr;

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

// ---------------------------------------------------------------------------
// Engine-identity ops — TYPED SDK VIRTUALS (clientlist-fakeconvar-onmapstart slice; retires the 5D.2
// hand-offset walk that went stale on 2000870). Every read is a compiler-resolved virtual against the
// pinned hl2sdk headers (self-healing on the routine per-update SDK bump — no gamedata, no shim edit
// on a layout move). CSSharp-cross-validated: their player_manager reads userids via GetPlayerUserId
// and tracks occupancy from the lifecycle hooks, never touching CServerSideClient. Placed AFTER the
// s_pEngine declaration above (these call it). Degrade-never-crash: any null -> safe miss.
//
// The one client fact with NO engine virtual is per-slot signon state (nothing on IVEngineServer2 /
// INetworkGameServer exposes it). Tracked from the six ALREADY-INSTALLED ISource2GameClients lifecycle
// hooks (the CSSharp lifecycle-driven model — the design's endorsed approach). Values preserve the two
// observable JS gates: >= 2 = connected (kSignonConnected), >= 4 = in-game (Client.kickWithReason
// deliver-now). This TRACKED signon is ALSO the validity signal (connect fires for bots too, so bots
// read valid — preserving the client_valid "connected incl. bots incl. pawnless" contract).
//   Why not GetPlayerUserId for validity: CPlayerUserId::_index is an `unsigned short`, so `.Get()`
//   returns 0..65535 and can NEVER equal -1 — `userid != -1` would mark every empty slot valid
//   (Clients.all() would yield 64 phantom clients). userid/name gate on the tracked-signon validity.
// ---------------------------------------------------------------------------
static const int kSignonNone = 0, kSignonConnected = 2, kSignonSpawn = 5, kSignonFull = 6;
static const int kMaxClientSlots = 64;
static int s_trackedSignon[kMaxClientSlots] = {0};

// INetworkGameServer via the TYPED virtual (replaces the NetworkServerService.gameServer offset).
// CNetworkGameServerBase (the GetIGameServer return type) IS-A INetworkGameServer.
static INetworkGameServer* S2_GameServer() {
    if (!s_pNetworkServerService) return nullptr;
    return static_cast<INetworkServerService*>(s_pNetworkServerService)->GetIGameServer();
}

static int s2_client_signon(int slot) {
    if (slot < 0 || slot >= kMaxClientSlots) return -1;
    return s_trackedSignon[slot];                        // 0 = never-connected / disconnected
}
static int s2_client_valid(int slot) {
    return s2_client_signon(slot) >= kSignonConnected ? 1 : 0;   // tracked; bots included (connect fires)
}
static int s2_client_userid(int slot) {
    if (!s_pEngine || !s2_client_valid(slot)) return -1;
    return s_pEngine->GetPlayerUserId(CPlayerSlot(slot)).Get();  // real engine user-id for an occupied slot
}
static const char* s2_client_name(int slot) {
    if (!s_pEngine || !s2_client_valid(slot)) return nullptr;
    return s_pEngine->GetClientConVarValue(CPlayerSlot(slot), "name");  // userinfo name; core copies
}
static int s2_client_find_by_userid(int id) {
    if (id < 0) return -1;
    INetworkGameServer* gs = S2_GameServer();
    int n = gs ? gs->GetMaxClients() : kMaxClientSlots;
    if (n <= 0 || n > kMaxClientSlots) n = kMaxClientSlots;
    for (int slot = 0; slot < n; slot++) {
        if (s2_client_userid(slot) == id) return slot;
    }
    return -1;
}

// ---------------------------------------------------------------------------
// Slice menu: per-client event fire (SourceMod FireToClient parity). Fires the event created by
// s2_event_create (s_pendingFireEvent) directly to ONE client's per-client legacy game-event
// listener, i.e. IGameEventListener2::FireGameEvent — this serializes straight to that client's
// netchannel and does NOT pass through IGameEventManager2::FireEvent, so it never re-enters our own
// FireEvent pre-hook / JS dispatch. Bot-skip is EXPLICIT (a valid slot per s2_client_valid includes
// bots — allConnected() reports them — so validity does NOT imply a netchannel): guarded the same way
// as s2_client_print/s2_client_console_print, via
// GetPlayerNetInfo(slot) == null. The per-client listener comes from the sig-resolved engine helper
// s_pGetLegacyListener(slot) (GetLegacyGameEventListener), NOT a CServerSideClient offset cast.
// ---------------------------------------------------------------------------
static int s2_event_fire_to_client(int slot) {
    if (!s_pGameEventManager || !s_pendingFireEvent) return 0;
    // Grab + clear the pending event and restore s_currentEvent UNCONDITIONALLY once we know there is
    // a manager + a pending event — every path below (miss or hit) frees `e` and restores the save/
    // restore nesting exactly once. Doing this BEFORE the client/netinfo checks matters: an early
    // `return 0` on a miss must not leak the CreateEvent()'d event or leave s_currentEvent pointing at
    // an orphaned event (which would corrupt a later, unrelated fire()/fireToClient() call's target).
    IGameEvent* e = s_pendingFireEvent;
    s_pendingFireEvent  = nullptr;
    s_currentEvent      = s_savedCurrentEvent;  // restore the write target (mirrors s2_event_fire)
    s_savedCurrentEvent = nullptr;
    // Require the sig-resolved listener helper, and bot-skip: a fake client has no netchannel
    // (GetPlayerNetInfo == null) — firing to one is pointless/unsafe (the print-ops precedent). Free the
    // event on every miss path so the CreateEvent()'d event never leaks.
    if (!s_pGetLegacyListener || !s_pEngine || !s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot))) {
        s_pGameEventManager->FreeEvent(e);
        return 0;
    }
    IGameEventListener2* pListener = s_pGetLegacyListener(slot);   // engine helper: slot -> per-client listener
    if (!pListener) {
        s_pGameEventManager->FreeEvent(e);
        return 0;
    }
    pListener->FireGameEvent(e);        // serialize to this client's netchannel (no broadcast)
    s_pGameEventManager->FreeEvent(e);  // FireGameEvent does not consume the event; free it (CSSharp parity)
    return 1;
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
// FakeConVar registration (clientlist-fakeconvar-onmapstart slice). Fills ConVarCreation_t exactly
// as the SDK's CConVar<T>::Register does (name/help/flags + ConVarValueInfo_t(type) + typed
// SetDefaultValue/SetMinValue/SetMaxValue; m_Version stays 0 — verified in tier1/convar.cpp, which
// passes the struct through to ICvar::RegisterConVar untouched), then vtable-calls RegisterConVar
// directly on s_pCvar — bypassing the NON-INLINE tier1 SetupConVar/SanitiseConVarFlags (the 5D.1
// dlopen cascade), the same call-the-interface-directly pattern as RegisterConCommand (6.1).
// s_convarRefs is the name-lifetime anchor + idempotency guard (mirrors s_concommandRefs): the map
// key owns m_pszName's storage; help/default strings persist in the entry. No unregistration — a
// registered cvar persists for the process lifetime (SM parity; no callback into plugin code, so
// no UAF surface on plugin reload).
// ---------------------------------------------------------------------------
struct S2ConVarReg { ConVarRef ref; std::string help; std::string defVal; };
static std::map<std::string, S2ConVarReg> s_convarRefs;
// CVValue_t's string arm is a CUtlString == exactly one char* member; every CUtlString mutator is
// DLL_CLASS_IMPORT (tier0). We therefore set a string default by writing the pointer bytes directly
// (SetDefaultValue<const char*>) into the value slot — guarded here. The engine copies the value
// into its own ConVarData at registration; our buffer additionally persists in s_convarRefs.
static_assert(sizeof(CUtlString) == sizeof(const char*),
              "CUtlString layout changed — string convar default punning is invalid");

static int s2_convar_register(const char* name, const char* help, uint64_t flags, int type,
                              const char* defaultValue, const char* minValue, const char* maxValue) {
    if (!name || !name[0] || !defaultValue) return 0;
    if (!s_pCvar) {
        META_CONPRINTF("[s2script] WARN: ConVar '%s' not registered — ICvar not acquired\n", name);
        return 0;
    }
    auto found = s_convarRefs.find(name);
    if (found != s_convarRefs.end()) return found->second.ref.IsValidRef() ? 1 : 0;  // idempotent (reload-safe)

    auto it = s_convarRefs.emplace(name, S2ConVarReg{}).first;
    S2ConVarReg& reg = it->second;
    reg.help   = help ? help : "s2script convar";
    reg.defVal = defaultValue;

    ConVarCreation_t setup;
    setup.m_pszName       = it->first.c_str();      // persistent (map key) — name-lifetime anchor
    setup.m_pszHelpString = reg.help.c_str();       // persistent (entry)
    // FCVAR_RELEASE: without it a retail CS2 hides the cvar from customers. Caller flags are additive.
    setup.m_nFlags        = flags | FCVAR_RELEASE;

    switch (type) {
        case 0: {   // bool
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_Bool);
            bool v = (reg.defVal == "1" || reg.defVal == "true");
            setup.m_valueInfo.SetDefaultValue<bool>(v);
            break;
        }
        case 1: {   // int32
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_Int32);
            setup.m_valueInfo.SetDefaultValue<int32>(atoi(reg.defVal.c_str()));
            if (minValue) setup.m_valueInfo.SetMinValue<int32>(atoi(minValue));
            if (maxValue) setup.m_valueInfo.SetMaxValue<int32>(atoi(maxValue));
            break;
        }
        case 2: {   // float32
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_Float32);
            setup.m_valueInfo.SetDefaultValue<float>(static_cast<float>(atof(reg.defVal.c_str())));
            if (minValue) setup.m_valueInfo.SetMinValue<float>(static_cast<float>(atof(minValue)));
            if (maxValue) setup.m_valueInfo.SetMaxValue<float>(static_cast<float>(atof(maxValue)));
            break;
        }
        case 3: {   // string — pointer punning into the CUtlString slot (see static_assert above)
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_String);
            setup.m_valueInfo.SetDefaultValue<const char*>(reg.defVal.c_str());  // persistent buffer
            break;
        }
        default:
            s_convarRefs.erase(it);
            return 0;
    }

    ConVarRef ref;
    ConVarData* data = nullptr;
    s_pCvar->RegisterConVar(setup, 0, &ref, &data);
    reg.ref = ref;
    if (ref.IsValidRef()) {
        META_CONPRINTF("[s2script] ConVar '%s' registered (accessIdx=%u)\n",
                       it->first.c_str(), (unsigned)ref.GetAccessIndex());
        return 1;
    }
    META_CONPRINTF("[s2script] WARN: ConVar '%s' — RegisterConVar returned invalid ref\n", it->first.c_str());
    return 0;   // entry stays in the map (invalid ref) to prevent retry loops
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
// General user messages (Game-rules + UserMessage slice) — generalize the SayText2
// reflection path above to an arbitrary named protobuf user message + arbitrary scalar
// fields set by reflection cpp_type, then PostEventAbstract to the given slots.
//
// A single build-then-send target (mirrors the 5D.3 s_currentEvent single-target model):
// create -> set* -> send is one synchronous JS burst with no await between, so there is
// no cross-message aliasing. Engine-generic — the message NAME is the caller's.
//
// LEAK-TODO (mirrors the 6.1c SayText2 note): ownership of pData after PostEventAbstract
// is unconfirmed (no DeallocateMessage seen), so we do NOT free it — a double-free is worse
// than a bounded per-send leak. create() also drops (does not free) any prior unsent message.
// ---------------------------------------------------------------------------
static INetworkMessageInternal* s_umInfo = nullptr;   // the message factory
static CNetMessage*             s_umData = nullptr;    // the allocated CNetMessage
static google::protobuf::Message* s_umMsg = nullptr;   // its protobuf Message view

static int s2_user_message_create(const char* name) {
    s_umInfo = nullptr; s_umData = nullptr; s_umMsg = nullptr;   // drop any prior unsent (bounded leak-TODO)
    if (!name || !s_pNetworkMessages) return 0;
    INetworkMessageInternal* info = s_pNetworkMessages->FindNetworkMessagePartial(name);
    if (!info) return 0;
    CNetMessage* data = info->AllocateMessage();
    if (!data) return 0;
    google::protobuf::Message* m = reinterpret_cast<google::protobuf::Message*>(data->AsProto());
    if (!m) return 0;
    s_umInfo = info; s_umData = data; s_umMsg = m;
    return 1;
}
static int s2_user_message_set_int(const char* field, int64_t value) {
    if (!s_umMsg || !field) return 0;
    const google::protobuf::Descriptor* d = s_umMsg->GetDescriptor();
    const google::protobuf::Reflection*  r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(field);
    if (!f || f->is_repeated()) return 0;   // repeated -> a scalar Set*() would abort the process (protobuf FATAL)
    using FD = google::protobuf::FieldDescriptor;
    switch (f->cpp_type()) {
        case FD::CPPTYPE_INT32:  r->SetInt32(s_umMsg, f, (int32_t)value);   break;
        case FD::CPPTYPE_UINT32: r->SetUInt32(s_umMsg, f, (uint32_t)value); break;
        case FD::CPPTYPE_INT64:  r->SetInt64(s_umMsg, f, (int64_t)value);   break;
        case FD::CPPTYPE_UINT64: r->SetUInt64(s_umMsg, f, (uint64_t)value); break;
        case FD::CPPTYPE_ENUM:   r->SetEnumValue(s_umMsg, f, (int)value);  break;
        case FD::CPPTYPE_BOOL:   r->SetBool(s_umMsg, f, value != 0);        break;
        case FD::CPPTYPE_FLOAT:  r->SetFloat(s_umMsg, f, (float)value);     break;
        case FD::CPPTYPE_DOUBLE: r->SetDouble(s_umMsg, f, (double)value);   break;
        default: return 0;
    }
    return 1;
}
static int s2_user_message_set_float(const char* field, double value) {
    if (!s_umMsg || !field) return 0;
    const google::protobuf::Descriptor* d = s_umMsg->GetDescriptor();
    const google::protobuf::Reflection*  r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(field);
    if (!f || f->is_repeated()) return 0;   // repeated -> a scalar Set*() would abort the process (protobuf FATAL)
    using FD = google::protobuf::FieldDescriptor;
    if (f->cpp_type() == FD::CPPTYPE_FLOAT)  { r->SetFloat(s_umMsg, f, (float)value); return 1; }
    if (f->cpp_type() == FD::CPPTYPE_DOUBLE) { r->SetDouble(s_umMsg, f, value);       return 1; }
    return 0;
}
static int s2_user_message_set_string(const char* field, const char* value) {
    if (!s_umMsg || !field) return 0;
    const google::protobuf::Descriptor* d = s_umMsg->GetDescriptor();
    const google::protobuf::Reflection*  r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(field);
    if (!f || f->is_repeated()) return 0;   // repeated -> a scalar Set*() would abort the process (protobuf FATAL)
    if (f->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING) return 0;
    r->SetString(s_umMsg, f, value ? value : "");
    return 1;
}
static int s2_user_message_set_bool(const char* field, int value) {
    if (!s_umMsg || !field) return 0;
    const google::protobuf::Descriptor* d = s_umMsg->GetDescriptor();
    const google::protobuf::Reflection*  r = s_umMsg->GetReflection();
    if (!d || !r) return 0;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(field);
    if (!f || f->is_repeated()) return 0;   // repeated -> a scalar Set*() would abort the process (protobuf FATAL)
    if (f->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_BOOL) return 0;
    r->SetBool(s_umMsg, f, value != 0);
    return 1;
}
static int s2_user_message_send(const int* slots, int slotCount) {
    if (!s_umMsg || !s_umInfo || !s_umData || !s_pGameEventSystem) {
        s_umInfo = nullptr; s_umData = nullptr; s_umMsg = nullptr; return 0;
    }
    uint64 clients = 0;
    if (slotCount < 0) {                                   // broadcast to all live non-bot slots
        for (int s = 0; s < 64; ++s)
            if (s_pEngine && s_pEngine->GetPlayerNetInfo(CPlayerSlot(s))) clients |= (1ull << (uint64)s);
    } else if (slots) {
        for (int i = 0; i < slotCount; ++i) {
            int s = slots[i];
            if (s < 0 || s >= 64) continue;
            if (s_pEngine && !s_pEngine->GetPlayerNetInfo(CPlayerSlot(s))) continue;   // skip bots (would crash)
            clients |= (1ull << (uint64)s);
        }
    }
    int ok = 0;
    if (clients != 0) {
        s_pGameEventSystem->PostEventAbstract(-1, false, 64, &clients, s_umInfo, s_umData, 0, BUF_RELIABLE);
        ok = 1;
    }
    s_umInfo = nullptr; s_umData = nullptr; s_umMsg = nullptr;   // clear the single target after send (leak-TODO: pData)
    return ok;
}

// ---------------------------------------------------------------------------
// UserMessage-interception slice. Doctrine: the ONE borrowed layout fact is
// NetMessageInfo_t::m_MessageId (inetworkserializer.h:53 — never exercised by the send path);
// validated fail-closed at subscribe (round-trip below) and on an observe-only first fire.
// Hot path: a bitmap test on the id, MRES_IGNORED on miss before ANY reflection.
// Block-scoped view statics are SEPARATE from the send builder's s_umInfo/s_umData/s_umMsg above,
// so a handler that builds+sends a NEW user message mid-hook cannot retarget the intercepted view.
// ---------------------------------------------------------------------------
static constexpr int kUserMsgMaxId = 2048;
static uint64_t s_userMsgSubBits[kUserMsgMaxId / 64] = {0};
static bool     s_userMsgHookInstalled = false;   // lazy SH_ADD_HOOK on first-ever sub
static bool     s_userMsgFirstFireDone = false;   // observe-only validation ran on the first subscribed fire
static bool     s_inUserMsgDispatch = false;      // recursion guard (a mid-hook send re-enters PostEventAbstract)
static google::protobuf::Message* s_hookMsg = nullptr;      // current intercepted message (block-scoped)
static const uint64*              s_hookClients = nullptr;  // its recipient mask (null = broadcast); uint64 == the hook param type
static int                        s_hookClientCount = 0;

static inline bool s2_usermsg_bit(int id) {
    return id >= 0 && id < kUserMsgMaxId && (s_userMsgSubBits[id >> 6] & (1ull << (id & 63)));
}

// Dotted-path walk: returns the leaf's parent message + writes the leaf field name. Every sub-message
// hop is guarded (CPPTYPE_MESSAGE, !is_repeated) — a scalar Get* on a repeated field is a protobuf
// FATAL that aborts the process (the shipping s2_user_message_set_* guards, mirrored). nullptr on a miss.
static const google::protobuf::Message* s2_usermsg_walk(const google::protobuf::Message* m,
                                                        const char* path, std::string& leaf) {
    if (!m || !path) return nullptr;
    std::string p(path);
    const google::protobuf::Message* cur = m;
    size_t dot;
    while ((dot = p.find('.')) != std::string::npos) {
        std::string seg = p.substr(0, dot);
        p = p.substr(dot + 1);
        const google::protobuf::Descriptor* d = cur->GetDescriptor();
        const google::protobuf::Reflection*  r = cur->GetReflection();
        if (!d || !r) return nullptr;
        const google::protobuf::FieldDescriptor* f = d->FindFieldByName(seg);
        if (!f || f->is_repeated()) return nullptr;                                 // repeated hop -> FATAL guard
        if (f->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_MESSAGE) return nullptr;
        cur = &r->GetMessage(*cur, f);
        if (!cur) return nullptr;
    }
    leaf = p;
    return cur;
}

// Subscribe-time validation (spec §2.1): resolve via the live-proven SayText2 path, then require a
// non-null NetMessageInfo, an id in [0,2048), and the requested name a substring of the canonical
// unscoped name. Any failure -> named USERMSG reason logged + return -1 (onPre throws plugin-side).
// On the first-ever OK sub, lazily SH_ADD_HOOK PostEventAbstract on the already-held s_pGameEventSystem.
static int s2_usermsg_hook_sub(const char* name, char* canonicalOut, int canonicalLen) {
    if (!name || !s_pNetworkMessages || !s_pGameEventSystem) return -1;
    INetworkMessageInternal* info = s_pNetworkMessages->FindNetworkMessagePartial(name);
    if (!info) { META_CONPRINTF("[s2script] USERMSG sub FAILED: no message matches '%s'\n", name); return -1; }
    const NetMessageInfo_t* mi = info->GetNetMessageInfo();
    if (!mi) { META_CONPRINTF("[s2script] USERMSG descriptor 'message-id-extract' FAILED: "
                              "GetNetMessageInfo null for '%s'\n", name); return -1; }
    int id = (int)mi->m_MessageId;
    const char* canonical = info->GetUnscopedName();
    if (id < 0 || id >= kUserMsgMaxId || !canonical || !*canonical || !strstr(canonical, name)) {
        META_CONPRINTF("[s2script] USERMSG descriptor 'message-id-extract' FAILED: '%s' -> id=%d "
                       "canonical='%s' (out of range or name mismatch — header layout drift?)\n",
                       name, id, canonical ? canonical : "(null)");
        return -1;
    }
    if (canonicalOut && canonicalLen > 0) snprintf(canonicalOut, (size_t)canonicalLen, "%s", canonical);
    if (!s_userMsgHookInstalled) {   // lazy install, idempotent (m_eventHookInstalled pattern; PRE = false)
        SH_ADD_HOOK(IGameEventSystem, PostEventAbstract, s_pGameEventSystem,
                    SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_PostEvent), false);
        s_userMsgHookInstalled = true;
        META_CONPRINTF("[s2script] usermsg: PostEventAbstract hook installed (lazy, first subscribe)\n");
    }
    s_userMsgSubBits[id >> 6] |= (1ull << (id & 63));
    return id;
}
static int s2_usermsg_hook_unsub(int id) {
    if (id < 0 || id >= kUserMsgMaxId) return 0;
    s_userMsgSubBits[id >> 6] &= ~(1ull << (id & 63));
    return 1;
}

// Read ops — Get* reflection mirrors of the shipping s2_user_message_set_* setters, each null-guarded on
// the block-scoped s_hookMsg and carrying the is_repeated() FATAL guard on the leaf field.
static int s2_usermsg_hook_read_int(const char* path, long long* out) {
    if (!s_hookMsg || !path || !out) return 0;
    std::string leaf;
    const google::protobuf::Message* m = s2_usermsg_walk(s_hookMsg, path, leaf);
    if (!m) return 0;
    const google::protobuf::Descriptor* d = m->GetDescriptor();
    const google::protobuf::Reflection*  r = m->GetReflection();
    if (!d || !r) return 0;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(leaf);
    if (!f || f->is_repeated()) return 0;
    using FD = google::protobuf::FieldDescriptor;
    switch (f->cpp_type()) {
        // UINT32 is protobuf's cpp_type for BOTH uint32 AND fixed32 (the `player` field is fixed32); widen
        // as UNSIGNED so an entity-handle's high bits survive (a signed GetInt32 would sign-extend + corrupt).
        case FD::CPPTYPE_UINT32: *out = (long long)(unsigned long long)r->GetUInt32(*m, f); return 1;
        case FD::CPPTYPE_INT32:  *out = (long long)r->GetInt32(*m, f);                       return 1;
        case FD::CPPTYPE_ENUM:   *out = (long long)r->GetEnumValue(*m, f);                   return 1;
        case FD::CPPTYPE_BOOL:   *out = r->GetBool(*m, f) ? 1 : 0;                           return 1;
        // int64/uint64/fixed64/sfixed64: DELIBERATELY unsupported. readInt marshals through an f64 (exact only
        // up to 2^53) and the locked 64-bit doctrine is decimal-string, never a lossy number; no TTT consumer
        // reads 64-bit (spec §10 deferred). default → 0 → the native returns null, never a truncated int.
        default: return 0;
    }
}
static int s2_usermsg_hook_read_float(const char* path, double* out) {
    if (!s_hookMsg || !path || !out) return 0;
    std::string leaf;
    const google::protobuf::Message* m = s2_usermsg_walk(s_hookMsg, path, leaf);
    if (!m) return 0;
    const google::protobuf::Descriptor* d = m->GetDescriptor();
    const google::protobuf::Reflection*  r = m->GetReflection();
    if (!d || !r) return 0;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(leaf);
    if (!f || f->is_repeated()) return 0;
    using FD = google::protobuf::FieldDescriptor;
    if (f->cpp_type() == FD::CPPTYPE_FLOAT)  { *out = (double)r->GetFloat(*m, f); return 1; }
    if (f->cpp_type() == FD::CPPTYPE_DOUBLE) { *out = r->GetDouble(*m, f);        return 1; }
    return 0;
}
static int s2_usermsg_hook_read_string(const char* path, char* buf, int buflen) {
    if (!s_hookMsg || !path || !buf || buflen <= 0) return -1;
    std::string leaf;
    const google::protobuf::Message* m = s2_usermsg_walk(s_hookMsg, path, leaf);
    if (!m) return -1;
    const google::protobuf::Descriptor* d = m->GetDescriptor();
    const google::protobuf::Reflection*  r = m->GetReflection();
    if (!d || !r) return -1;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(leaf);
    if (!f || f->is_repeated()) return -1;
    if (f->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING) return -1;
    std::string s = r->GetString(*m, f);
    int n = (int)s.size();
    if (n > buflen - 1) n = buflen - 1;
    memcpy(buf, s.data(), (size_t)n);
    buf[n] = 0;
    return n;
}
static int s2_usermsg_hook_has_field(const char* path) {
    if (!s_hookMsg) return -1;
    if (!path) return 0;
    std::string leaf;
    const google::protobuf::Message* m = s2_usermsg_walk(s_hookMsg, path, leaf);
    if (!m) return 0;
    const google::protobuf::Descriptor* d = m->GetDescriptor();
    const google::protobuf::Reflection*  r = m->GetReflection();
    if (!d || !r) return 0;
    const google::protobuf::FieldDescriptor* f = d->FindFieldByName(leaf);
    if (!f) return 0;
    if (f->is_repeated()) return 1;                       // exists-by-definition; can't HasField a repeated (FATAL)
    if (f->has_presence()) return r->HasField(*m, f) ? 1 : 0;
    return 1;                                             // implicit-presence scalar: always "there"
}
static int s2_usermsg_hook_recipients(unsigned long long* outMask) {
    if (!s_hookMsg || !outMask) return 0;
    if (s_hookClients) { *outMask = *s_hookClients; return 1; }   // bit N = slot N (as our send builds it)
    unsigned long long mask = 0;                                   // broadcast (clients==null): all valid slots
    for (int s = 0; s < 64; ++s)
        if (s2_client_valid(s)) mask |= (1ull << (unsigned)s);
    *outMask = mask;
    return 1;
}
static int s2_usermsg_hook_debug(char* buf, int buflen) {
    if (!s_hookMsg || !buf || buflen <= 0) return -1;
    std::string s = s_hookMsg->DebugString();
    int n = (int)s.size();
    if (n > buflen - 1) n = buflen - 1;
    memcpy(buf, s.data(), (size_t)n);
    buf[n] = 0;
    return n;
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
// Voice-control slice. The mute is FRAMEWORK state (CSSharp keeps CPlayer::m_voiceFlag the same way —
// no engine/schema mute bit exists): a shim-resident flag array consulted by the SetClientListening
// PRE hook, which rewrites bListen->false whenever the SENDER is muted. The hook fires per
// (receiver, sender) pair per game voice refresh (up to O(n^2)) — everything here is plain array
// reads, no FFI/JS/allocations. Doctrine: the vtable slots come from a hand-patched eiface.h region,
// so enforcement is gated on runtime validation (first-fire arg sanity + a one-shot Get/Set
// round-trip once two clients are active); any failure -> named degrade, ops return 0/-1.
// ---------------------------------------------------------------------------
static uint8_t s_voiceMuted[kMaxClientSlots] = {0};        // 1 = sender muted for all receivers
static time_t  s_voiceLastNotify[kMaxClientSlots] = {0};   // per-slot ClientVoice throttle (<=1/s)
static bool    s_voiceNotifyHookInstalled = false;         // ClientVoice POST hook on m_gameClients
static bool    s_voiceListenHookInstalled = false;         // SetClientListening PRE hook on s_pEngine
static bool    s_voiceListenSeen = false;                  // first engine call observed (sanity-checked)
static bool    s_voiceListenValidated = false;             // Get/Set round-trip passed
static bool    s_voiceListenDegraded = false;              // NAMED degrade: rewrite + ops disabled

// One-shot behavioral validation of the hand-patched Get/SetClientListening vtable slots (the
// ChangeTeam 102-vs-101 drift lesson): flip one (receiver, sender) listen bit both ways and read it
// back through the ADJACENT virtual. Runs from Hook_ClientActive once two clients (bots count) are
// active; retried on every activation until it can run. Skips muted slots so our own pre-hook's
// rewrite can't fake a mismatch. Pass -> proactive-apply enabled; fail -> named degrade.
static void MaybeValidateVoiceListening() {
    if (s_voiceListenValidated || s_voiceListenDegraded || !s_voiceListenHookInstalled || !s_pEngine) return;
    int a = -1, b = -1;
    for (int i = 0; i < kMaxClientSlots; i++) {
        if (!s2_client_valid(i) || s_voiceMuted[i]) continue;
        if (a < 0) a = i; else { b = i; break; }
    }
    if (b < 0) return;   // need two un-muted occupied slots; try again on the next ClientActive
    // ENGINE PARAM TRANSPOSE (self-resolved on build 2000875, libengine2.so CEngineServer vtable via
    // RTTI — GetClientListening is slot 90, SetClientListening 91, NEITHER drifted). The engine's
    // getter and setter address OPPOSITE cells of the per-(owner,bit) listen matrix at client+0xbc8:
    //   SetClientListening(a,b,v)  writes cell(owner=b, bit=a)
    //   GetClientListening(a,b)    reads  cell(owner=a, bit=b)
    // The vendored eiface.h declares both (iReceiver,iSender), but the real getter's params are
    // reversed vs the setter. So read back Set(a,b)'s cell with Get(b,a). (The mute-enforcement path
    // s2_voice_set_muted already uses Set the engine's way and is unaffected.)
    bool orig     = s_pEngine->GetClientListening(CPlayerSlot(b), CPlayerSlot(a));
    s_pEngine->SetClientListening(CPlayerSlot(a), CPlayerSlot(b), !orig);
    bool flipped  = s_pEngine->GetClientListening(CPlayerSlot(b), CPlayerSlot(a));
    s_pEngine->SetClientListening(CPlayerSlot(a), CPlayerSlot(b), orig);
    bool restored = s_pEngine->GetClientListening(CPlayerSlot(b), CPlayerSlot(a));
    if (flipped == !orig && restored == orig) {
        s_voiceListenValidated = true;
        META_CONPRINTF("[s2script] VOICE VALIDATION: Get/SetClientListening round-trip OK (slots %d,%d)\n", a, b);
    } else {
        s_voiceListenDegraded = true;
        META_CONPRINTF("[s2script] VOICE VALIDATION FAILED: SetClientListening round-trip mismatch "
                       "(orig=%d flipped=%d restored=%d) — Get/SetClientListening slots moved on this "
                       "build; voice mute DISABLED (voiceMuted is inert)\n", (int)orig, (int)flipped, (int)restored);
    }
}

// voice_set_muted op. Records the flag, then (mute only, only once the round-trip PROVED the vtable
// slots) proactively forces listen=false for every current receiver so the mute doesn't wait for the
// engine's next voice refresh. Our own PRE hook sees these calls harmlessly (param already false).
// Unmute is engine-paced: the game's next refresh restores its own truth (a laggy unmute is benign).
static int s2_voice_set_muted(int slot, int muted) {
    if (slot < 0 || slot >= kMaxClientSlots) return 0;
    s_voiceMuted[slot] = muted ? 1 : 0;
    if (!s_voiceListenHookInstalled || s_voiceListenDegraded) return 0;   // recorded but inert
    if (muted && s_voiceListenValidated && s_pEngine) {
        for (int r = 0; r < kMaxClientSlots; r++) {
            if (r == slot || !s2_client_valid(r)) continue;
            s_pEngine->SetClientListening(CPlayerSlot(r), CPlayerSlot(slot), false);
        }
    }
    return 1;
}
static int s2_voice_get_muted(int slot) {
    if (slot < 0 || slot >= kMaxClientSlots) return -1;
    return s_voiceMuted[slot] ? 1 : 0;
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
// Server-info ops (reservedslots+basetriggers) — typed vtable calls on the game-server pointer.
// Reuse the client-list slice's typed S2_GameServer() (INetworkServerService::GetIGameServer());
// the compiler derives the GetMaxClients / GetMapName / GetGlobals()->curtime vtable indices from
// iserver.h — no manual index math. Degrade-never-crash: null → 0 / "" / 0.
// ---------------------------------------------------------------------------
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

// Crash-reporter slice: the engine build number (IVEngineServer2::GetBuildVersion — a typed SDK
// virtual on the already-acquired s_pEngine; engine-generic). 0 = interface unavailable (degrade).
static int s2_server_build_number(void) {
    return s_pEngine ? s_pEngine->GetBuildVersion() : 0;
}

// Crash-harness op (spec §10): a REAL fault in shim code, so the live gate exercises the exact
// Breakpad path a production crash takes. Only reachable through the dev_test-gated core native.
static void s2_crash_test_native(int kind) {
    if (kind == 1) abort();
    volatile int* p = nullptr;
    *p = 42; // SIGSEGV
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
// player_change_team (changeteam slice) — move a player's CONTROLLER between teams via
// CCSPlayerController::ChangeTeam(int team), sig-resolved on OUR libserver.so (s_pChangeTeam, loaded in
// Load). ChangeTeam (the poor-sharptimer/CSSharp `!spec` path) moves the player IMMEDIATELY — unlike
// SwitchTeam, which the live gate proved queues a deferred switch (no move). The signature self-resolves
// the real function (CSSharp's vtable OFFSET 101 is a `ret` stub here; ChangeTeam is slot 102 — the
// CommitSuicide-index drift), so it is NOT a borrowed index. GUARDED identically to pawn_commit_suicide:
// the controller is reconstructed from (idx, serial) + serial-gated (s2_deref_handle → null if stale), and
// the resolved fn ptr must point into libserver's .text (reuses s_serverText/s_serverTextSize) — a
// null/out-of-range ptr or a stale ref degrades to a logged no-op, never a crash. `team` is bounded to
// 0..3 (Unassigned/Spec/T/CT). ABI: void CCSPlayerController::ChangeTeam(this /*rdi*/, int team /*esi*/).
// ---------------------------------------------------------------------------
typedef void (*ChangeTeam_t)(void* thisptr, int team);
static ChangeTeam_t s_pChangeTeam = nullptr;             // sig-resolved fn ptr (loaded in Load)
static void s2_player_change_team(int idx, int serial, int team) {
    if (!s_pChangeTeam) return;                          // signature unresolved -> no-op
    if (team < 0 || team > 3) return;                    // Unassigned/Spectator/T/CT only
    CEntityHandle h(idx, serial);
    void* controller = s2_deref_handle(static_cast<unsigned int>(h.ToInt()));  // null if stale/free slot
    if (!controller) return;
    const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pChangeTeam);
    if (!s_serverText || f < s_serverText || f >= s_serverText + s_serverTextSize) {
        META_CONPRINTF("[s2script] ChangeTeam fn %p out of libserver .text — no-op\n", (const void*)f);
        return;
    }
    s_pChangeTeam(controller, team);
}

// ---------------------------------------------------------------------------
// player_switch_team (switchteam slice) — NON-LETHAL controller team move via
// CCSPlayerController::SwitchTeam(this, team): the player stays alive and keeps weapons (vs ChangeTeam
// = jointeam semantics); the pawn MAY be respawned (consumers re-resolve player.pawn next frame). For
// team <= 1 (None/Spectator) dispatches to s2_player_change_team — CSSharp/SwiftlyS2 parity: the
// engine SwitchTeam is CS:GO-lineage T/CT-only. Guarded identically to change_team: serial-gate +
// 0..3 bounds + .text-range check; any failure degrades to a (logged) no-op, never a crash.
// HISTORY: an earlier borrowed "SwitchTeam" sig hit the WRONG function on our build (the deferred
// m_bSwitchTeamsOnNextRoundReset halftime swap — live-gate-proven no-move); this sig is the real
// per-player function, validated UNIQUE @0x1525f40 on 2000875 and re-validated every boot.
// ABI: void CCSPlayerController::SwitchTeam(this /*rdi*/, unsigned int team /*esi*/).
// ---------------------------------------------------------------------------
typedef void (*SwitchTeam_t)(void* thisptr, int team);
static SwitchTeam_t s_pSwitchTeam = nullptr;             // sig-resolved fn ptr (loaded in Load)
static void s2_player_switch_team(int idx, int serial, int team) {
    if (team < 0 || team > 3) return;                    // Unassigned/Spectator/T/CT only
    if (team <= 1) {                                     // None/Spectator -> ChangeTeam (parity path)
        s2_player_change_team(idx, serial, team);
        return;
    }
    if (!s_pSwitchTeam) return;                          // signature unresolved -> no-op
    CEntityHandle h(idx, serial);
    void* controller = s2_deref_handle(static_cast<unsigned int>(h.ToInt()));  // null if stale/free slot
    if (!controller) return;
    const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pSwitchTeam);
    if (!s_serverText || f < s_serverText || f >= s_serverText + s_serverTextSize) {
        META_CONPRINTF("[s2script] SwitchTeam fn %p out of libserver .text — no-op\n", (const void*)f);
        return;
    }
    s_pSwitchTeam(controller, team);
}

// ---------------------------------------------------------------------------
// player_respawn (player-respawn slice) — re-activate a (dead) player via the sig-resolved
// CCSPlayerController::Respawn(this) (s_pRespawn, loaded in Load behind TWO gates: unique-match AND
// the Respawn.vtable-member RTTI check — CSSharp ships a BARE vtable index here, the sm_slay/ChangeTeam
// borrowed-index failure class, so the shipped sig must prove it landed on a genuine CCSPlayerController
// virtual). DEFERRED EXECUTION: Respawn fires player_spawn SYNCHRONOUSLY; called inline from a JS
// native (inside the core's isolate borrow) the re-entry would be try_borrow-skipped and EVERY plugin
// would silently miss the event. So the op only enqueues into a deduped MULTI-ENTRY pending set
// (TTT's round-start loops respawn many players in one dispatch — unlike terminate-round, latest-wins
// would be a correctness bug) and Hook_GameFrameRespawnDrain (installed eagerly at Load iff both gates
// passed) executes OUTSIDE the JS borrow. (idx, serial) = the CONTROLLER entity; alive_off = the
// "pawn is alive" bool offset from the game package (re-checked at drain to close the enqueue->drain
// TOCTOU; < 0 skips the re-check). Serial-gated at BOTH enqueue and drain; .text-guarded like
// ChangeTeam. NOTE Plan A (spec §2.3): Respawn ALONE, no SetPawn pre-call — CSSharp's SetPawn sig is
// STALE on 2000875 (0 hits); if the live gate shows a dead player is not re-activated, Plan B is a
// pawn.js schema pre-write (m_hPawn <- m_hPlayerPawn), zero shim changes.
// ---------------------------------------------------------------------------
typedef void (*Respawn_t)(void* controller);
static Respawn_t s_pRespawn = nullptr;                   // sig-resolved fn ptr (loaded in Load, dual-gated)
// CBasePlayerController::SetPawn(pawn, b1, b2) — called (playerPawn, true, false) BEFORE Respawn to
// re-activate a dead player's pawn (observer teardown + m_hPawn repoint + dirty flags). A 4-ARG function
// (void*,void*,bool,bool) — verbatim what SwiftlyS2 (player.cpp:345, gamedata sig BYTE-IDENTICAL to ours)
// and CSSharp both declare + call; passing extra args feeds the function a different reset flag. NON-VIRTUAL
// on 2000875 (unique sig + .text guard; no vtable-member gate). SysV: rdi=controller, rsi=pawn, edx=b1, ecx=b2.
typedef void (*SetPawn_t)(void* controller, void* pawn, int b1, int b2);
static SetPawn_t s_pSetPawn = nullptr;                   // sig-resolved fn ptr (loaded in Load)
struct PendingRespawn { uint32_t handle; int aliveOff; int hplayerpawnOff; };
static const int kRespawnPendingMax = 130;               // > 64 slots * controller+margin; engine-generic cap
static PendingRespawn s_pendingRespawn[kRespawnPendingMax];
static int s_pendingRespawnCount = 0;
static bool s_respawnDrainHooked = false;                // Load-installed, Unload-removed

static int s2_player_respawn(int idx, int serial, int alive_off, int hplayerpawn_off) {
    if (!s_pRespawn || !s_pSetPawn) return 0;            // respawn needs BOTH engine facts resolved -> degrade
    if (!s_respawnDrainHooked) return 0;                 // no drain hook installed -> nothing would drain the queue
    CEntityHandle h(idx, serial);
    if (!s2_deref_handle(static_cast<unsigned int>(h.ToInt()))) return 0;  // stale NOW; re-gated at drain
    uint32_t hv = static_cast<uint32_t>(h.ToInt());
    for (int i = 0; i < s_pendingRespawnCount; i++)
        if (s_pendingRespawn[i].handle == hv) return 1;  // dedupe: double-respawn-same-frame is idempotent
    if (s_pendingRespawnCount >= kRespawnPendingMax) {
        META_CONPRINTF("[s2script] player_respawn: pending set full (%d) — rejected\n", kRespawnPendingMax);
        return 0;
    }
    // ENQUEUE — the SetPawn+Respawn engine sequence runs at the next GameFrame drain, OUTSIDE the JS isolate
    // borrow, so the resulting player_spawn reaches every plugin's handlers (round-control §4.1 precedent).
    s_pendingRespawn[s_pendingRespawnCount++] = { hv, alive_off, hplayerpawn_off };
    return 1;
}

// ---------------------------------------------------------------------------
// gamerules_terminate_round (round-control slice) — force the round to end via the sig-resolved
// CCSGameRules::TerminateRound(float delay, uint32 reason, void* unk3=0, uint32 unk4=0) (s_pTerminateRound,
// loaded in Load behind TWO gates: unique-match AND the scope-string semantic check — the borrowed
// CSSharp/Swiftly sig is unique-but-WRONG on 2000875). DEFERRED EXECUTION: TerminateRound fires the
// round-end event machinery SYNCHRONOUSLY; called inline from a JS native (inside the core's isolate
// borrow) the round_end re-entry would be try_borrow-skipped and EVERY plugin would silently miss the
// event. So the op only arms a single-slot pending request (latest-wins — a round ends once) and
// Hook_GameFrameRoundDrain (installed eagerly at Load iff the sig resolved; one branch/frame) executes
// it OUTSIDE the JS borrow. (idx, serial) identify the rules PROXY entity and rules_ptr_off the offset
// of its rules-struct pointer field — both come from the game package; no game names live here. The
// proxy is serial-gated at BOTH enqueue (fast feedback) and drain (it can die in between); the fn ptr
// is .text-range-guarded like ChangeTeam. reason is host-bounded 0..22 (mirrors the engine's own
// `cmp $0x16` check; in-range legacy holes 2/3/15 pass through — the engine's switch handles them).
// ---------------------------------------------------------------------------
typedef void (*TerminateRound_t)(void* rules, float delay, uint32_t reason, void* unk3, uint32_t unk4);
static TerminateRound_t s_pTerminateRound = nullptr;     // sig-resolved fn ptr (loaded in Load, dual-gated)
struct PendingTerminate { bool armed; uint32_t proxyHandle; int rulesPtrOff; float delay; int reason; };
static PendingTerminate s_pendingTerminate = { false, 0, 0, 0.0f, 0 };
static bool s_termDrainHooked = false;                   // Load-installed, Unload-removed

static int s2_gamerules_terminate_round(int idx, int serial, int rules_ptr_off, float delay, int reason) {
    if (!s_pTerminateRound) return 0;                    // signature unresolved/failed-semantic -> degrade
    if (reason < 0 || reason > 22) {
        META_CONPRINTF("[s2script] terminate_round: reason %d out of range 0..22 — rejected\n", reason);
        return 0;
    }
    if (rules_ptr_off < 0) return 0;
    CEntityHandle h(idx, serial);
    if (!s2_deref_handle(static_cast<unsigned int>(h.ToInt()))) return 0;  // stale proxy NOW; re-gated at drain
    if (s_pendingTerminate.armed)
        META_CONPRINTF("[s2script] terminate_round: overwriting a pending request (latest wins)\n");
    s_pendingTerminate = { true, static_cast<uint32_t>(h.ToInt()), rules_ptr_off, delay, reason };
    return 1;
}

// ---------------------------------------------------------------------------
// Usercmd primitive (per-tick input read/modify/block; SM OnPlayerRunCmd parity) — detours
// CCSPlayer_MovementServices::ProcessUsercmds (self-resolved sig "ProcessUsercmds"; batch ABI + return
// type + CUserCmd stride confirmed by an offline disassembly spike, 2026-07-14 — see
// docs/superpowers/plans/2026-07-14-usercmd-primitive.md). Each CUserCmd (stride S2_USERCMD_STRIDE)
// wraps a CSGOUserCmdPB protobuf at +0x10; ALL read/modify happens by protobuf reflection over the
// shim-side s_currentUserCmd (block-scoped: valid ONLY during a usercmd dispatch, mirrors
// s_currentDamageInfo/s_currentEvent — the raw Message* NEVER crosses to JS). ENGINE-GENERIC numeric
// field enum (0 fwd,1 side,2 up,3 pitch,4 yaw,5 roll,6 impulse) maps to the Source2-shared
// usercmd.proto CBaseUserCmdPB/CMsgQAngle/CInButtonStatePB nesting HERE (shim-only — core never sees a
// protobuf field name or a CS2 type name). LAZILY installed: the signature is resolved at Load into
// s_pProcessUsercmdsAddr (NOT yet detoured); Shim_UsercmdHookInstall (the usercmd_hook_install op)
// performs s2detour::Install idempotently on the FIRST-EVER UserCmd.onRun subscribe (core calls it —
// see s2_usercmd_subscribe), so there is zero overhead until a plugin actually wants per-tick input.
// s2detour::RemoveAll() (already called in Unload()) restores this detour's prologue along with every
// other installed one — no usercmd-specific teardown code needed.
// ---------------------------------------------------------------------------
typedef int (*ProcessUsercmds_t)(void* thisptr, void* cmds, int numcmds, bool paused, float margin);
static void*             s_pProcessUsercmdsAddr    = nullptr;   // sig-resolved address (Load) — NOT yet installed
static ProcessUsercmds_t g_origProcessUsercmds     = nullptr;   // the trampoline, set once s2detour::Install succeeds
static bool              s_usercmdHookInstalled    = false;
static google::protobuf::Message* s_currentUserCmd = nullptr;   // the in-flight CSGOUserCmdPB; block-scoped

static constexpr int S2_USERCMD_STRIDE = 0x90;   // sizeof(CUserCmd); spike-confirmed (Task 1, 2026-07-14)
// SUBTICK VERDICT (live human spike, 2026-07-14): a coarse forwardmove=0 write ALONE (no subtick clear)
// stopped the player — subtick_moves do NOT override the coarse fields. So neutralizing a BLOCKED cmd
// (zeroing forwardMove/sideMove/upMove/buttons) is already a full stop without an extra subtick clear;
// kept as a named, disableable constant (rather than deleting the call site outright) in case a future
// finding narrows a case where it IS needed. The write ops themselves (s2_usercmd_write) NEVER
// auto-clear subtick regardless of this flag — clearSubtickMoves() stays an explicit opt-in helper.
static constexpr bool S2_SUBTICK_CLEAR_ON_BLOCK = false;

// Cached FieldDescriptor*s for CSGOUserCmdPB's "base" (CBaseUserCmdPB) submessage + its nested fields.
// Resolved ONCE from the first live s_currentUserCmd's descriptor (a function-local static — a C++
// "magic static", thread-safe single-init; the game drives this detour from one thread). protobuf
// FieldDescriptor*s are stable for the process lifetime (the descriptor POOL is a compiled-in
// singleton keyed by type), so caching is always safe: a null entry here (Valve renamed/removed the
// field) stays a permanent, harmless no-op/0 (never re-resolved, never UB). This is a
// per-tick-PER-PLAYER hot path (unlike the rare usermessage path), so caching matters.
struct UsercmdFieldCache {
    const google::protobuf::FieldDescriptor* baseF         = nullptr;
    const google::protobuf::FieldDescriptor* fwdF          = nullptr;
    const google::protobuf::FieldDescriptor* leftF         = nullptr;   // raw field; NEGATED at the read/write boundary (MF-2)
    const google::protobuf::FieldDescriptor* upF           = nullptr;
    const google::protobuf::FieldDescriptor* impulseF      = nullptr;
    const google::protobuf::FieldDescriptor* viewAnglesF   = nullptr;
    const google::protobuf::FieldDescriptor* vaXF          = nullptr;
    const google::protobuf::FieldDescriptor* vaYF          = nullptr;
    const google::protobuf::FieldDescriptor* vaZF          = nullptr;
    const google::protobuf::FieldDescriptor* buttonsPbF    = nullptr;
    const google::protobuf::FieldDescriptor* buttonState1F = nullptr;
    const google::protobuf::FieldDescriptor* subtickMovesF = nullptr;
};

static const UsercmdFieldCache& GetUsercmdFieldCache() {
    static UsercmdFieldCache s_cache;
    static bool s_inited = false;
    if (s_inited || !s_currentUserCmd) return s_cache;   // no live cmd yet -> stay uninited, retry later
    const auto* d = s_currentUserCmd->GetDescriptor();
    if (!d) return s_cache;                              // stay uninited -> retry on a later call
    using FD = google::protobuf::FieldDescriptor;
    s_cache.baseF = d->FindFieldByName("base");
    if (s_cache.baseF && s_cache.baseF->cpp_type() == FD::CPPTYPE_MESSAGE) {
        const auto* baseD = s_cache.baseF->message_type();
        if (baseD) {
            s_cache.fwdF          = baseD->FindFieldByName("forwardmove");
            s_cache.leftF         = baseD->FindFieldByName("leftmove");
            s_cache.upF           = baseD->FindFieldByName("upmove");
            s_cache.impulseF      = baseD->FindFieldByName("impulse");
            s_cache.viewAnglesF   = baseD->FindFieldByName("viewangles");
            s_cache.buttonsPbF    = baseD->FindFieldByName("buttons_pb");
            s_cache.subtickMovesF = baseD->FindFieldByName("subtick_moves");
            if (s_cache.viewAnglesF && s_cache.viewAnglesF->cpp_type() == FD::CPPTYPE_MESSAGE) {
                const auto* vaD = s_cache.viewAnglesF->message_type();
                if (vaD) {
                    s_cache.vaXF = vaD->FindFieldByName("x");
                    s_cache.vaYF = vaD->FindFieldByName("y");
                    s_cache.vaZF = vaD->FindFieldByName("z");
                }
            }
            if (s_cache.buttonsPbF && s_cache.buttonsPbF->cpp_type() == FD::CPPTYPE_MESSAGE) {
                const auto* bD = s_cache.buttonsPbF->message_type();
                if (bD) s_cache.buttonState1F = bD->FindFieldByName("buttonstate1");
            }
        }
    }
    s_inited = true;
    return s_cache;
}

// s2_usercmd_read(field) -> double. field: 0 forwardMove,1 sideMove(=-leftmove, MF-2),2 upMove,
// 3 pitch,4 yaw,5 roll,6 impulse. GetMessage() ONLY (a read must never allocate / set has-bits).
// Every FieldDescriptor* is null-guarded + cpp_type()-validated before use. 0.0 on any guard failure.
static double s2_usercmd_read(int field) {
    if (!s_currentUserCmd) return 0.0;
    const auto& c = GetUsercmdFieldCache();
    if (!c.baseF) return 0.0;
    const auto* r = s_currentUserCmd->GetReflection();
    if (!r) return 0.0;
    const auto& base = r->GetMessage(*s_currentUserCmd, c.baseF);
    const auto* br = base.GetReflection();
    if (!br) return 0.0;
    using FD = google::protobuf::FieldDescriptor;
    switch (field) {
        case 0:   // forwardMove
            if (!c.fwdF || c.fwdF->cpp_type() != FD::CPPTYPE_FLOAT) return 0.0;
            return static_cast<double>(br->GetFloat(base, c.fwdF));
        case 1:   // sideMove = -leftmove (MF-2: leftmove is +LEFT; sideMove is +RIGHT)
            if (!c.leftF || c.leftF->cpp_type() != FD::CPPTYPE_FLOAT) return 0.0;
            return -static_cast<double>(br->GetFloat(base, c.leftF));
        case 2:   // upMove
            if (!c.upF || c.upF->cpp_type() != FD::CPPTYPE_FLOAT) return 0.0;
            return static_cast<double>(br->GetFloat(base, c.upF));
        case 3: case 4: case 5: {   // pitch/yaw/roll via viewangles (CMsgQAngle)
            if (!c.viewAnglesF) return 0.0;
            const auto& va = br->GetMessage(base, c.viewAnglesF);
            const auto* var = va.GetReflection();
            if (!var) return 0.0;
            const auto* f = (field == 3) ? c.vaXF : (field == 4) ? c.vaYF : c.vaZF;
            if (!f || f->cpp_type() != FD::CPPTYPE_FLOAT) return 0.0;
            return static_cast<double>(var->GetFloat(va, f));
        }
        case 6:   // impulse
            if (!c.impulseF || c.impulseF->cpp_type() != FD::CPPTYPE_INT32) return 0.0;
            return static_cast<double>(br->GetInt32(base, c.impulseF));
        default:
            return 0.0;   // out-of-range field -> no-op (the native is plugin-reachable with any int)
    }
}

// s2_usercmd_write(field, value) — same navigation via MutableMessage() (writes only). Every Set* is
// is_repeated()/cpp_type()-guarded (an is_repeated scalar Set* is a protobuf GOOGLE_LOG(FATAL) process
// abort). field 1 writes leftmove = -value (MF-2). NO auto-subtick-clear — the spike verdict found a
// coarse write alone takes effect; callers wanting a clear call usercmd_clear_subtick explicitly.
static void s2_usercmd_write(int field, double value) {
    if (!s_currentUserCmd) return;
    const auto& c = GetUsercmdFieldCache();
    if (!c.baseF) return;
    const auto* r = s_currentUserCmd->GetReflection();
    if (!r) return;
    auto* base = r->MutableMessage(s_currentUserCmd, c.baseF);
    if (!base) return;
    const auto* br = base->GetReflection();
    if (!br) return;
    using FD = google::protobuf::FieldDescriptor;
    switch (field) {
        case 0:
            if (!c.fwdF || c.fwdF->is_repeated() || c.fwdF->cpp_type() != FD::CPPTYPE_FLOAT) return;
            br->SetFloat(base, c.fwdF, static_cast<float>(value));
            return;
        case 1:
            if (!c.leftF || c.leftF->is_repeated() || c.leftF->cpp_type() != FD::CPPTYPE_FLOAT) return;
            br->SetFloat(base, c.leftF, static_cast<float>(-value));   // MF-2
            return;
        case 2:
            if (!c.upF || c.upF->is_repeated() || c.upF->cpp_type() != FD::CPPTYPE_FLOAT) return;
            br->SetFloat(base, c.upF, static_cast<float>(value));
            return;
        case 3: case 4: case 5: {
            if (!c.viewAnglesF) return;
            auto* va = br->MutableMessage(base, c.viewAnglesF);
            if (!va) return;
            const auto* var = va->GetReflection();
            if (!var) return;
            const auto* f = (field == 3) ? c.vaXF : (field == 4) ? c.vaYF : c.vaZF;
            if (!f || f->is_repeated() || f->cpp_type() != FD::CPPTYPE_FLOAT) return;
            var->SetFloat(va, f, static_cast<float>(value));
            return;
        }
        case 6:
            if (!c.impulseF || c.impulseF->is_repeated() || c.impulseF->cpp_type() != FD::CPPTYPE_INT32) return;
            br->SetInt32(base, c.impulseF, static_cast<int32_t>(value));
            return;
        default:
            return;
    }
}

// s2_usercmd_read_buttons() -> the current usercmd's pressed-button mask (base.buttons_pb.buttonstate1,
// a uint64). GetMessage() ONLY. 0 on any guard failure.
static uint64_t s2_usercmd_read_buttons() {
    if (!s_currentUserCmd) return 0;
    const auto& c = GetUsercmdFieldCache();
    if (!c.baseF || !c.buttonsPbF || !c.buttonState1F) return 0;
    if (c.buttonState1F->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_UINT64) return 0;
    const auto* r = s_currentUserCmd->GetReflection();
    if (!r) return 0;
    const auto& base = r->GetMessage(*s_currentUserCmd, c.baseF);
    const auto* br = base.GetReflection();
    if (!br) return 0;
    const auto& btn = br->GetMessage(base, c.buttonsPbF);
    const auto* btnR = btn.GetReflection();
    if (!btnR) return 0;
    return btnR->GetUInt64(btn, c.buttonState1F);
}

// s2_usercmd_write_buttons(mask) — overwrite base.buttons_pb.buttonstate1. is_repeated()/cpp_type()
// guarded. No-op on any guard failure.
static void s2_usercmd_write_buttons(uint64_t mask) {
    if (!s_currentUserCmd) return;
    const auto& c = GetUsercmdFieldCache();
    if (!c.baseF || !c.buttonsPbF || !c.buttonState1F) return;
    if (c.buttonState1F->is_repeated() ||
        c.buttonState1F->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_UINT64) return;
    const auto* r = s_currentUserCmd->GetReflection();
    if (!r) return;
    auto* base = r->MutableMessage(s_currentUserCmd, c.baseF);
    if (!base) return;
    const auto* br = base->GetReflection();
    if (!br) return;
    auto* btn = br->MutableMessage(base, c.buttonsPbF);
    if (!btn) return;
    const auto* btnR = btn->GetReflection();
    if (!btnR) return;
    btnR->SetUInt64(btn, c.buttonState1F, mask);
}

// s2_usercmd_clear_subtick() — drop base.subtick_moves (an OPTIONAL helper; see S2_SUBTICK_CLEAR_ON_BLOCK
// above — the write ops never call this automatically).
static void s2_usercmd_clear_subtick() {
    if (!s_currentUserCmd) return;
    const auto& c = GetUsercmdFieldCache();
    if (!c.baseF || !c.subtickMovesF) return;
    const auto* r = s_currentUserCmd->GetReflection();
    if (!r) return;
    auto* base = r->MutableMessage(s_currentUserCmd, c.baseF);
    if (!base) return;
    const auto* br = base->GetReflection();
    if (!br) return;
    br->ClearField(base, c.subtickMovesF);
}

// Derive the firing player's SLOT from the detour's `this`. `this+0x7c0` holds the firing PAWN's
// entity handle (spike-derived from disassembly of the resolved ProcessUsercmds on OUR libserver.so —
// docs/superpowers/plans/2026-07-14-usercmd-primitive.md, Step 1b). LIVE-VERIFIED against a human-joined
// server (Task 5, 2026-07-14): the logged slot matches the tester's known slot and a schema
// Pawn.forSlot(slot) read of the SAME player. The recipe is documented inline below.
static int DeriveUsercmdSlot(void* thisptr) {
    if (!thisptr) return -1;
    // this+0x7c0 holds the firing PAWN's entity handle (the game decodes it via `shr $9 & 0x3f` [chunk]
    // + `& 0x1ff` [slot-in-chunk] to look the pawn up in the entity system). The LIVE GATE (2026-07-14,
    // Task 5) proved `(idx>>9)&0x3f` is the entity-system CHUNK, NOT the player slot (a slot-0 human read
    // slot=1). Correct path: resolve the pawn -> read m_hController -> the CONTROLLER's entity index - 1
    // IS the 0-based player slot (controllers are entities 1..64). Degrades to -1 on any miss.
    uint16_t raw = *reinterpret_cast<const uint16_t*>(reinterpret_cast<const char*>(thisptr) + 0x7c0);
    void* pawn = s2_ent_by_index(static_cast<int>(raw & 0x7fff));   // low 15 bits = the pawn's entity index
    if (!pawn) return -1;
    int off = s2_schema_offset("CBasePlayerPawn", "m_hController");
    if (off < 0) return -1;
    uint32 hController = *reinterpret_cast<const uint32*>(reinterpret_cast<const char*>(pawn) + off);
    int ci = CEntityHandle(hController).GetEntryIndex();            // controller entity index, or -1
    int slot = ci - 1;
    return (slot >= 0 && slot < 64) ? slot : -1;
}

// The production ProcessUsercmds detour. For each in-flight CUserCmd, points s_currentUserCmd at its
// embedded CSGOUserCmdPB (block-scoped — cleared right after dispatch) and runs the JS UserCmd.onRun
// subscribers via the core mux. A collapsed HookResult >= Handled (2) NEUTRALIZES that one cmd (zeroes
// movement + buttons) — server-authoritative: the original trampoline is ALWAYS still called with the
// (possibly modified/neutralized) cmds, never skipped, since a usercmd is data the engine must still
// process (unlike a suppressed event/output, there is no "the original call" to skip here).
static int Detour_ProcessUsercmds(void* thisptr, void* cmds, int numcmds, bool paused, float margin) {
    int slot = DeriveUsercmdSlot(thisptr);
    if (cmds && numcmds > 0) {
        for (int i = 0; i < numcmds; i++) {
            s_currentUserCmd = reinterpret_cast<google::protobuf::Message*>(
                reinterpret_cast<char*>(cmds) + static_cast<size_t>(i) * S2_USERCMD_STRIDE + 0x10);
            int res = s2script_core_dispatch_usercmd(slot);   // JS reads/modifies s_currentUserCmd in place
            if (res >= 2) {   // Handled|Stop -> neutralize this cmd
                s2_usercmd_write(0, 0.0);
                s2_usercmd_write(1, 0.0);
                s2_usercmd_write(2, 0.0);
                s2_usercmd_write_buttons(0);
                if (S2_SUBTICK_CLEAR_ON_BLOCK) s2_usercmd_clear_subtick();
            }
            s_currentUserCmd = nullptr;
        }
    }
    return g_origProcessUsercmds ? g_origProcessUsercmds(thisptr, cmds, numcmds, paused, margin) : 0;
}

// usercmd_hook_install: called by core on the FIRST-EVER UserCmd.onRun subscribe (lazy — zero overhead
// until a plugin actually wants per-tick input). Idempotent (a 2nd+ call is a no-op success once
// installed). Returns 1 iff the detour is (now, or already) installed, else 0 (unresolved signature on
// this build -> UserCmd.onRun degrades to a silent no-op, never a crash).
static int Shim_UsercmdHookInstall() {
    if (s_usercmdHookInstalled) return 1;
    if (!s_pProcessUsercmdsAddr) return 0;   // ProcessUsercmds signature unresolved on this build
    if (s2detour::Install(s_pProcessUsercmdsAddr, reinterpret_cast<void*>(&Detour_ProcessUsercmds),
                          reinterpret_cast<void**>(&g_origProcessUsercmds))) {
        s_usercmdHookInstalled = true;
        META_CONPRINTF("[s2script] ProcessUsercmds hooked @%p (UserCmd.onRun, lazy-installed)\n", s_pProcessUsercmdsAddr);
        return 1;
    }
    META_CONPRINTF("[s2script] WARN: ProcessUsercmds detour install failed — UserCmd.onRun off\n");
    return 0;
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

// CrashSpoolDir: addons/s2script/data/crashes, resolved relative to the plugin .so via dladdr
// (mirrors PluginsDir). Created (mkdir -p equivalent, two levels) if absent; "" on any failure
// (fail-off — crash reporting then stays disarmed).
static std::string CrashSpoolDir() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&CrashSpoolDir), &info) && info.dli_fname) {
        std::string dir(info.dli_fname);
        // The .so lives at addons/s2script/bin/linuxsteamrt64/s2script.so — 3 dirname steps reach
        // the addon root (mirrors s2_db_data_dir()/PluginsDir()). ONE step lands the spool under
        // bin/ (a :ro mount in docker-compose) → mkdir fails → silent fail-off. Must be 3.
        for (int i = 0; i < 3; i++) dir = dir.substr(0, dir.find_last_of('/'));
        std::string data = dir + "/data";
        std::string spool = data + "/crashes";
        mkdir(data.c_str(), 0755);            // EEXIST is fine
        if (mkdir(spool.c_str(), 0755) == 0 || errno == EEXIST) return spool;
    }
    return "";
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

// ConfigFilePath: like ConfigPath but the name INCLUDES its extension (no .json append). Reuses the same
// sanitize (non-[A-Za-z0-9._-] -> '_', which neutralizes '/'); additionally refuses names containing ".."
// or empty (returns "" -> read/write fail) so there is no traversal.
static std::string ConfigFilePath(const char* name) {
    if (!name || !*name) return "";
    if (std::string(name).find("..") != std::string::npos) return "";
    std::string safe;
    for (const char* p = name; *p; ++p) {
        char c = *p;
        safe += ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
                 || c == '.' || c == '_' || c == '-') ? c : '_';
    }
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&ConfigFilePath), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf); snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);             snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);
        return dir + "/configs/" + safe;
    }
    return "addons/s2script/configs/" + safe;
}
static std::string s_configFileReadBuf;
static const char* s2_config_read_file(const char* name) {
    std::string path = ConfigFilePath(name);
    if (path.empty()) return nullptr;
    std::ifstream f(path); if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf(); s_configFileReadBuf = ss.str();
    return s_configFileReadBuf.c_str();
}
static int s2_config_write_file(const char* name, const char* content) {
    std::string path = ConfigFilePath(name); if (path.empty() || !content) return 0;
    std::error_code ec; std::filesystem::create_directories(std::filesystem::path(path).parent_path(), ec);
    std::ofstream f(path); if (!f) return 0; f << content; return f.good() ? 1 : 0;
}

// ---------------------------------------------------------------------------
// Translations slice: read addons/s2script/translations/[<lang>/]<name>.phrases.json.
// TranslationsPath: mirror ConfigFilePath's walk + sanitize (non-[A-Za-z0-9._-] -> '_',
// neutralizing '/'); refuses a segment containing ".." or empty name.
// ---------------------------------------------------------------------------
static std::string TranslationsPath(const char* lang, const char* name) {
    if (!name || !*name) return "";
    auto bad = [](const char* s) { return !s ? false : std::string(s).find("..") != std::string::npos; };
    if (bad(lang) || bad(name)) return "";
    auto sani = [](const char* p) { std::string o; for (; p && *p; ++p) { char c = *p;
        o += ((c>='A'&&c<='Z')||(c>='a'&&c<='z')||(c>='0'&&c<='9')||c=='.'||c=='_'||c=='-') ? c : '_'; } return o; };
    std::string safeLang = lang ? sani(lang) : "";
    std::string safeName = sani(name);
    Dl_info info;
    std::string root;
    if (dladdr(reinterpret_cast<void*>(&TranslationsPath), &info) && info.dli_fname) {
        char buf[4096]; snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf); snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);             snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);
        root = dir + "/translations/";
    } else {
        root = "addons/s2script/translations/";
    }
    if (!safeLang.empty()) root += safeLang + "/";
    return root + safeName + ".phrases.json";
}
static std::string s_translationsReadBuf;
static const char* s2_translations_read(const char* lang, const char* name) {
    std::string path = TranslationsPath(lang, name);
    if (path.empty()) return nullptr;
    std::ifstream f(path); if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf(); s_translationsReadBuf = ss.str();
    return s_translationsReadBuf.c_str();
}
static const char* s2_client_language(int slot) {
    if (!s_pEngine || !s2_client_valid(slot)) return nullptr;
    return s_pEngine->GetClientConVarValue(CPlayerSlot(slot), "cl_language");
}

// ---------------------------------------------------------------------------
// db_data_dir (Slice DB): absolute path to addons/s2script/data, created if absent. Resolved
// relative to the plugin .so via dladdr (mirrors ConfigPath's dirname ×3 walk to the addon root),
// sibling of the configs/ dir.
// ---------------------------------------------------------------------------
static std::string s_dbDataDirBuf;
static const char* s2_db_data_dir(void) {
    Dl_info info;
    std::string dir;
    if (dladdr(reinterpret_cast<void*>(&s2_db_data_dir), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string d = dirname(buf);               // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", d.c_str());
        d = dirname(buf);                            // bin
        snprintf(buf, sizeof buf, "%s", d.c_str());
        d = dirname(buf);                            // s2script addon root
        dir = d + "/data";
    } else {
        // Fallback: relative to the server's cwd.
        dir = "addons/s2script/data";
    }
    std::error_code ec; std::filesystem::create_directories(dir, ec);
    s_dbDataDirBuf = dir;
    return s_dbDataDirBuf.c_str();
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

// Full mapped [lo, hi) LOAD extent of the SAME module FindModuleText selects (largest-PF_X-wins,
// Metamod-proxy-safe). Needed because .rodata (where sig-anchoring C-strings live) sits in a LOAD
// segment BELOW the PF_X base — a rip-relative string target is OUTSIDE the .text buffer and must be
// range-guarded against the whole mapping before it is read.
struct ModBounds { const uint8_t* lo; const uint8_t* hi; };
static ModBounds FindModuleBounds(const char* soname) {
    struct Ctx { const char* name; size_t bestX; ModBounds out; } ctx{ soname, 0, { nullptr, nullptr } };
    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto* c = static_cast<Ctx*>(data);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) return 0;
        size_t maxX = 0;
        ElfW(Addr) lo = ~static_cast<ElfW(Addr)>(0), hi = 0;
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type != PT_LOAD) continue;
            if ((ph.p_flags & PF_X) && ph.p_filesz > maxX) maxX = ph.p_filesz;
            if (ph.p_vaddr < lo) lo = ph.p_vaddr;
            if (ph.p_vaddr + ph.p_memsz > hi) hi = ph.p_vaddr + ph.p_memsz;
        }
        if (maxX > c->bestX) {   // same winner rule as FindModuleText: largest PF_X segment
            c->bestX = maxX;
            c->out.lo = reinterpret_cast<const uint8_t*>(info->dlpi_addr + lo);
            c->out.hi = reinterpret_cast<const uint8_t*>(info->dlpi_addr + hi);
        }
        return 0;
    }, &ctx);
    return ctx.out;
}

// Semantic load-gate for the TerminateRound descriptor (uniqueness is NOT enough — the borrowed
// CSSharp/Swiftly sig matches UNIQUELY at the WRONG function on build 2000875). The self-derived
// pattern pins the `48 8D 35` (lea rsi,[rip+disp32]) opcode at fn+0xb and masks only the disp;
// this follows the disp and verifies the target is the literal scope string "TerminateRound".
static bool ValidateTerminateRoundScopeString(const ModText& mt, int64_t fnOff, const char* module) {
    int64_t tgt = s2sig::ResolveLeaDisp(mt.text, mt.size, fnOff + 0xb, /*dispOff=*/3, /*instrLen=*/7);
    if (tgt == s2sig::kFail) return false;
    const uint8_t* p = mt.text + tgt;   // typically BELOW mt.text (.rodata precedes .text in the map)
    ModBounds mb = FindModuleBounds(module);
    static const char kScope[] = "TerminateRound";   // compare INCLUDING the NUL
    if (!mb.lo || p < mb.lo || p + sizeof(kScope) > mb.hi) return false;
    return std::memcmp(p, kScope, sizeof(kScope)) == 0;
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

// Semantic load-gate for the Respawn descriptor (uniqueness is NOT enough — the round-control slice
// proved a sig can match exactly once at the WRONG function, and Respawn has no unique log string to
// xref). Runtime-resolves the CCSPlayerController PRIMARY vtable via RTTI (s2vtable::GetVTableByName —
// the trace-slice precedent) and asserts the sig-resolved address is one of its fn slots. The walk
// ends at the first slot value outside libserver .text (the next sub-vtable's offset-to-top header) —
// fail-closed: a truncated walk that misses the fn FAILS the gate, it never passes wrongly. Logs the
// matched slot as a treadmill breadcrumb (CSSharp's offline hint was 274 on 2000875).
static bool ValidateRespawnVtableMember(const uint8_t* fn, const ModText& mt) {
    void** vt = s2vtable::GetVTableByName("libserver.so", "CCSPlayerController");
    if (!vt) return false;
    for (int i = 0; i < 512; i++) {
        const uint8_t* p = reinterpret_cast<const uint8_t*>(vt[i]);
        if (!p || p < mt.text || p >= mt.text + mt.size) break;   // sub-vtable header = end of fn slots
        if (p == fn) {
            META_CONPRINTF("[s2script] Respawn = CCSPlayerController vtable slot %d\n", i);
            return true;
        }
    }
    return false;
}
static void GamedataBanner() {
    META_CONPRINTF("[s2script] === GAMEDATA VALIDATION: %d ok, %d FAILED%s ===\n", s_gdOk, s_gdFail,
                   s_gdFail ? "  (STALE for this CS2 build — regenerate; see docs/re-strategy.md)" : "");
}

// ---------------------------------------------------------------------------
// Entity-creation lifecycle slice (Task 2): UTIL_CreateEntityByName / CBaseEntity::DispatchSpawn /
// UTIL_Remove are self-validated byte signatures (resolved below, in Load()); CBaseEntity::Teleport
// is called through the entity's own vtable at a gamedata-supplied INDEX, .text-validated before
// the first call (never trusted blind — the CommitSuicide-index lesson). Entities are handled here
// exactly as CEntityInstance* everywhere else in this file (id->m_pInstance IS a CEntityInstance*;
// the class hierarchy is linear, so the object's vtable pointer at offset 0 and GetRefEHandle() are
// valid through that type) — this file never needs the (incomplete, forward-declared-only)
// CBaseEntity type. The raw pointer NEVER crosses to JS: entity_create packs it into a
// CEntityHandle.ToInt() int; entity_spawn/teleport/remove take (index, serial) and re-resolve
// through the EXISTING serial-gated chunk walk (s2_deref_handle) on every call.
// ---------------------------------------------------------------------------
using CreateEntityByNameFn = CEntityInstance* (*)(const char* className, int forceEdictIndex);
using DispatchSpawnFn      = void (*)(CEntityInstance* self, void* pEntityKeyValues);
using UtilRemoveFn         = void (*)(CEntityInstance* self);
static CreateEntityByNameFn s_pCreateEntityByName = nullptr;
static DispatchSpawnFn      s_pDispatchSpawn      = nullptr;
static UtilRemoveFn         s_pUtilRemove         = nullptr;
static int                  s_teleportVtblIndex   = -1;   // from gamedata offsets; -1 = unresolved

// Resolve (index, serial) -> CEntityInstance*, serial-gated via the EXISTING chunk-walk resolver
// (s2_deref_handle). Returns null on a stale/reused/out-of-range pair — never a dangling deref.
static CEntityInstance* ResolveEntityBySerial(int index, int serial) {
    if (index < 0 || serial < 0) return nullptr;
    CEntityHandle h(index, serial);
    return static_cast<CEntityInstance*>(s2_deref_handle(static_cast<unsigned int>(h.ToInt())));
}

// transmit_set op: upsert the AND-merged visibility mask for (index, serial). Serial-gated at
// registration — a stale ref never enters the table. Returns 0 when the entity is stale, the
// table is at cap, the hook isn't installed, or the first-fire layout validation FAILED.
static int s2_transmit_set(int index, int serial, unsigned long long mask) {
    if (!g_S2ScriptPlugin.m_checkTransmitHookInstalled || s_transmitLayoutState < 0) return 0;
    if (index < 0 || serial < 0) return 0;
    if (index >= MAX_EDICTS) return 0;   // not a networkable edict; the bit index would be OOB on m_pTransmitEntity
    if (!ResolveEntityBySerial(index, serial)) return 0;
    auto it = s_transmitTable.find(index);
    if (it == s_transmitTable.end() && s_transmitTable.size() >= kTransmitTableCap) return 0;
    s_transmitTable[index] = TransmitEntry{serial, static_cast<uint64_t>(mask)};
    return 1;
}
static int s2_transmit_clear(int index) {
    return s_transmitTable.erase(index) > 0 ? 1 : 0;
}
static void s2_transmit_stats(unsigned long long* out) {
    if (!out) return;
    out[0] = s_transmitSnapshots;
    out[1] = static_cast<unsigned long long>(s_transmitTable.size());
    out[2] = s_transmitBitsCleared;
    out[3] = s_transmitNsLast;
    out[4] = s_transmitNsMax;
}

// Validate a resolved (vtable-slot / signature) fn pointer lands inside libserver.so's own
// executable range — Rule 2 parity with ResolveSigValidated / the TraceShape vtable-index check.
// A borrowed/stale index could point anywhere; this stops a wrong-but-in-range call before it
// happens rather than crashing on first use.
static bool IsAddressInServerText(void* fn) {
    if (!fn) return false;
    // libserver.so's .text range is fixed after load; cache it on first use so the per-frame
    // entity_teleport hot path (a beam.update per held-E player each frame) does NOT re-walk every
    // loaded module via dl_iterate_phdr on every call. Function-local statics keep this decoupled
    // from the CommitSuicide-path s_serverText global (which is only populated if that sig resolves).
    static const uint8_t* s_text = nullptr;
    static size_t          s_textSize = 0;
    if (!s_text) { ModText mt = FindModuleText("libserver.so"); s_text = mt.text; s_textSize = mt.size; }
    const uint8_t* f = reinterpret_cast<const uint8_t*>(fn);
    return s_text && f >= s_text && f < s_text + s_textSize;
}

// create: className -> packed CEntityHandle (ToInt). The raw ptr NEVER leaves the shim.
static int Shim_EntityCreate(const char* className) {
    if (!s_pCreateEntityByName || !className) return 0;
    CEntityInstance* ent = s_pCreateEntityByName(className, -1);
    if (!ent) return 0;
    return ent->GetRefEHandle().ToInt();
}

// spawn: DispatchSpawn on a serial-gated entity. Returns 1 on success, 0 if unresolved/stale.
static int Shim_EntitySpawn(int index, int serial) {
    if (!s_pDispatchSpawn) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    s_pDispatchSpawn(ent, nullptr);
    return 1;
}

// spawn-with-keyvalues: DispatchSpawn(ent, CEntityKeyValues) — the entity's own Spawn() parses the
// keys (the SM DispatchKeyValue / CSSharp DispatchSpawn(kv) mechanism). The EKV's whole lifecycle is
// inside this call: build -> AddRef (so a balanced engine AddRef/Release can never delete our-heap
// memory mid-call) -> DispatchSpawn -> Release IF the engine isn't still holding it queued (then we
// WARN once + deliberately leak one small object — a bounded leak beats a UAF/cross-heap free).
static int Shim_EntitySpawnKv(int index, int serial, int count,
                              const char* const* keys, const int* types, const char* const* values) {
    if (!s_pDispatchSpawn) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    void* ekv = S2EKV_Build(count, keys, types, values);
    if (!ekv) return 0;
    S2EKV_AddRef(ekv);
    s_pDispatchSpawn(ent, ekv);
    if (!S2EKV_ReleaseIfSafe(ekv)) {
        static bool s_warnedEkvLeak = false;
        if (!s_warnedEkvLeak) {
            s_warnedEkvLeak = true;
            META_CONPRINTF("[s2script] WARN: engine kept a spawn CEntityKeyValues queued — leaking it (once-per-boot notice)\n");
        }
    }
    return 1;
}

// teleport: CBaseEntity::Teleport via the gamedata vtable index, .text-validated on every call
// (mirrors the trace-slice's load-time check but re-checked here since the index is read once at
// load — this guards against a corrupted/freed vtable slot, not just a stale gamedata index).
// origin/angles/velocity are nullable float[3] pointers (already validated 3-element arrays or
// null by the Rust caller).
static int Shim_EntityTeleport(int index, int serial, const float* o, const float* a, const float* v) {
    if (s_teleportVtblIndex < 0) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    void** vtbl = *reinterpret_cast<void***>(ent);
    void* fn = vtbl[s_teleportVtblIndex];
    if (!IsAddressInServerText(fn)) return 0;
    using TeleportFn = void (*)(void*, const Vector*, const QAngle*, const Vector*);
    reinterpret_cast<TeleportFn>(fn)(ent,
        reinterpret_cast<const Vector*>(o), reinterpret_cast<const QAngle*>(a), reinterpret_cast<const Vector*>(v));
    return 1;
}

// remove: UTIL_Remove on a serial-gated entity. Returns 1 on success, 0 if unresolved/stale.
static int Shim_EntityRemove(int index, int serial) {
    if (!s_pUtilRemove) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    s_pUtilRemove(ent);
    return 1;
}

// setmodel: CBaseEntity::SetModel(const char* modelName) via a validated byte-sig. Gives a runtime
// entity a model (and its collision) — a trigger_multiple needs this for a physics volume that fires
// touch (map triggers get it via InitTrigger->SetModel(GetModelName()); a runtime entity's model name
// is empty, so its InitTrigger SetModel("") builds nothing). Returns 1 on success, 0 if unresolved/stale.
using SetModelFn = void (*)(CEntityInstance* self, const char* modelName);
static SetModelFn s_pSetModel = nullptr;

static int Shim_EntitySetModel(int index, int serial, const char* modelName) {
    if (!s_pSetModel || !modelName) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    s_pSetModel(ent, modelName);
    return 1;
}

// ---------------------------------------------------------------------------
// Sound slice — emit (see docs/superpowers/specs/2026-07-13-sound-emitsound-precache-design.md).
// A minimal modern recipient filter over the SDK's 4-method IRecipientFilter
// (public/irecipientfilter.h), ported from CSSharp's recipientfilters.h: a slot-indexed
// CPlayerBitVec, bounded 0..63. Reliable buffer, never an init message, no predicted slot.
// ---------------------------------------------------------------------------
class S2RecipientFilter : public IRecipientFilter {
public:
    S2RecipientFilter() { m_Recipients.ClearAll(); }
    ~S2RecipientFilter() override {}
    NetChannelBufType_t GetNetworkBufType() const override { return BUF_RELIABLE; }
    bool IsInitMessage() const override { return false; }
    const CPlayerBitVec& GetRecipients() const override { return m_Recipients; }
    CPlayerSlot GetPredictedPlayerSlot() const override { return CPlayerSlot(-1); }
    void AddRecipient(int slot) { if (slot >= 0 && slot < 64) m_Recipients.Set(slot); }
    int Count() const {
        int n = 0;
        for (int s = 0; s < 64; s++) if (m_Recipients.IsBitSet(s)) n++;
        return n;
    }
private:
    CPlayerBitVec m_Recipients;
};

// ---------------------------------------------------------------------------
// Sound slice — precache. CS2 builds the session resource manifest at map load; custom resources are
// added by intercepting the EXISTING CGameRulesGameSystem's OnPrecacheResource(IResourceManifest*)
// (NOT a new game-system registration — CSSharp's heavier fallback).
//
// MECHANISM: a VTABLE-SLOT HOOK of the shared CGameRulesGameSystem CLASS vtable (resolved by RTTI via
// s2vtable::GetVTableByName, the trace-slice self-resolve; gamedata carries only the vtable INDEX, a
// validated HINT). We swap slot[idx] (OnPrecacheResource) to our free handler and save the original.
// This was chosen over the two options the reviewer offered AFTER the offline RE ruled them out on the
// pinned build-2000873 libserver.so:
//   (1) NOT a factory-list walk to the live instance. The plan's premise — the game-system factory
//       node yields the instance at node+24 — is FALSE here: CGameRulesGameSystem's factory is a
//       CGameSystemReallocatingFactory (RTTI "30CGameSystemReallocatingFactoryI20CGameRulesGameSystemS0_E"
//       @ factory-vtable 0x24c9f88; slot 8 IsReallocating -> `mov $1;ret`, slot 9 GetStaticGameSystem
//       -> `xor eax;ret` = nullptr). +0x18 is m_ppGlobalPointer (U**), which the single construction
//       site zeroes statically (`movq $0x0, 0x2867798` @0x18edbb0) and nothing in .text ever
//       re-points; SetGlobalPtr writes THROUGH it (`mov rax,[rdi+0x18]; test; je; mov rsi,(rax)`) so
//       it no-ops forever. The factory therefore never holds the live instance. (The factory is also
//       registered as "GameRulesGameSystem" — NO leading 'C' — @0x90f33e; the C-name strcmp could
//       never have matched anyway.)
//   (2) NOT an inline detour (s2detour) of the slot function body. OnPrecacheResource's prologue
//       @0x18d48e0 STARTS with a RIP-relative `mov [rip+0xf92e79],rdi` — s2detour::Install refuses to
//       relocate any rip-relative stolen instruction (detour.cpp), so it can never patch this fn.
//   (3) NOT a per-instance manual SourceHook: the reallocating factory recreates the instance per map,
//       which would drop an instance-scoped hook. The shared CLASS-vtable patch has none of these
//       problems — no live instance needed, no prologue relocation, and it SURVIVES instance
//       reallocation (a new instance uses the same already-patched class vtable). The class vtable is
//       static data present at module load, so the hook installs ONCE in Load() (no lazy retry).
// ADDING A RESOURCE (sound_precache_add) — review C1 fix. The borrowed ModSharp fact
// "manifest->vtable[0](manifest, path)" (a 2-arg call at slot 0 of the passed pManifest) was
// DISPROVEN against OUR pinned build-2000873 libserver.so. Offline disasm of THIS build's
// OnPrecacheResource (the hooked slot[7] @0x18d48e0) and its byte-identical clone @0x18ce700 shows
// the engine adds each of its default resource strings ("ParticleEffect"/"ParticleEffectStop"/
// "GlassImpact"/"Impact"/"player"/…) by calling a helper @0x19eca40 with the resource string in rdi.
// That helper adds NOT to the passed pManifest but to a GLOBAL manifest singleton g=*0x284f348, via
//     g->vtable[8](g, /*int*/1, /*const char* */path, /*int*/0)
// — a 4-arg call at SLOT 8 (offset 0x40). The old code was therefore wrong on THREE counts (wrong
// object: pManifest vs the global; wrong slot: 0 vs 8; wrong arg count: 2 vs 4), and worse, slot 0
// is the Itanium complete-object destructor — the old `pManifest->vtable[0](pManifest, path)` would
// have DESTRUCTED the live manifest mid-precache -> server crash/corruption at map load. FIX: call
// the engine's own helper verbatim (sig-resolved as "PrecacheAddResource"; it self-resolves the
// global + issues the correct vtable[8] args, so we make NO assumption about the global's offset,
// the vtable index, or pManifest's identity). s_currentPrecacheManifest is now purely a WINDOW GATE:
// it is non-null ONLY for the synchronous duration of the hook dispatch — exactly the window the
// engine's global precache manifest is populated (the original slot[7] reads that global there with
// no null-guard). We never INVOKE the stashed pManifest; it never crosses to JS. Degrade-never-crash:
// helper unresolved / outside the window / out-of-.text -> the add no-ops (returns 0), never a crash.
// ---------------------------------------------------------------------------
typedef void (*OnPrecacheResourceFn_t)(void* thisptr, void* pManifest);
static OnPrecacheResourceFn_t s_origOnPrecacheResource = nullptr;   // saved original slot value (for un-hook + chaining)
static void** s_pGameRulesVtable   = nullptr;                       // the shared CGameRulesGameSystem class vtable (for restore)
static void*  s_currentPrecacheManifest = nullptr;                  // WINDOW GATE: non-null ONLY during the hook dispatch
static int    s_precacheVtblIdx    = -1;                            // gamedata offsets entry (vtable index; a HINT)
static bool   s_precacheHookInstalled = false;                      // the vtable slot is swapped to our handler

// The engine's own "add one resource string to the current precache manifest" helper (@0x19eca40 on
// the pinned build-2000873 libserver.so): a single-arg `void add(const char* path)` that internally
// resolves the global manifest singleton and issues g->vtable[8](g, 1, path, 0). Sig-resolved in
// Load() ("PrecacheAddResource"); null -> the op no-ops (degrade-never-crash).
typedef void (*PrecacheAddResourceFn_t)(const char* path);
static PrecacheAddResourceFn_t s_pPrecacheAddResource = nullptr;

// The sound_precache_add op. Returns the TRUE outcome: 1 ONLY if the engine helper was actually
// invoked against a validated (.text) target inside the live precache window; 0 otherwise. We NEVER
// touch the passed pManifest's vtable (see the block comment — the engine adds via a global helper,
// and slot 0 of the manifest is its destructor).
static int s2_sound_precache_add(const char* path) {
    if (!s_currentPrecacheManifest) return 0;                                    // outside the precache window
    if (!path || !path[0]) return 0;
    if (!s_pPrecacheAddResource) return 0;                                       // sig unresolved -> safe no-op
    if (!IsAddressInServerText(reinterpret_cast<void*>(s_pPrecacheAddResource))) return 0;
    s_pPrecacheAddResource(path);
    return 1;
}

// The OnPrecacheResource replacement (virtual dispatch delivers the SysV register args here:
// rdi=this instance, rsi=manifest). Stash the manifest for the block-scoped sound_precache_add op,
// dispatch to the Sound.onPrecache subscribers, clear, then CHAIN to the original slot (so the game's
// own resource precache still runs). A free function — this is a vtable-slot swap, not a member hook.
static void Detour_OnPrecacheResource(void* thisptr, void* pManifest) {
    s_currentPrecacheManifest = pManifest;
    s2script_core_dispatch_precache();
    s_currentPrecacheManifest = nullptr;
    if (s_origOnPrecacheResource) s_origOnPrecacheResource(thisptr, pManifest);
}

// Overwrite one class-vtable slot (Sound slice — precache). The CGameRulesGameSystem class vtable
// lives in libserver.so's .data.rel.ro (made read-only by RELRO after load), so mprotect the page(s)
// spanning the slot to R/W around the single pointer write, then restore R-only (best-effort). Returns
// false (nothing written) on mprotect failure. Reused for install (-> our handler) and Unload (-> the
// saved original).
static bool WriteVtableSlot(void** vt, int idx, void* fn) {
    void** slot = &vt[idx];
    long pg = sysconf(_SC_PAGESIZE);
    if (pg <= 0) return false;
    uintptr_t a = reinterpret_cast<uintptr_t>(slot);
    uintptr_t pageStart = a & ~static_cast<uintptr_t>(pg - 1);
    size_t span = (a + sizeof(void*)) - pageStart;
    // The mprotect(RW) / pointer-write / mprotect(R) sequence below is NOT atomic, but it is safe here:
    // both callers (InstallPrecacheHook in Load(), the restore in Unload()) and the game systems that
    // dispatch through this vtable run on the main game thread only — there is no concurrent reader.
    if (mprotect(reinterpret_cast<void*>(pageStart), span, PROT_READ | PROT_WRITE) != 0) return false;
    *slot = fn;
    mprotect(reinterpret_cast<void*>(pageStart), span, PROT_READ);   // best-effort restore
    return true;
}

// Install the OnPrecacheResource class-vtable hook (called ONCE from Load; see the block comment for
// the mechanism rationale). RTTI-resolved vtable + the gamedata vtable INDEX (a HINT), the resolved
// slot fn .text-validated before we touch it. Degrade-never-crash: any failure logs + leaves the slot
// untouched (the hook off; onPrecache never fires; emit unaffected). s_precacheVtblIdx is filled from
// the offsets block earlier in Load; its key-existence is reported to the gamedata banner there.
static void InstallPrecacheHook() {
    if (s_precacheHookInstalled) return;
    if (s_precacheVtblIdx < 0) return;   // the offsets-block GamedataResult already recorded the absent key
    void** vt = s2vtable::GetVTableByName("libserver.so", "CGameRulesGameSystem");
    if (!vt) {
        META_CONPRINTF("[s2script] WARN: precache — CGameRulesGameSystem RTTI vtable not found; onPrecache OFF\n");
        return;
    }
    void* slotFn = vt[s_precacheVtblIdx];
    if (!IsAddressInServerText(slotFn)) {   // a stale/wrong index could point anywhere
        META_CONPRINTF("[s2script] WARN: precache — OnPrecacheResource vtbl[%d]=%p out of libserver .text; onPrecache OFF\n",
                       s_precacheVtblIdx, slotFn);
        return;
    }
    s_origOnPrecacheResource = reinterpret_cast<OnPrecacheResourceFn_t>(slotFn);
    if (!WriteVtableSlot(vt, s_precacheVtblIdx, reinterpret_cast<void*>(&Detour_OnPrecacheResource))) {
        META_CONPRINTF("[s2script] WARN: precache — vtable slot mprotect/write failed; onPrecache OFF\n");
        s_origOnPrecacheResource = nullptr;
        return;
    }
    s_pGameRulesVtable = vt;
    s_precacheHookInstalled = true;
    META_CONPRINTF("[s2script] precache hook installed (CGameRulesGameSystem vtable @%p, slot %d, orig=%p)\n",
                   reinterpret_cast<void*>(vt), s_precacheVtblIdx, reinterpret_cast<void*>(s_origOnPrecacheResource));
}

// CBaseEntity::EmitSound — the CSSharp static prototype (entity_manager.h:257), CHOSEN because the
// Task-2 offline RE step DISPROVED the ModSharp member overload on our binary: that member wrapper
// (@0x1a48ee0) takes volume as a float BY VALUE in xmm0 (not the plan's `const float*`), reorders its
// args, and internally builds this very EmitSound_t (storing the xmm0 float at struct offset 20 =
// m_flVolume) before tail-calling the same core fn CSSharp's key resolves to (@0x1a476c0, which xrefs
// "EmitSoundByHandle" and reads m_nForceGuid@32 / m_nFlags-bit-0x10@42 out of the EmitSound_t — every
// field offset below RE-confirmed from that core fn + the member wrapper's stack stores). The committed
// "EmitSound" sig resolves UNIQUE to CSSharp's thin thunk @0x1a48e30, which forwards (rsi=filter,
// edx=ent-index, rcx=&params) straight to that core fn. EmitSound_t is a byte-exact port of CSSharp's
// (entity_manager.h:221, live-proven by CSSharp on this engine); the ctor defaults are CSSharp's
// verbatim (m_nSourceSoundscape 0, m_nPitch PITCH_NORM=100). SndOpEventGuid_t is 24 bytes
// (CSSharp entity_manager.h:250) -> SysV sret: rdi=sret, rsi=filter, edx=ent index, rcx=params.
typedef uint32 SoundEventGuid_t;
struct EmitSound_t {
    const char*      m_pSoundName        = nullptr;
    Vector           m_vecOrigin         = Vector(0.0f, 0.0f, 0.0f);   // 3D positional deferred — zeroed
    float            m_flVolume          = 1.0f;
    float            m_flSoundTime       = 0.0f;
    CEntityIndex     m_nSpeakerEntity    = CEntityIndex(-1);
    SoundEventGuid_t m_nForceGuid        = 0;
    CEntityIndex     m_nSourceSoundscape = CEntityIndex(0);
    uint16           m_nPitch            = 100;   // PITCH_NORM; dead in the engine (CSSharp comment)
    uint8            m_nFlags            = 0;      // 0 = attach to the entity index
};
struct SndOpEventGuid_t {
    uint32 m_nGuid;
    uint64 m_hStackHash;
    uint64 pad;   // CSSharp: "size might be incorrect" — harmless for an out-value we only read m_nGuid from
};
typedef SndOpEventGuid_t (*EmitSoundFn_t)(S2RecipientFilter& filter, CEntityIndex ent,
                                          const EmitSound_t& params);
static EmitSoundFn_t s_pEmitSound = nullptr;   // sig-resolved in Load(); null -> op no-ops

// The sound_emit op. Degrade-never-crash — return 0 WITHOUT calling the engine ONLY when: unresolved
// sig / out-of-.text fn / !soundName / stale-or-null source entity / the CALLER requested no
// recipients (slotCount <= 0 || !slots). An all-bot-skipped filter (Count()==0 after the loop) is
// NOT a degrade — build it and CALL the engine anyway: a PVS/PAS filter excluding everyone is a
// normal, safe engine path (plays to nobody, no netchannel touched), more correct than a "failed" 0
// for a bot-only target, and it exercises the resolved fn + its 24-byte sret ABI + prototype on a
// bots-only live gate. entSerial >= 0 -> serial-gated via ResolveEntityBySerial (the
// pawn_commit_suicide pattern); entSerial < 0 -> the sentinel: entIndex used directly (worldspawn /
// global 2D emit from index 0). Recipient bot-skip: a fake client has no netchannel — it can't hear
// the sound AND a null-netchannel send is the client_print / user_message_send crash surface, so each
// requested slot is admitted only if GetPlayerNetInfo(slot) != null. Volume clamped into [0,1]
// (NaN/out-of-range -> 1.0). NOTE (Variant B): the engine call takes the ENTITY INDEX, not the
// pointer; the serial-gate is kept anyway (resolve `ent`, return 0 if stale) so a dead EntityRef
// still degrades to 0 — the resolved pointer is simply unused past the gate.
static int s2_sound_emit(const char* soundName, int entIndex, int entSerial,
                         const int* slots, int slotCount, float volume) {
    if (!s_pEmitSound || !soundName || !soundName[0]) return 0;
    if (!IsAddressInServerText(reinterpret_cast<void*>(s_pEmitSound))) return 0;
    if (!slots || slotCount <= 0) return 0;                    // CALLER requested no recipients -> no-op
    void* ent = nullptr;
    if (entSerial >= 0) {
        ent = ResolveEntityBySerial(entIndex, entSerial);
    } else {
        ent = s2_ent_by_index(entIndex);
    }
    if (!ent) return 0;                                        // stale/free slot -> no-op
    S2RecipientFilter filter;
    for (int i = 0; i < slotCount; i++) {
        int slot = slots[i];
        if (slot < 0 || slot >= 64) continue;
        if (!s_pEngine || !s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot))) continue;   // bot-skip
        filter.AddRecipient(slot);
    }
    // An all-bot-skipped filter (Count()==0) is NOT a degrade — call the engine anyway (plays to
    // nobody, no netchannel touched). This also exercises the resolved fn on a bots-only live gate.
    float vol = volume;
    if (!(vol >= 0.0f) || vol > 1.0f) vol = 1.0f;               // !(>=0) also catches NaN
    EmitSound_t params;
    params.m_pSoundName = soundName;
    params.m_flVolume   = vol;
    SndOpEventGuid_t guid = s_pEmitSound(filter, CEntityIndex(entIndex), params);
    META_CONPRINTF("[s2script] EmitSound '%s' recipients=%d -> guid=%u\n",
                   soundName, filter.Count(), guid.m_nGuid);
    return static_cast<int>(guid.m_nGuid);
}

// entity_listener_install: called by core on the first-ever JS entity-lifecycle subscribe. Set the
// want-flag + register now (if the entity system exists); the StartupServer POST hook re-asserts each
// map. Returns 1 if the AddListenerEntity signature resolved, else 0 (degrade — subscribe delivers nothing).
static int Shim_EntityListenerInstall() {
    s_wantEntityListener = true;
    EnsureEntityListenerRegistered();
    return s_pAddListenerEntity ? 1 : 0;
}

// collision_activate: register a serial-gated entity's collision with the spatial partition so a
// runtime-created trigger_multiple fires touch (zones real-trigger slice; Task-1 RE). Reaches the
// entity's EMBEDDED CCollisionProperty via the schema m_Collision offset resolved once at Load
// (s_collisionPropOffset — observed 0x8c8, resolved live). Recipe A+B (Task-1 finding): call
// MarkPartitionHandleDirty(collProp) (enqueues into the dirty spatial-partition list) THEN
// UpdatePartition(collProp) (creates the handle IMMEDIATELY this frame, not on the deferred drain).
// Both are single-arg (rdi = CCollisionProperty*). Returns 1 if the calls were made, 0 if
// unresolved/stale. Escalation (SetSolid worker / CollisionRulesChanged) is documented in gamedata
// if A+B proves insufficient at the live gate.
using CollProbeFn = void (*)(void* collisionProperty);   // MarkPartitionHandleDirty / UpdatePartition — both (this)
static CollProbeFn s_pCollMarkDirty       = nullptr;
static CollProbeFn s_pCollUpdatePartition = nullptr;
static int         s_collisionPropOffset  = -1;   // schema m_Collision offset (CBaseModelEntity); -1 = unresolved
static int         s_collisionRulesChangedVtblIndex = -1;   // OUTER entity vtable index; -1 = unresolved
// ModSharp recipe: the REAL engine setters (resolved by validated byte-sig). SetSolid(CCollisionProperty*,
// SolidType) rebuilds the Rubikon collision SHAPE honoring the current solid flags (the step raw schema
// writes + CollisionRulesChanged omit — which is why a raw-written FSOLID_NOT_SOLID box stayed solid).
// SetCollisionBounds(CBaseModelEntity*, &mins, &maxs) sets bounds + recomputes surrounding bounds. Both
// take the OUTER entity / collprop as `this` per ModSharp's Sharp.Shared ICollisionProperty API.
struct S2Vec3 { float x, y, z; };
using SetSolidFn           = void (*)(void* collProp, int solidType);
using SetCollisionBoundsFn = void (*)(void* entity, const void* mins, const void* maxs);
static SetSolidFn           s_pSetSolid           = nullptr;
static SetCollisionBoundsFn s_pSetCollisionBounds = nullptr;
static int s_collMinsOff = -1, s_collMaxsOff = -1, s_collSolidTypeOff = -1;   // absolute offsets on the entity
static int Shim_CollisionActivate(int index, int serial) {
    if (s_collisionPropOffset < 0) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    void* collProp = reinterpret_cast<uint8_t*>(ent) + s_collisionPropOffset;  // embedded, not a ptr deref
    // ModSharp path: call the real setters. SetCollisionBounds first (bounds + surround), then SetSolid
    // (builds the shape + does MarkPartitionHandleDirty+UpdatePartition+CollisionRulesChanged itself — the
    // worker we derived index 185 from). This is what makes an FSOLID_NOT_SOLID box a real non-solid
    // trigger, not a solid blocker.
    if (s_pSetSolid && s_pSetCollisionBounds && s_collMinsOff >= 0 && s_collMaxsOff >= 0 && s_collSolidTypeOff >= 0) {
        S2Vec3 mins = *reinterpret_cast<S2Vec3*>(reinterpret_cast<uint8_t*>(ent) + s_collMinsOff);
        S2Vec3 maxs = *reinterpret_cast<S2Vec3*>(reinterpret_cast<uint8_t*>(ent) + s_collMaxsOff);
        int solidType = *(reinterpret_cast<uint8_t*>(ent) + s_collSolidTypeOff);
        constexpr int SOLID_BBOX = 2;
        s_pSetCollisionBounds(ent, &mins, &maxs);
        // Build the collision SHAPE as SOLID_BBOX from the (schema-written) bounds. SetSolid early-returns
        // on an unchanged type (disasm: `cmp [rdi+0x5b], sil; je ret`), so only call it when the type differs.
        // Then STOP — pass-through (a player walking THROUGH the trigger while touch still fires) comes from
        // the collision GROUP (COLLISION_GROUP_WEAPON=14) set by the JS recipe, NOT from a SOLID_NONE
        // downgrade. The old SOLID_NONE transition here DELETED the collision entirely (no touch), so it is
        // removed.
        if (solidType != SOLID_BBOX) s_pSetSolid(collProp, SOLID_BBOX);
        return 1;
    }
    // Fallback (recipe A+B+D) if the setters didn't resolve: MarkPartitionHandleDirty + UpdatePartition +
    // CollisionRulesChanged. Registers the partition handle but does NOT rebuild the shape (stays solid).
    if (!s_pCollMarkDirty) return 0;
    s_pCollMarkDirty(collProp);
    if (s_pCollUpdatePartition) s_pCollUpdatePartition(collProp);   // immediate handle create (recipe B)
    if (s_collisionRulesChangedVtblIndex >= 0) {
        void** vtbl = *reinterpret_cast<void***>(ent);
        void* fn = vtbl[s_collisionRulesChangedVtblIndex];
        if (IsAddressInServerText(fn)) reinterpret_cast<void (*)(void*)>(fn)(ent);
    }
    return 1;
}

// ---------------------------------------------------------------------------
// Item / weapon manipulation slice (Task 2): GiveNamedItem / RemovePlayerItem are self-validated
// DIRECT byte signatures (resolved below, in Load()), re-confirmed unique + ABI-checked by disasm
// (spike, Task 2). entity_subobj_vcall is the reusable engine-generic primitive: it reads a
// sub-object pointer off the entity at a caller-supplied offset (m_pItemServices/m_pWeaponServices,
// live-schema-resolved JS-side — never a CS2 name in this file) and calls a caller-supplied vtable
// INDEX on it, .text-validated before every call (the same IsAddressInServerText guard as
// Shim_EntityTeleport — a borrowed/stale index can't jump outside libserver.so, but per the
// gamedata-file comment on CCSPlayer_ItemServices_RemoveWeapons/DropActivePlayerWeapon, this spike
// found the two BORROWED indices for THIS build resolve to something else entirely; they are
// wired here only as the generic mechanism, not as confirmed-correct call sites — see the
// gamedata comment). entity_read_handle_vector follows a pointer-deref chain then reads a
// CUtlVector<CHandle> header: `count` @ `vectorOff` (uint32), `elements` @ `vectorOff + 8`
// (CHandle*, 4-byte packed handles). SPIKE FINDING (CUtlVector layout, Task 2): a live disasm
// access site specific to m_hMyWeapons was not pinned down within the spike's bound, but the
// layout is the same CNetworkUtlVectorBase<T> used identically across every Source 2 title for
// over a decade, and is independently cross-checked here against our OWN live schema-catalog.json
// dump: m_hMyWeapons (CPlayer_WeaponServices) and m_networkAnimTiming (CCSPlayer_WeaponServices)
// are both declared exactly 24 bytes wide (the gap to the next field), consistent with
// int32-count(4)+pad(4)+T*-elements(8)+allocCount(4)+growSize(4) = 24 — i.e. count@+0/elements@+8.
// The raw sub-object/elements pointers NEVER cross to JS: every handle is decoded + serial-gated
// via ResolveEntityBySerial (through entity_resolve_ptr on the Rust side) before becoming an
// EntityRef.
// ---------------------------------------------------------------------------
using GiveNamedItemFn    = CEntityInstance* (*)(void* itemServices, const char* name, void* iSubType, void* pScriptItem, void* a5, void* a6);
using RemovePlayerItemFn = bool (*)(void* pawn, void* weapon);
static GiveNamedItemFn    s_pGiveNamedItem    = nullptr;   // sig-resolved fn ptr (loaded in Load)
static RemovePlayerItemFn s_pRemovePlayerItem = nullptr;   // sig-resolved fn ptr (loaded in Load)
static int s_removeWeaponsVtblIndex = -1;   // gamedata (informational; the call site takes the index from the JS caller — see the header comment)
static int s_dropActiveVtblIndex    = -1;   // gamedata (informational; the call site takes the index from the JS caller — see the header comment)

// give: read a sub-object pointer (e.g. m_pItemServices) off a serial-gated entity, call
// GiveNamedItem(itemServices, name, 0, nullptr, 0, nullptr). Returns a packed CEntityHandle
// (ToInt) of the created weapon, or 0 on failure/unresolved. The raw CBaseEntity*/sub-object ptr
// never crosses to JS.
static int Shim_GiveNamedItem(int index, int serial, int subObjOffset, const char* className) {
    if (!s_pGiveNamedItem || !className || subObjOffset < 0) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    void* subObj = *reinterpret_cast<void**>(reinterpret_cast<uint8_t*>(ent) + subObjOffset);
    if (!subObj) return 0;
    CEntityInstance* w = s_pGiveNamedItem(subObj, className, 0, nullptr, 0, nullptr);
    if (!w) return 0;
    return w->GetRefEHandle().ToInt();
}

// subobj vcall: read a sub-object pointer off a serial-gated entity, then call vtable[vtableIndex]
// on that sub-object with an optional single entity arg (argIdx<0 -> null). The resolved fn ptr is
// validated to land inside libserver.so's own .text (IsAddressInServerText) before ever being
// called — a stale/wrong index can't jump outside the module, though (per the gamedata comment on
// the two borrowed indices this slice ships) it may still call the WRONG in-range function.
// vtableIndex is upper-bounded (< 512) BEFORE the vtbl[] read so a hostile/huge index (this native
// is exposed on the plugin global) degrades rather than reading out of bounds — a vtable has a
// natural small bound, unlike a raw byte offset. Returns 1 on success, 0 if unresolved/stale/invalid.
static int Shim_EntitySubobjVcall(int index, int serial, int subObjOffset, int vtableIndex, int argIdx, int argSerial) {
    if (vtableIndex < 0 || vtableIndex >= 512 || subObjOffset < 0) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    void* subObj = *reinterpret_cast<void**>(reinterpret_cast<uint8_t*>(ent) + subObjOffset);
    if (!subObj) return 0;
    void* argPtr = nullptr;
    if (argIdx >= 0) {
        argPtr = ResolveEntityBySerial(argIdx, argSerial);
        if (!argPtr) return 0;   // an explicit arg was requested but didn't resolve -> fail, don't silently drop it
    }
    void** vtbl = *reinterpret_cast<void***>(subObj);
    void* fn = vtbl[vtableIndex];
    if (!IsAddressInServerText(fn)) return 0;
    reinterpret_cast<void (*)(void*, void*)>(fn)(subObj, argPtr);
    return 1;
}

// remove item: RemovePlayerItem(pawn, weapon) -> bool, both serial-gated. Returns 1/0.
static int Shim_RemovePlayerItem(int pawnIndex, int pawnSerial, int weaponIndex, int weaponSerial) {
    if (!s_pRemovePlayerItem) return 0;
    CEntityInstance* pawn = ResolveEntityBySerial(pawnIndex, pawnSerial);
    CEntityInstance* w    = ResolveEntityBySerial(weaponIndex, weaponSerial);
    if (!pawn || !w) return 0;
    s_pRemovePlayerItem(pawn, w);
    return 1;
}

// read handle vector: follow a pointer-deref chain off a serial-gated entity, then read a
// CUtlVector<CHandle> header (count @ vectorOff, elements @ vectorOff+8) and copy up to maxCount
// packed CHandles into out[]. Returns the element count written (<= maxCount), 0 on any
// unresolved step. Every intermediate pointer stays shim-side; only the packed int handles cross.
static int Shim_EntityReadHandleVector(int index, int serial, const int* ptrOffs, int ptrCount, int vectorOff, int maxCount, int* out) {
    if (vectorOff < 0 || maxCount <= 0 || !out) return 0;
    CEntityInstance* ent = ResolveEntityBySerial(index, serial);
    if (!ent) return 0;
    uint8_t* cur = reinterpret_cast<uint8_t*>(ent);
    for (int i = 0; i < ptrCount; i++) {
        cur = *reinterpret_cast<uint8_t**>(cur + ptrOffs[i]);
        if (!cur) return 0;
    }
    uint32_t count = *reinterpret_cast<uint32_t*>(cur + vectorOff);          // size @ +0 (spike-confirmed via schema-catalog struct size cross-check)
    uint8_t* elems = *reinterpret_cast<uint8_t**>(cur + vectorOff + 8);      // elements @ +8
    if (!elems) return 0;
    int n = static_cast<int>(count);
    if (n < 0) n = 0;
    if (n > maxCount) n = maxCount;
    for (int i = 0; i < n; i++) out[i] = *reinterpret_cast<int*>(elems + i * 4);   // CHandle = 4-byte packed int
    return n;
}

// ---------------------------------------------------------------------------
// Entity-I/O slice (Task 2): fire inputs via AddEntityIOEvent (the game's own input-firing path)
// + hook outputs by detouring FireOutputInternal (the 6.6 damage-hook detour engine).
//
// CEntityIOOutput / EntityIOOutputDesc_t: layouts NOT in the vendored SDK (recon gap — CS2's
// public headers don't expose entity-I/O internals). Cross-confirmed via CounterStrikeSharp's
// entity_manager.h (CEntityIOOutput { vtable; EntityIOConnection_t* m_pConnections;
// EntityIOOutputDesc_t* m_pDesc; }, EntityIOOutputDesc_t { const char* m_pName; ...}) AND
// independently disasm-verified at our OWN resolved FireOutputInternal address (spike, Task 2):
// `mov r13,[this+0x8]` walks a linked list (m_pConnections) and `mov r8,[this+0x10]` passes
// m_pDesc to a debug-listener vcall — matches vtable@0/m_pConnections@+0x8/m_pDesc@+0x10 exactly.
// m_pConnections is opaque here (void*; we never walk it — only m_pDesc->m_pName is read).
// ---------------------------------------------------------------------------
struct EntityIOOutputDesc_t {
    const char* m_pName;
    uint32 m_nFlags;
    uint32 m_nOutputOffset;
};
class CEntityIOOutput {
public:
    void* vtable;
    void* m_pConnections;          // opaque linked-list head; not walked here
    EntityIOOutputDesc_t* m_pDesc;
};

// AddEntityIOEvent(entitySystem, target, input, activator, caller, value, delay, outputID,
// unk1, unk2) — the SDK's variant_t (public/variant.h, typedef'd from CVariant) built from a
// string; Source parses it per the input's expected field type. ABI confirmed against
// CounterStrikeSharp's entity_manager.h (CEntitySystem_AddEntityIOEvent, identical arg order).
using AddEntityIOEventFn = void (*)(void* entitySystem, CEntityInstance* target, const char* input,
                                    CEntityInstance* act, CEntityInstance* caller, variant_t* value,
                                    float delay, int outputID, void*, void*);
static AddEntityIOEventFn s_pAddEntityIOEvent = nullptr;

// fire input: (index,serial) serial-gates the target; activator/caller are optional serial-gated
// entities (<0 index = none/null); value is the input's string argument ("" = none). Returns 1/0.
static int Shim_EntityFireInput(int index, int serial, const char* input, const char* value,
                                int actIdx, int actSerial, int callerIdx, int callerSerial, float delay) {
    if (!s_pAddEntityIOEvent || !input) return 0;
    CEntityInstance* target = ResolveEntityBySerial(index, serial);
    if (!target) return 0;
    CGameEntitySystem* sys = GetEntitySystem();
    if (!sys) return 0;
    CEntityInstance* act    = (actIdx    >= 0) ? ResolveEntityBySerial(actIdx, actSerial)       : nullptr;
    CEntityInstance* caller = (callerIdx >= 0) ? ResolveEntityBySerial(callerIdx, callerSerial) : nullptr;
    variant_t v(value ? value : "");   // self-contained: CVariantDefaultAllocator is malloc/free, no tier1
    s_pAddEntityIOEvent(sys, target, input, act, caller, &v, delay, 0, nullptr, nullptr);
    return 1;
}

// Format a CVariant's value as a string WITHOUT calling any tier1 CBufferString method
// (CVariant::ToString()/AssignTo(CBufferString&) call DLL_CLASS_IMPORT tier1 symbols — the exact
// dlopen-cascade blocker from 5D.1/6.1c). Hand-rolled via snprintf on the union fields directly
// (self-contained), mirroring natives_cvariant.cpp's per-type union-member access pattern.
// Unsupported types (Color/Vector2D/Vector4D/EHANDLE/…) degrade to "" — MVP, per the spec's
// documented non-goal (full typed CVariant marshalling deferred).
static void CVariantToString(const CVariant* v, char* buf, size_t bufSize) {
    if (bufSize == 0) return;
    buf[0] = '\0';
    if (!v) return;
    switch (v->m_type) {
        case FIELD_VOID:      break;
        case FIELD_FLOAT32:   snprintf(buf, bufSize, "%g", v->m_float32); break;
        case FIELD_FLOAT64:   snprintf(buf, bufSize, "%g", v->m_float64); break;
        case FIELD_INT32:     snprintf(buf, bufSize, "%d", v->m_int32); break;
        case FIELD_UINT32:    snprintf(buf, bufSize, "%u", v->m_uint32); break;
        case FIELD_INT64:     snprintf(buf, bufSize, "%lld", static_cast<long long>(v->m_int64)); break;
        case FIELD_UINT64:    snprintf(buf, bufSize, "%llu", static_cast<unsigned long long>(v->m_uint64)); break;
        case FIELD_BOOLEAN:   snprintf(buf, bufSize, "%s", v->m_bool ? "true" : "false"); break;
        case FIELD_CHARACTER: snprintf(buf, bufSize, "%c", v->m_char); break;
        case FIELD_STRING:    snprintf(buf, bufSize, "%s", v->m_stringt.ToCStr()); break;
        case FIELD_CSTRING:   snprintf(buf, bufSize, "%s", v->m_pszString ? v->m_pszString : ""); break;
        case FIELD_VECTOR:
            if (v->m_pVector) snprintf(buf, bufSize, "%g %g %g", v->m_pVector->x, v->m_pVector->y, v->m_pVector->z);
            break;
        case FIELD_QANGLE:
            if (v->m_pQAngle) snprintf(buf, bufSize, "%g %g %g", v->m_pQAngle->x, v->m_pQAngle->y, v->m_pQAngle->z);
            break;
        default: break;   // unsupported -> "" (already set above)
    }
}

// The FireOutputInternal detour target + trampoline (installed/removed via the shim's shared
// s2detour engine — same mechanism as the 6.6 DispatchTraceAttack / 6.11b HostSay detours).
using FireOutputInternalFn = void (*)(CEntityIOOutput*, CEntityInstance*, CEntityInstance*, const CVariant*, float, void*, char*);
static FireOutputInternalFn s_origFireOutputInternal = nullptr;

// Extract output name/class/activator/caller/value, dispatch SYNCHRONOUSLY to core (the
// damage/event pre-hook pattern — a handler must be able to block), and supersede (skip) the
// original call when the collapsed HookResult is >= Handled (2). The raw CEntityIOOutput* /
// CEntityInstance* / CVariant* pointers NEVER cross to JS: activator/caller cross as packed
// CEntityHandle ints (core decodes + serial-gate-validates them via the existing entity-ref
// path), the value crosses as a string. Any resolve failure (null pThis/m_pDesc/m_pName) falls
// straight through to the original — never suppress on a shim-side miss.
static void Hook_FireOutputInternal(CEntityIOOutput* pThis, CEntityInstance* act, CEntityInstance* caller,
                                    const CVariant* value, float delay, void* u1, char* u2) {
    int result = 0;   // Continue
    if (pThis && pThis->m_pDesc && pThis->m_pDesc->m_pName) {
        const char* outputName = pThis->m_pDesc->m_pName;
        const char* cls = caller ? caller->GetClassname() : "";
        int actH    = act    ? act->GetRefEHandle().ToInt()    : -1;
        int callerH = caller ? caller->GetRefEHandle().ToInt() : -1;
        char valbuf[256];
        CVariantToString(value, valbuf, sizeof valbuf);
        result = s2script_core_dispatch_output(cls, outputName, actH, callerH, valbuf, delay);
    }
    if (result >= 2) return;   // Handled|Stop -> suppress: skip the original (do NOT call it)
    if (s_origFireOutputInternal) s_origFireOutputInternal(pThis, act, caller, value, delay, u1, u2);
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
                // Voice-control slice: the 7th sibling — throttled voice-packet notify.
                SH_ADD_HOOK(ISource2GameClients, ClientVoice, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientVoice), true);   // POST, like CSSharp
                s_voiceNotifyHookInstalled = true;
                META_CONPRINTF("[s2script] voice: ClientVoice hook installed (throttled notify)\n");
            } else {
                m_gameClients = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: Source2GameClients (%s) — console commands off\n", verStr);
            }
        }

        // Acquire ISource2GameEntities + install the CheckTransmit POST hook (checktransmit
        // slice): per-client entity visibility filtering. A sibling of the Source2GameClients
        // acquisition — same serverFactory, same degrade-never-crash. The hook only OBSERVES
        // until the first-fire layout validation passes (see Hook_CheckTransmit).
        {
            auto it = versions.find("Source2GameEntities");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : INTERFACEVERSION_SERVERGAMEENTS;
            int ret = 0;
            m_gameEntities = serverFactory
                ? reinterpret_cast<ISource2GameEntities*>(serverFactory(verStr, &ret)) : nullptr;
            std::string ctiErr;
            auto ctiOffsets = LoadOffsets(GamedataPath(), "linuxsteamrt64", ctiErr);
            auto cit = ctiOffsets.find("CheckTransmitInfo_clientEntityIndex");
            s_ctiClientOff = (ctiErr.empty() && cit != ctiOffsets.end() && cit->second >= 0)
                                 ? cit->second : -1;
            GamedataResult("CheckTransmitInfo_clientEntityIndex", s_ctiClientOff >= 0,
                           !ctiErr.empty() ? ctiErr.c_str() : "offset key absent from gamedata");
            // Reset the per-Load hook state (a shim reload starts a fresh validation cycle).
            s_transmitTable.clear();
            s_transmitLayoutState = 0;
            s_transmitValidateAttempts = 0;
            s_transmitSnapshots = 0; s_transmitBitsCleared = 0;
            s_transmitNsLast = 0; s_transmitNsMax = 0;
            if (m_gameEntities && ret == 0 && s_ctiClientOff >= 0 && m_clientLifecycleHooksInstalled) {
                META_CONPRINTF("[s2script] interface OK: Source2GameEntities (%s)\n", verStr);
                SH_ADD_HOOK(ISource2GameEntities, CheckTransmit, m_gameEntities,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_CheckTransmit), true);
                m_checkTransmitHookInstalled = true;
                META_CONPRINTF("[s2script] CheckTransmit hook installed (entity visibility; "
                               "layout validates on first fire)\n");
            } else if (!m_gameEntities || ret != 0) {
                m_gameEntities = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: Source2GameEntities (%s) — "
                               "transmit filtering off\n", verStr);
            } else if (!m_clientLifecycleHooksInstalled) {
                META_CONPRINTF("[s2script] WARN: CheckTransmit hook NOT installed — client lifecycle "
                               "hooks unavailable (validation needs signon tracking); transmit "
                               "filtering off\n");
            } else {
                META_CONPRINTF("[s2script] WARN: CheckTransmitInfo offset not in gamedata — "
                               "transmit filtering off\n");
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
                // Voice-control slice: the mute-enforcement rewrite hook. Enforcement stays gated on
                // the runtime validation (first-fire sanity here, Get/Set round-trip at 2nd
                // ClientActive) because the eiface vtable region is hand-patched.
                SH_ADD_HOOK(IVEngineServer2, SetClientListening, s_pEngine,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_SetClientListening), false);  // PRE
                s_voiceListenHookInstalled = true;
                META_CONPRINTF("[s2script] voice: SetClientListening hook installed (mute enforcement)\n");
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
                // OnMapStart (clientlist-fakeconvar-onmapstart slice): POST hook StartupServer on the
                // just-acquired NetworkServerService — the CSSharp OnMapStart mechanism.
                SH_ADD_HOOK(INetworkServerService, StartupServer,
                            static_cast<INetworkServerService*>(s_pNetworkServerService),
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_StartupServer), true);   // POST
                m_startupServerHookInstalled = true;
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
            // changeteam slice: resolve CCSPlayerController::ChangeTeam (Player.changeTeam / .spectate). A
            // DIRECT prologue signature self-resolved on OUR libserver.so (the real function, located via the
            // CTMDBG log-string xref — NOT CSSharp's SwitchTeam sig [deferred switch] nor its ChangeTeam vtable
            // OFFSET [slot 101 is a ret stub here; ChangeTeam is slot 102 — index drift]). Also (re)sets
            // s_serverText so the call-site .text guard holds even if the CommitSuicide sig failed.
            // Degrade-never-crash: unresolved -> change_team no-op.
            auto stit = sigs.find("ChangeTeam");
            if (stit == sigs.end()) {
                GamedataResult("ChangeTeam", false, "signature absent from gamedata");
            } else {
                int64_t stOff = ResolveSigValidated("ChangeTeam", stit->second);
                ModText stmt = FindModuleText(stit->second.module.c_str());
                if (stOff != s2sig::kFail && stmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pChangeTeam = reinterpret_cast<ChangeTeam_t>(const_cast<uint8_t*>(stmt.text) + stOff);
                    s_serverText = stmt.text; s_serverTextSize = stmt.size;   // .text range for the call guard
                    META_CONPRINTF("[s2script] ChangeTeam resolved @%p (Player.changeTeam; libserver .text=%p+%zu)\n",
                                   reinterpret_cast<void*>(s_pChangeTeam), (const void*)s_serverText, s_serverTextSize);
                }   // stOff == kFail: ResolveSigValidated already recorded the reason
            }
            // switchteam slice: resolve CCSPlayerController::SwitchTeam (Player.switchTeam — the
            // NON-LETHAL T/CT move). Sig corroborated by SwiftlyS2 + CSSharp but VALIDATED on OUR
            // libserver.so (unique @0x1525f40 on 2000875) — NOT the changeteam-era borrowed sig that
            // hit the deferred m_bSwitchTeamsOnNextRoundReset function (see the gamedata comment).
            // Degrade-never-crash: unresolved -> switch_team no-ops (the spectator dispatch to
            // ChangeTeam still works if that sig resolved).
            auto swit = sigs.find("SwitchTeam");
            if (swit == sigs.end()) {
                GamedataResult("SwitchTeam", false, "signature absent from gamedata");
            } else {
                int64_t swOff = ResolveSigValidated("SwitchTeam", swit->second);
                ModText swmt = FindModuleText(swit->second.module.c_str());
                if (swOff != s2sig::kFail && swmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pSwitchTeam = reinterpret_cast<SwitchTeam_t>(const_cast<uint8_t*>(swmt.text) + swOff);
                    META_CONPRINTF("[s2script] SwitchTeam resolved @%p (Player.switchTeam; libserver .text=%p+%zu)\n",
                                   reinterpret_cast<void*>(s_pSwitchTeam), (const void*)s_serverText, s_serverTextSize);
                }   // swOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Round-control slice: resolve CCSGameRules::TerminateRound (GameRules.terminateRound).
            // DUAL-GATED: unique-match (ResolveSigValidated) AND the scope-string semantic check —
            // on THIS build the borrowed CSSharp/Swiftly sig is unique yet lands on the WRONG function,
            // so uniqueness alone must never assign the pointer. Failure of either gate leaves
            // s_pTerminateRound null -> the op degrades to 0 (degrade-never-crash).
            auto trit = sigs.find("TerminateRound");
            if (trit == sigs.end()) {
                GamedataResult("TerminateRound", false, "signature absent from gamedata");
            } else {
                int64_t trOff = ResolveSigValidated("TerminateRound", trit->second);
                ModText trmt = FindModuleText(trit->second.module.c_str());
                if (trOff != s2sig::kFail && trmt.text) {
                    if (!ValidateTerminateRoundScopeString(trmt, trOff, trit->second.module.c_str())) {
                        GamedataResult("TerminateRound.scope-string", false,
                            "prologue lea does not reference the 'TerminateRound' scope string "
                            "(unique-but-WRONG match — the borrowed-sig trap); descriptor disabled");
                    } else {
                        GamedataResult("TerminateRound.scope-string", true, nullptr);
                        s_pTerminateRound = reinterpret_cast<TerminateRound_t>(const_cast<uint8_t*>(trmt.text) + trOff);
                        s_serverText = trmt.text; s_serverTextSize = trmt.size;
                        META_CONPRINTF("[s2script] TerminateRound resolved @%p (GameRules.terminateRound)\n",
                                       reinterpret_cast<void*>(s_pTerminateRound));
                        // Eager drain-hook install (NOT lazy): adding a SourceHook from inside a frame
                        // dispatch would mutate the hook chain mid-iteration; one if-not-armed branch
                        // per frame is negligible.
                        if (m_server && !s_termDrainHooked) {
                            SH_ADD_HOOK(ISource2Server, GameFrame, m_server,
                                        SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRoundDrain), false);
                            s_termDrainHooked = true;
                        }
                    }
                }   // trOff == kFail: ResolveSigValidated already recorded the reason
            }
            // player-respawn slice: resolve CCSPlayerController::Respawn (Player.respawn).
            // DUAL-GATED: unique-match (ResolveSigValidated) AND RTTI vtable membership — CSSharp
            // ships a bare vtable index here (the sm_slay-400/ChangeTeam-101 borrowed-index class),
            // so the shipped self-derived sig must additionally prove it landed on a genuine
            // CCSPlayerController virtual. Failure of either gate leaves s_pRespawn null -> the op
            // degrades to 0 (degrade-never-crash) and the drain hook is never installed.
            auto rsit = sigs.find("Respawn");
            if (rsit == sigs.end()) {
                GamedataResult("Respawn", false, "signature absent from gamedata");
            } else {
                int64_t rsOff = ResolveSigValidated("Respawn", rsit->second);
                ModText rsmt = FindModuleText(rsit->second.module.c_str());
                if (rsOff != s2sig::kFail && rsmt.text) {
                    const uint8_t* rsfn = rsmt.text + rsOff;
                    if (!ValidateRespawnVtableMember(rsfn, rsmt)) {
                        GamedataResult("Respawn.vtable-member", false,
                            "sig-resolved address is NOT a member of the RTTI-derived "
                            "CCSPlayerController primary vtable (unique-but-WRONG match — the "
                            "borrowed-sig trap); descriptor disabled");
                    } else {
                        GamedataResult("Respawn.vtable-member", true, nullptr);
                        s_pRespawn = reinterpret_cast<Respawn_t>(const_cast<uint8_t*>(rsfn));
                        s_serverText = rsmt.text; s_serverTextSize = rsmt.size;
                        META_CONPRINTF("[s2script] Respawn resolved @%p (Player.respawn)\n",
                                       reinterpret_cast<void*>(s_pRespawn));
                    }
                }   // rsOff == kFail: ResolveSigValidated already recorded the reason
            }
            // player-respawn slice: resolve CBasePlayerController::SetPawn (the pre-step that re-activates
            // a dead player's pawn — Respawn-alone only clears the death screen, live-gate proven). NON-
            // VIRTUAL on 2000875 -> unique-match + .text guard only (no vtable-member gate). Unresolved ->
            // s_pSetPawn null -> the op degrades to 0 (respawn needs BOTH Respawn AND SetPawn).
            auto spit = sigs.find("SetPawn");
            if (spit == sigs.end()) {
                GamedataResult("SetPawn", false, "signature absent from gamedata");
            } else {
                int64_t spOff = ResolveSigValidated("SetPawn", spit->second);
                ModText spmt = FindModuleText(spit->second.module.c_str());
                if (spOff != s2sig::kFail && spmt.text) {
                    s_pSetPawn = reinterpret_cast<SetPawn_t>(const_cast<uint8_t*>(spmt.text) + spOff);
                    if (!s_serverText) { s_serverText = spmt.text; s_serverTextSize = spmt.size; }
                    META_CONPRINTF("[s2script] SetPawn resolved @%p (respawn pre-step)\n",
                                   reinterpret_cast<void*>(s_pSetPawn));
                }   // spOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Eager drain-hook install (NOT lazy — mutating the hook chain from inside a frame dispatch is
            // unsafe). Install ONLY if BOTH engine facts resolved: respawn needs Respawn AND SetPawn.
            if (s_pRespawn && s_pSetPawn && m_server && !s_respawnDrainHooked) {
                SH_ADD_HOOK(ISource2Server, GameFrame, m_server,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRespawnDrain), false);
                s_respawnDrainHooked = true;
            }
            // Slice menu: resolve GetLegacyGameEventListener (per-client event fire; Events.fireToClient).
            // A DIRECT prologue signature self-resolved on OUR libserver.so (CSSharp reaches the per-client
            // listener via this engine function, NOT a CServerSideClient cast). Unresolved ->
            // s_pGetLegacyListener stays null -> s2_event_fire_to_client no-ops (degrade-never-crash).
            auto lelit = sigs.find("LegacyGameEventListener");
            if (lelit == sigs.end()) {
                GamedataResult("LegacyGameEventListener", false, "signature absent from gamedata");
            } else {
                int64_t lelOff = ResolveSigValidated("LegacyGameEventListener", lelit->second);
                ModText lelmt = FindModuleText(lelit->second.module.c_str());
                if (lelOff != s2sig::kFail && lelmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pGetLegacyListener = reinterpret_cast<GetLegacyListener_t>(const_cast<uint8_t*>(lelmt.text) + lelOff);
                    META_CONPRINTF("[s2script] LegacyGameEventListener resolved @%p (Events.fireToClient)\n",
                                   reinterpret_cast<void*>(s_pGetLegacyListener));
                }   // lelOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Entity-creation lifecycle slice (Task 2): resolve UTIL_CreateEntityByName,
            // CBaseEntity::DispatchSpawn, and UTIL_Remove — all DIRECT prologue signatures
            // self-validated on OUR libserver.so. Degrade-never-crash: any unresolved leaves its
            // s_p* null -> the matching entity_* op no-ops (createEntity/spawn/remove -> null/false).
            auto ucbnit = sigs.find("UtilCreateEntityByName");
            if (ucbnit == sigs.end()) {
                GamedataResult("UtilCreateEntityByName", false, "signature absent from gamedata");
            } else {
                int64_t ucbnOff = ResolveSigValidated("UtilCreateEntityByName", ucbnit->second);
                ModText ucbnmt = FindModuleText(ucbnit->second.module.c_str());
                if (ucbnOff != s2sig::kFail && ucbnmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pCreateEntityByName = reinterpret_cast<CreateEntityByNameFn>(const_cast<uint8_t*>(ucbnmt.text) + ucbnOff);
                    META_CONPRINTF("[s2script] UtilCreateEntityByName resolved @%p (createEntity)\n",
                                   reinterpret_cast<void*>(s_pCreateEntityByName));
                }   // ucbnOff == kFail: ResolveSigValidated already recorded the reason
            }
            auto dsit = sigs.find("DispatchSpawn");
            if (dsit == sigs.end()) {
                GamedataResult("DispatchSpawn", false, "signature absent from gamedata");
            } else {
                int64_t dsOff = ResolveSigValidated("DispatchSpawn", dsit->second);
                ModText dsmt = FindModuleText(dsit->second.module.c_str());
                if (dsOff != s2sig::kFail && dsmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pDispatchSpawn = reinterpret_cast<DispatchSpawnFn>(const_cast<uint8_t*>(dsmt.text) + dsOff);
                    META_CONPRINTF("[s2script] DispatchSpawn resolved @%p (EntityRef.spawn)\n",
                                   reinterpret_cast<void*>(s_pDispatchSpawn));
                }   // dsOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Zones real-trigger slice: resolve CBaseEntity::SetModel (DIRECT sig, fresh CSSharp
            // gamedata for build 2000873). Absent/unresolved -> s_pSetModel null -> the entity_set_model
            // op no-ops (setModel -> false). Gives a runtime trigger a model -> physics volume -> touch.
            auto smit = sigs.find("SetModel");
            if (smit == sigs.end()) {
                GamedataResult("SetModel", false, "signature absent from gamedata");
            } else {
                int64_t smOff = ResolveSigValidated("SetModel", smit->second);
                ModText smmt = FindModuleText(smit->second.module.c_str());
                if (smOff != s2sig::kFail && smmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pSetModel = reinterpret_cast<SetModelFn>(const_cast<uint8_t*>(smmt.text) + smOff);
                    META_CONPRINTF("[s2script] SetModel resolved @%p (EntityRef.setModel)\n",
                                   reinterpret_cast<void*>(s_pSetModel));
                }
            }
            // Sound slice: resolve CBaseEntity::EmitSound (soundevent emit; Sound.emit /
            // pawn.emitSound). A DIRECT prologue signature self-validated UNIQUE on OUR libserver.so
            // (the Task-2 offline RE step disproved the ModSharp member prototype — volume is by-value,
            // not a const float* — so the committed sig + Variant B EmitSound_t call shape are the
            // RE finding; see the gamedata "EmitSound" comment). The unique match is CSSharp's thunk
            // that forwards to the core EmitSound. Unresolved -> s_pEmitSound stays null -> sound_emit
            // no-ops (degrade-never-crash).
            auto esit = sigs.find("EmitSound");
            if (esit == sigs.end()) {
                GamedataResult("EmitSound", false, "signature absent from gamedata");
            } else {
                int64_t esOff = ResolveSigValidated("EmitSound", esit->second);
                ModText esmt = FindModuleText(esit->second.module.c_str());
                if (esOff != s2sig::kFail && esmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pEmitSound = reinterpret_cast<EmitSoundFn_t>(const_cast<uint8_t*>(esmt.text) + esOff);
                    META_CONPRINTF("[s2script] EmitSound resolved @%p (Sound.emit)\n",
                                   reinterpret_cast<void*>(s_pEmitSound));
                }   // esOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Sound slice (precache ADD — review C1): resolve the engine's own "add one resource string
            // to the current precache manifest" helper (@0x19eca40 on build 2000873). The precache HOOK
            // stays a CLASS-vtable swap resolved by RTTI at InstallPrecacheHook time (below); only the
            // ADD is a signature, because the borrowed "manifest->vtable[0]" fact was disproven on our
            // binary (see the s2_sound_precache_add block comment + the "PrecacheAddResource" gamedata
            // sig). Unresolved -> s_pPrecacheAddResource stays null -> sound_precache_add no-ops.
            auto parit = sigs.find("PrecacheAddResource");
            if (parit == sigs.end()) {
                GamedataResult("PrecacheAddResource", false, "signature absent from gamedata");
            } else {
                int64_t parOff = ResolveSigValidated("PrecacheAddResource", parit->second);
                ModText parmt = FindModuleText(parit->second.module.c_str());
                if (parOff != s2sig::kFail && parmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pPrecacheAddResource = reinterpret_cast<PrecacheAddResourceFn_t>(
                        const_cast<uint8_t*>(parmt.text) + parOff);
                    META_CONPRINTF("[s2script] PrecacheAddResource resolved @%p (Sound.precache add)\n",
                                   reinterpret_cast<void*>(s_pPrecacheAddResource));
                }   // parOff == kFail: ResolveSigValidated already recorded the reason
            }
            // (Sound slice precache HOOK: no signature here — the OnPrecacheResource hook is a CLASS-vtable
            // swap resolved by RTTI (s2vtable::GetVTableByName) at InstallPrecacheHook time, not a
            // sig-resolved factory-list walk. The abandoned "GameSystemFactoryList" signature +
            // instance-from-factory premise are documented at InstallPrecacheHook / in gamedata.)
            // Zones real-trigger slice: resolve CCollisionProperty::MarkPartitionHandleDirty +
            // UpdatePartition (both DIRECT sigs) + the embedded m_Collision offset. Degrade-never-crash:
            // MarkPartitionHandleDirty unresolved -> op no-ops; UpdatePartition unresolved -> recipe A only.
            s_collisionPropOffset = s2_schema_offset("CBaseModelEntity", "m_Collision");
            auto cmdit = sigs.find("CollisionMarkPartitionDirty");
            if (cmdit == sigs.end()) {
                GamedataResult("CollisionMarkPartitionDirty", false, "signature absent from gamedata");
            } else {
                int64_t cmdOff = ResolveSigValidated("CollisionMarkPartitionDirty", cmdit->second);
                ModText cmdmt = FindModuleText(cmdit->second.module.c_str());
                if (cmdOff != s2sig::kFail && cmdmt.text) {
                    s_pCollMarkDirty = reinterpret_cast<CollProbeFn>(const_cast<uint8_t*>(cmdmt.text) + cmdOff);
                    META_CONPRINTF("[s2script] CollisionMarkPartitionDirty resolved @%p (collision_activate)\n",
                                   reinterpret_cast<void*>(s_pCollMarkDirty));
                }
            }
            auto cupit = sigs.find("CollisionUpdatePartition");
            if (cupit == sigs.end()) {
                GamedataResult("CollisionUpdatePartition", false, "signature absent from gamedata");
            } else {
                int64_t cupOff = ResolveSigValidated("CollisionUpdatePartition", cupit->second);
                ModText cupmt = FindModuleText(cupit->second.module.c_str());
                if (cupOff != s2sig::kFail && cupmt.text) {
                    s_pCollUpdatePartition = reinterpret_cast<CollProbeFn>(const_cast<uint8_t*>(cupmt.text) + cupOff);
                    META_CONPRINTF("[s2script] CollisionUpdatePartition resolved @%p (collision_activate)\n",
                                   reinterpret_cast<void*>(s_pCollUpdatePartition));
                }
            }
            // ModSharp recipe: the REAL engine setters CCollisionProperty::SetSolid (rebuilds the Rubikon
            // shape honoring solid flags) + CBaseModelEntity::SetCollisionBounds (bounds + surround). Both
            // DIRECT sigs (validated unique vs the pinned libserver.so). Absent -> Shim_CollisionActivate
            // falls back to the A+B+D partition-only path (registers touch but stays solid).
            if (s_collisionPropOffset >= 0) {
                int mo = s2_schema_offset("CCollisionProperty", "m_vecMins");
                int xo = s2_schema_offset("CCollisionProperty", "m_vecMaxs");
                int so = s2_schema_offset("CCollisionProperty", "m_nSolidType");
                s_collMinsOff      = (mo >= 0) ? s_collisionPropOffset + mo : -1;
                s_collMaxsOff      = (xo >= 0) ? s_collisionPropOffset + xo : -1;
                s_collSolidTypeOff = (so >= 0) ? s_collisionPropOffset + so : -1;
            }
            auto ssit = sigs.find("CollisionSetSolid");
            if (ssit == sigs.end()) {
                GamedataResult("CollisionSetSolid", false, "signature absent from gamedata");
            } else {
                int64_t ssOff = ResolveSigValidated("CollisionSetSolid", ssit->second);
                ModText ssmt = FindModuleText(ssit->second.module.c_str());
                if (ssOff != s2sig::kFail && ssmt.text) {
                    s_pSetSolid = reinterpret_cast<SetSolidFn>(const_cast<uint8_t*>(ssmt.text) + ssOff);
                    META_CONPRINTF("[s2script] CollisionSetSolid resolved @%p (collision_activate/ModSharp)\n",
                                   reinterpret_cast<void*>(s_pSetSolid));
                }
            }
            auto scbit = sigs.find("SetCollisionBounds");
            if (scbit == sigs.end()) {
                GamedataResult("SetCollisionBounds", false, "signature absent from gamedata");
            } else {
                int64_t scbOff = ResolveSigValidated("SetCollisionBounds", scbit->second);
                ModText scbmt = FindModuleText(scbit->second.module.c_str());
                if (scbOff != s2sig::kFail && scbmt.text) {
                    s_pSetCollisionBounds = reinterpret_cast<SetCollisionBoundsFn>(const_cast<uint8_t*>(scbmt.text) + scbOff);
                    META_CONPRINTF("[s2script] SetCollisionBounds resolved @%p (collision_activate/ModSharp)\n",
                                   reinterpret_cast<void*>(s_pSetCollisionBounds));
                }
            }
            auto urit = sigs.find("UtilRemove");
            if (urit == sigs.end()) {
                GamedataResult("UtilRemove", false, "signature absent from gamedata");
            } else {
                int64_t urOff = ResolveSigValidated("UtilRemove", urit->second);
                ModText urmt = FindModuleText(urit->second.module.c_str());
                if (urOff != s2sig::kFail && urmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pUtilRemove = reinterpret_cast<UtilRemoveFn>(const_cast<uint8_t*>(urmt.text) + urOff);
                    META_CONPRINTF("[s2script] UtilRemove resolved @%p (EntityRef.remove)\n",
                                   reinterpret_cast<void*>(s_pUtilRemove));
                }   // urOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Item slice (Task 2): resolve CCSPlayer_ItemServices::GiveNamedItem and
            // CBasePlayerPawn::RemovePlayerItem — DIRECT prologue signatures self-validated on OUR
            // libserver.so (re-confirmed unique + ABI-checked by disasm in the Task-2 spike).
            // Degrade-never-crash: unresolved leaves its s_p* null -> the matching op no-ops.
            auto gnit = sigs.find("GiveNamedItem");
            if (gnit == sigs.end()) {
                GamedataResult("GiveNamedItem", false, "signature absent from gamedata");
            } else {
                int64_t gnOff = ResolveSigValidated("GiveNamedItem", gnit->second);
                ModText gnmt = FindModuleText(gnit->second.module.c_str());
                if (gnOff != s2sig::kFail && gnmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pGiveNamedItem = reinterpret_cast<GiveNamedItemFn>(const_cast<uint8_t*>(gnmt.text) + gnOff);
                    META_CONPRINTF("[s2script] GiveNamedItem resolved @%p (pawn.giveNamedItem)\n",
                                   reinterpret_cast<void*>(s_pGiveNamedItem));
                }   // gnOff == kFail: ResolveSigValidated already recorded the reason
            }
            auto rpiit = sigs.find("RemovePlayerItem");
            if (rpiit == sigs.end()) {
                GamedataResult("RemovePlayerItem", false, "signature absent from gamedata");
            } else {
                int64_t rpiOff = ResolveSigValidated("RemovePlayerItem", rpiit->second);
                ModText rpimt = FindModuleText(rpiit->second.module.c_str());
                if (rpiOff != s2sig::kFail && rpimt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pRemovePlayerItem = reinterpret_cast<RemovePlayerItemFn>(const_cast<uint8_t*>(rpimt.text) + rpiOff);
                    META_CONPRINTF("[s2script] RemovePlayerItem resolved @%p (pawn.removeWeapon)\n",
                                   reinterpret_cast<void*>(s_pRemovePlayerItem));
                }   // rpiOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Entity-I/O slice (Task 2): resolve CEntitySystem::AddEntityIOEvent (fires inputs; the
            // primary EntityRef.acceptInput mechanism) — a DIRECT prologue signature self-validated
            // on OUR libserver.so. Degrade-never-crash: unresolved leaves s_pAddEntityIOEvent null ->
            // entity_fire_input no-ops.
            auto aioit = sigs.find("AddEntityIOEvent");
            if (aioit == sigs.end()) {
                GamedataResult("AddEntityIOEvent", false, "signature absent from gamedata");
            } else {
                int64_t aioOff = ResolveSigValidated("AddEntityIOEvent", aioit->second);
                ModText aiomt = FindModuleText(aioit->second.module.c_str());
                if (aioOff != s2sig::kFail && aiomt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pAddEntityIOEvent = reinterpret_cast<AddEntityIOEventFn>(const_cast<uint8_t*>(aiomt.text) + aioOff);
                    META_CONPRINTF("[s2script] AddEntityIOEvent resolved @%p (EntityRef.acceptInput)\n",
                                   reinterpret_cast<void*>(s_pAddEntityIOEvent));
                }   // aioOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Entity-I/O slice (Task 2): resolve + detour CEntityIOOutput::FireOutputInternal (the
            // output-hook entry) — same direct-prologue + inline-detour pattern as
            // DispatchTraceAttack/HostSay. Degrade-never-crash: unresolved leaves outputs unhooked
            // (Entity.onOutput never fires), never a crash.
            auto foiit = sigs.find("FireOutputInternal");
            if (foiit == sigs.end()) {
                GamedataResult("FireOutputInternal", false, "signature absent from gamedata");
            } else {
                int64_t foiOff = ResolveSigValidated("FireOutputInternal", foiit->second);
                ModText foimt = FindModuleText(foiit->second.module.c_str());
                if (foiOff != s2sig::kFail && foimt.text) {  // resolve=="direct": the unique match IS the function start
                    void* foiAddr = const_cast<uint8_t*>(foimt.text) + foiOff;
                    if (s2detour::Install(foiAddr, reinterpret_cast<void*>(&Hook_FireOutputInternal),
                                          reinterpret_cast<void**>(&s_origFireOutputInternal))) {
                        META_CONPRINTF("[s2script] FireOutputInternal hooked @%p (Entity.onOutput)\n", foiAddr);
                    } else {
                        META_CONPRINTF("[s2script] WARN: FireOutputInternal detour install failed — Entity.onOutput off\n");
                    }
                }   // foiOff == kFail: ResolveSigValidated already recorded the reason
            }
            // Entity lifecycle listeners slice: resolve CGameEntitySystem::AddListenerEntity (register an
            // IEntityListener) + RemoveListenerEntity (best-effort Unload cleanup). Both validated UNIQUE +
            // .text via ResolveSigValidated. Unresolved -> entity_listener_install no-ops / Unload skips remove.
            auto aleit = sigs.find("AddListenerEntity");
            if (aleit == sigs.end()) {
                GamedataResult("AddListenerEntity", false, "signature absent from gamedata");
            } else {
                int64_t aleOff = ResolveSigValidated("AddListenerEntity", aleit->second);
                ModText alemt = FindModuleText(aleit->second.module.c_str());
                if (aleOff != s2sig::kFail && alemt.text) {
                    s_pAddListenerEntity = reinterpret_cast<AddRemoveListenerFn>(const_cast<uint8_t*>(alemt.text) + aleOff);
                    META_CONPRINTF("[s2script] AddListenerEntity resolved @%p (entity lifecycle listeners)\n",
                                   reinterpret_cast<void*>(s_pAddListenerEntity));
                    // E1: the entity books are load-bearing for ALL entity access now — the
                    // listener is wanted from boot, not only after a JS Entity.on* subscribe.
                    // (Registration still happens at StartupServer POST via
                    // EnsureEntityListenerRegistered — the entity system doesn't exist yet here.)
                    s_wantEntityListener = true;
                }
            }
            auto rleit = sigs.find("RemoveListenerEntity");
            if (rleit == sigs.end()) {
                GamedataResult("RemoveListenerEntity", false, "signature absent from gamedata");
            } else {
                int64_t rleOff = ResolveSigValidated("RemoveListenerEntity", rleit->second);
                ModText rlemt = FindModuleText(rleit->second.module.c_str());
                if (rleOff != s2sig::kFail && rlemt.text) {
                    s_pRemoveListenerEntity = reinterpret_cast<AddRemoveListenerFn>(const_cast<uint8_t*>(rlemt.text) + rleOff);
                    META_CONPRINTF("[s2script] RemoveListenerEntity resolved @%p (entity lifecycle listeners)\n",
                                   reinterpret_cast<void*>(s_pRemoveListenerEntity));
                }
            }
            // Usercmd primitive: resolve CCSPlayer_MovementServices::ProcessUsercmds (the per-tick input
            // entry) into s_pProcessUsercmdsAddr — a DIRECT prologue signature self-validated on OUR
            // libserver.so. Degrade-never-crash: unresolved leaves s_pProcessUsercmdsAddr null ->
            // Shim_UsercmdHookInstall (usercmd_hook_install) no-ops -> UserCmd.onRun never fires.
            // LAZY: the detour is NOT installed here — only resolved into an address; s2detour::Install
            // runs later, once, on the first UserCmd.onRun subscribe (see Shim_UsercmdHookInstall).
            auto puit = sigs.find("ProcessUsercmds");
            if (puit == sigs.end()) {
                GamedataResult("ProcessUsercmds", false, "signature absent from gamedata");
            } else {
                int64_t puOff = ResolveSigValidated("ProcessUsercmds", puit->second);
                ModText pumt = FindModuleText(puit->second.module.c_str());
                if (puOff != s2sig::kFail && pumt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pProcessUsercmdsAddr = const_cast<uint8_t*>(pumt.text) + puOff;
                    META_CONPRINTF("[s2script] ProcessUsercmds resolved @%p (UserCmd.onRun; lazy install)\n",
                                   s_pProcessUsercmdsAddr);
                }   // puOff == kFail: ResolveSigValidated already recorded the reason
            }
        }
        // Load the engine-identity offsets (Slice 5D.2). Absent/typoed keys stay -1 -> degrade.
        {
            std::string offErr;
            auto offs = LoadOffsets(GamedataPath(), "linuxsteamrt64", offErr);
            auto pick = [&](const char* k) { auto i = offs.find(k); return i != offs.end() ? i->second : -1; };
            // Entity-creation lifecycle slice (Task 2): CBaseEntity::Teleport's vtable INDEX. A
            // borrowed index is a HINT, not trusted blind — Shim_EntityTeleport re-validates the
            // resolved fn ptr lands inside libserver.so's .text on EVERY call (IsAddressInServerText),
            // so an absent/stale key here just means "no createEntity live gate for the value would be
            // needed" — the real safety check is per-call, not this presence check.
            s_teleportVtblIndex = pick("CBaseEntity_Teleport");
            GamedataResult("CBaseEntity_Teleport", s_teleportVtblIndex >= 0, "offset (vtable index) key absent from gamedata");
            // Sound slice: the OnPrecacheResource vtable index (a HINT — InstallPrecacheHook resolves
            // the CGameRulesGameSystem class vtable by RTTI then .text-validates vtbl[idx] before
            // swapping the slot; see the InstallPrecacheHook / gamedata comment).
            s_precacheVtblIdx = pick("CGameRulesGameSystem_OnPrecacheResource");
            GamedataResult("CGameRulesGameSystem_OnPrecacheResource", s_precacheVtblIdx >= 0,
                           "offset (vtable index) key absent from gamedata");
            // clientlist-fakeconvar-onmapstart slice: the six 5D.2 engine-identity offsets
            // (NetworkServerService.gameServer / NetworkGameServer.clientCount+clientElems /
            // ServerSideClient.name+signon+userId) are RETIRED — the client ops now use typed SDK
            // virtuals (GetIGameServer / GetPlayerUserId / GetClientConVarValue) + a lifecycle-tracked
            // signon array, so there is nothing to pick() or validate here (offsets were never re-scanned,
            // which is exactly how they went stale on 2000870). GAMEDATA VALIDATION count drops by 6.
        }
        // Resolve CNavPhysicsInterface::TraceShape via an RTTI vtable-by-name scan (ray-trace
        // slice, Task 1). CS2 does not export this vtable through dlsym (stripped .symtab, not in
        // .dynsym) — s2vtable::GetVTableByName locates it from the RTTI type_info name embedded in
        // .rodata (self-resolve doctrine: no borrowed pointer, only a borrowed vtable INDEX, which
        // is gamedata and validated below). Unresolved -> s_pTraceShape stays null -> the
        // trace_shape op degrades to a no-op (never a crash).
        {
            void** vt = s2vtable::GetVTableByName("libserver.so", "CNavPhysicsInterface");
            if (!vt) {
                GamedataResult("CNavPhysicsInterface (RTTI vtable)", false,
                               "RTTI typeinfo/vtable not found in libserver.so — regenerate");
            } else {
                std::string offErr;
                auto offs = LoadOffsets(GamedataPath(), "linuxsteamrt64", offErr);
                auto oit = offs.find("CNavPhysicsInterface_TraceShape");
                if (oit == offs.end() || oit->second < 0) {
                    GamedataResult("CNavPhysicsInterface_TraceShape", false,
                                   "offset (vtable index) key absent from gamedata");
                } else {
                    int idx = oit->second;
                    void* fn = vt[idx];
                    // Validate the resolved slot lands inside libserver.so's own executable range
                    // (Rule 2 parity with ResolveSigValidated) — a borrowed/stale index could point
                    // anywhere; a wrong-but-in-range value would otherwise crash on first call.
                    ModText svt = FindModuleText("libserver.so");
                    const uint8_t* f = reinterpret_cast<const uint8_t*>(fn);
                    if (fn && svt.text && f >= svt.text && f < svt.text + svt.size) {
                        s_pTraceShape = reinterpret_cast<s2trace::TraceShapeFn>(fn);
                        GamedataResult("CNavPhysicsInterface_TraceShape", true, nullptr);
                        META_CONPRINTF("[s2script] trace: OK (RTTI CNavPhysicsInterface idx %d, fn=%p)\n",
                                       idx, fn);
                    } else {
                        GamedataResult("CNavPhysicsInterface_TraceShape", false,
                                       "resolved fn ptr outside libserver.so .text — stale index");
                        META_CONPRINTF("[s2script] trace: MISSING (resolved slot out of range)\n");
                    }
                }
            }
        }
        // Sound slice: install the precache hook (a CGameRulesGameSystem class-vtable swap resolved by
        // RTTI, like the trace block above). The class vtable is static data present at module load, so
        // this installs ONCE here — no lazy StartupServer retry. Before GamedataBanner so its warn (if
        // the RTTI vtable is missing) prints alongside the rest of the gamedata report.
        InstallPrecacheHook();
        GamedataBanner();   // Slice 6.9: loud pass/fail summary — a version mismatch screams here, not later.

        // EKV self-test (permanent, treadmill): link/ctor/layout integrity of the compiled-in
        // CEntityKeyValues. A failure degrades kv-spawns to false — it disables nothing else.
        META_CONPRINTF("[s2script] EKV self-test: %s\n", S2EKV_SelfTest() ? "OK" : "FAILED (kv-spawn degraded)");
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
    // Slice DB: APPENDED after server_game_time; order MUST match S2EngineOps.
    ops.db_data_dir        = &s2_db_data_dir;
    // Slice menu: per-client event fire — APPENDED after db_data_dir; order MUST match S2EngineOps.
    ops.event_fire_to_client = &s2_event_fire_to_client;
    // Slice nominations: raw configs-dir file read/write — APPENDED after event_fire_to_client; order MUST match S2EngineOps.
    ops.config_read_file  = &s2_config_read_file;
    ops.config_write_file = &s2_config_write_file;
    // Ray-trace slice — APPENDED after config_write_file; order MUST match S2EngineOps.
    ops.trace_shape = &s2_trace_shape;
    // Entity-creation lifecycle slice — APPENDED after trace_shape; order MUST match S2EngineOps.
    ops.entity_create   = &Shim_EntityCreate;
    ops.entity_spawn    = &Shim_EntitySpawn;
    ops.entity_teleport = &Shim_EntityTeleport;
    ops.entity_remove   = &Shim_EntityRemove;
    // Item slice — APPENDED after entity_remove; order MUST match S2EngineOps.
    ops.give_named_item           = &Shim_GiveNamedItem;
    ops.entity_subobj_vcall       = &Shim_EntitySubobjVcall;
    ops.remove_player_item        = &Shim_RemovePlayerItem;
    ops.entity_read_handle_vector = &Shim_EntityReadHandleVector;
    // Entity-I/O slice — APPENDED after entity_read_handle_vector; order MUST match S2EngineOps.
    ops.entity_fire_input = &Shim_EntityFireInput;
    // EKV slice — APPENDED after entity_fire_input; order MUST match S2EngineOps.
    ops.entity_spawn_kv = &Shim_EntitySpawnKv;
    // Game-rules + UserMessage slice — APPENDED after entity_spawn_kv; order MUST match S2EngineOps.
    ops.entity_find_by_class = &s2_entity_find_by_class;
    // UserMessage send family — APPENDED after entity_find_by_class; order MUST match S2EngineOps.
    ops.user_message_create     = &s2_user_message_create;
    ops.user_message_set_int    = &s2_user_message_set_int;
    ops.user_message_set_float  = &s2_user_message_set_float;
    ops.user_message_set_string = &s2_user_message_set_string;
    ops.user_message_set_bool   = &s2_user_message_set_bool;
    ops.user_message_send       = &s2_user_message_send;
    // FakeConVar slice — APPENDED after user_message_send; order MUST match S2EngineOps.
    ops.convar_register         = &s2_convar_register;
    // Translations slice — APPENDED after convar_register; order MUST match S2EngineOps.
    ops.translations_read = &s2_translations_read;
    ops.client_language   = &s2_client_language;
    // Zones real-trigger slice — APPENDED after client_language; order MUST match S2EngineOps.
    ops.collision_activate = &Shim_CollisionActivate;
    // Zones real-trigger slice — APPENDED after collision_activate; order MUST match S2EngineOps.
    ops.entity_set_model = &Shim_EntitySetModel;
    // Entity lifecycle listeners slice — APPENDED after entity_set_model; order MUST match S2EngineOps.
    ops.entity_listener_install = &Shim_EntityListenerInstall;
    // entity_name slice — APPENDED after entity_listener_install; order MUST match S2EngineOps.
    ops.entity_name = &s2_entity_name;
    // Sound slice — APPENDED after entity_name; order MUST match S2EngineOps.
    // Both op fns are defined above: s2_sound_emit with the emit block, s2_sound_precache_add with the
    // precache vtable-hook block (which also defines Detour_OnPrecacheResource / InstallPrecacheHook).
    ops.sound_emit         = &s2_sound_emit;
    ops.sound_precache_add = &s2_sound_precache_add;
    // changeteam slice — APPENDED after sound_precache_add; order MUST match S2EngineOps.
    ops.player_change_team = &s2_player_change_team;
    // Usercmd primitive — APPENDED after player_change_team; order MUST match S2EngineOps.
    ops.usercmd_hook_install  = &Shim_UsercmdHookInstall;
    ops.usercmd_read          = &s2_usercmd_read;
    ops.usercmd_write         = &s2_usercmd_write;
    ops.usercmd_read_buttons  = &s2_usercmd_read_buttons;
    ops.usercmd_write_buttons = &s2_usercmd_write_buttons;
    ops.usercmd_clear_subtick = &s2_usercmd_clear_subtick;
    // checktransmit slice — APPENDED after usercmd_clear_subtick; order MUST match S2EngineOps.
    ops.transmit_set   = &s2_transmit_set;
    ops.transmit_clear = &s2_transmit_clear;
    ops.transmit_stats = &s2_transmit_stats;
    // Round-control slice — APPENDED after transmit_stats; order MUST match S2EngineOps.
    ops.gamerules_terminate_round = &s2_gamerules_terminate_round;
    // Voice-control slice — APPENDED after gamerules_terminate_round; order MUST match S2EngineOps.
    ops.voice_set_muted = &s2_voice_set_muted;
    ops.voice_get_muted = &s2_voice_get_muted;
    // switchteam slice — APPENDED after voice_get_muted; order MUST match S2EngineOps.
    ops.player_switch_team = &s2_player_switch_team;
    // UserMessage-interception slice — APPENDED after player_switch_team; order MUST match S2EngineOps.
    ops.usermsg_hook_sub         = &s2_usermsg_hook_sub;
    ops.usermsg_hook_unsub       = &s2_usermsg_hook_unsub;
    ops.usermsg_hook_read_int    = &s2_usermsg_hook_read_int;
    ops.usermsg_hook_read_float  = &s2_usermsg_hook_read_float;
    ops.usermsg_hook_read_string = &s2_usermsg_hook_read_string;
    ops.usermsg_hook_has_field   = &s2_usermsg_hook_has_field;
    ops.usermsg_hook_recipients  = &s2_usermsg_hook_recipients;
    ops.usermsg_hook_debug       = &s2_usermsg_hook_debug;
    // player-respawn slice — APPENDED after usermsg_hook_debug; order MUST match S2EngineOps.
    ops.player_respawn = &s2_player_respawn;
    // Crash-reporter slice — APPENDED after player_respawn; order MUST match S2EngineOps.
    ops.server_build_number = &s2_server_build_number;
    // Crash-harness — APPENDED after server_build_number; order MUST match S2EngineOps.
    ops.crash_test_native = &s2_crash_test_native;
    // E1 entity-liveness slice (APPENDED after crash_test_native; order is the ABI).
    ops.ent_resolve        = &s2_ent_resolve;
    ops.ent_identity_flags = &s2_ent_identity_flags;
    ops.ent_snapshot       = &s2_ent_snapshot;

    // Pass both callbacks + the engine-ops table; the core calls s2_request_hook("OnGameFrame", 1)
    // to lazily install the SourceHook detour once a script subscribes.
    if (s2script_core_init(&s2_logger, &s2_request_hook, &ops) != 0) {
        META_CONPRINTF("[s2script] ERROR: V8 core init failed (plugin stays loaded for diagnosis)\n");
        return true; // degrade, do not fail the load (spec §7)
    }

    // --- Crash reporter: identity + spool-dir push (fail-off: any miss degrades to "") ---
    {
        // FNV-1a 64 over a file's bytes; also reused for the registered game-package JS below.
        auto fnv64hex = [](const std::string& bytes) -> std::string {
            uint64_t h = 0xcbf29ce484222325ULL;
            for (unsigned char c : bytes) { h ^= c; h *= 0x100000001b3ULL; }
            char out[17];
            snprintf(out, sizeof out, "%016llx", (unsigned long long)h);
            return std::string(out);
        };
        auto slurp = [](const std::string& path) -> std::string {
            FILE* f = fopen(path.c_str(), "rb");
            if (!f) return std::string();
            fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
            std::string s(sz > 0 ? (size_t)sz : 0, '\0');
            if (sz > 0 && fread(&s[0], 1, (size_t)sz, f) != (size_t)sz) s.clear();
            fclose(f);
            return s;
        };
        std::string gdPath = GamedataPath();
        std::string gdBytes = slurp(gdPath);
        std::string gdFp = gdBytes.empty() ? "" : fnv64hex(gdBytes);
        char gdMtime[32] = "";
        struct stat st{};
        if (stat(gdPath.c_str(), &st) == 0)
            snprintf(gdMtime, sizeof gdMtime, "%lld", (long long)st.st_mtime);
        std::string schemaHash;
        {
            std::string js = slurp(Cs2JsPath());   // the deployed pawn.js concat carries the
            if (!js.empty()) schemaHash = fnv64hex(js);  // generated schema accessors (D-6)
        }
        std::string spool = CrashSpoolDir();
#ifndef S2_HL2SDK_BUILD
#define S2_HL2SDK_BUILD "unknown"
#endif
        s2script_core_crash_set_identity(gdFp.c_str(), gdMtime, S2_HL2SDK_BUILD,
                                         schemaHash.c_str(), s_gdFail, spool.c_str());
        META_CONPRINTF("[s2script] crash identity pushed (gamedata %s, spool %s)\n",
                       gdFp.empty() ? "<none>" : gdFp.c_str(), spool.c_str());

        // Arm Breakpad AFTER core init (spec §6.2) — boot-time crashes from here on are caught.
        // Fail-off: an empty spool dir leaves the reporter disarmed and the server running.
        if (!spool.empty() &&
            S2CrashArm(spool.c_str(), s2script_core_crash_breadcrumb(),
                       s2script_core_crash_breadcrumb_size())) {
            META_CONPRINTF("[s2script] crash handler armed (spool %s)\n", spool.c_str());
        } else {
            META_CONPRINTF("[s2script] WARN: crash handler NOT armed (spool dir unavailable)\n");
        }
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

    // player-respawn slice: remove the deferred-drain GameFrame pre-hook (installed eagerly at Load
    // iff both Respawn boot gates passed) and clear any un-drained pending entries.
    if (s_respawnDrainHooked && m_server) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRespawnDrain), false);
        s_respawnDrainHooked = false;
        s_pendingRespawnCount = 0;
    }

    if (s_termDrainHooked && m_server) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRoundDrain), false);
        s_termDrainHooked = false;
        s_pendingTerminate.armed = false;
    }

    // Remove the FireEvent pre-hook (Slice 5D.3) before tearing down the event listener.
    if (m_eventHookInstalled && s_pGameEventManager) {
        SH_REMOVE_HOOK(IGameEventManager2, FireEvent, s_pGameEventManager,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_FireEventPre), false);
        m_eventHookInstalled = false;
    }

    // Remove the lazy PostEventAbstract pre-hook (usermsg-hook slice — ledger/teardown authority).
    if (s_userMsgHookInstalled && s_pGameEventSystem) {
        SH_REMOVE_HOOK(IGameEventSystem, PostEventAbstract, s_pGameEventSystem,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_PostEvent), false);
        s_userMsgHookInstalled = false;
        s_userMsgFirstFireDone = false;                       // a later re-arm re-observes + re-validates
        for (auto& w : s_userMsgSubBits) w = 0;               // clear the subscribed-id bitmap
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

    // Remove the CheckTransmit POST hook (checktransmit slice) + drop the rule table.
    if (m_checkTransmitHookInstalled && m_gameEntities) {
        SH_REMOVE_HOOK(ISource2GameEntities, CheckTransmit, m_gameEntities,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_CheckTransmit), true);
        m_checkTransmitHookInstalled = false;
    }
    s_transmitTable.clear();

    // Voice-control slice: remove both voice hooks. Any forced-false listen values already stored in
    // the engine are restored by the game's own next voice refresh (engine-paced; see live-gate note).
    if (s_voiceNotifyHookInstalled && m_gameClients) {
        SH_REMOVE_HOOK(ISource2GameClients, ClientVoice, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientVoice), true);
        s_voiceNotifyHookInstalled = false;
    }
    if (s_voiceListenHookInstalled && s_pEngine) {
        SH_REMOVE_HOOK(IVEngineServer2, SetClientListening, s_pEngine,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_SetClientListening), false);
        s_voiceListenHookInstalled = false;
    }

    // Remove the StartupServer map-start POST hook (clientlist-fakeconvar-onmapstart slice).
    if (m_startupServerHookInstalled && s_pNetworkServerService) {
        SH_REMOVE_HOOK(INetworkServerService, StartupServer,
                       static_cast<INetworkServerService*>(s_pNetworkServerService),
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_StartupServer), true);
        m_startupServerHookInstalled = false;
    }

    // Restore the OnPrecacheResource class-vtable slot (Sound slice — precache vtable hook): write the
    // saved original back before core teardown so the game's own precache path is intact if the shim is
    // reloaded. Guarded on the install flag + the saved vtable/original so a never-installed (or
    // failed-install) hook no-ops cleanly.
    if (s_precacheHookInstalled && s_pGameRulesVtable && s_origOnPrecacheResource && s_precacheVtblIdx >= 0) {
        // WARN loudly if the restore write fails: a failed restore leaves vtable[idx] pointing at our
        // Detour_OnPrecacheResource, which is about to be unmapped -> the next precache would jump into
        // freed memory and crash. Nothing we can do to recover here, but the log names the hazard.
        if (!WriteVtableSlot(s_pGameRulesVtable, s_precacheVtblIdx, reinterpret_cast<void*>(s_origOnPrecacheResource))) {
            META_CONPRINTF("[s2script] WARN: precache — vtable slot restore write FAILED; slot still points at the "
                           "detour being unloaded (next precache may crash)\n");
        }
        s_precacheHookInstalled = false;
        s_pGameRulesVtable = nullptr;
        s_origOnPrecacheResource = nullptr;
    }

    // Entity lifecycle listeners slice: unregister the IEntityListener so a dangling vtable call can't
    // happen if s2script is unloaded while the entity system lives. Best-effort (unresolved sig -> skip).
    if (s_wantEntityListener && s_pRemoveListenerEntity) {
        CGameEntitySystem* es = GetEntitySystem();
        if (es) s_pRemoveListenerEntity(es, S2_GetEntityListener());
    } else if (s_wantEntityListener && s_pAddListenerEntity && !s_pRemoveListenerEntity) {
        // We registered the listener (AddListenerEntity resolved) but CANNOT unregister it
        // (RemoveListenerEntity signature is unresolved/stale on this build). The listener object is
        // about to be freed with the .so, so the next engine-driven entity create/spawn/delete would
        // call a dangling vtable -> SIGSEGV. We cannot safely remove it, so at least tell the operator
        // loudly (the boot GAMEDATA VALIDATION gate also flags the stale RemoveListenerEntity sig).
        META_CONPRINTF("[s2script] WARN: entity listener registered but RemoveListenerEntity is "
                       "unresolved on this build -- a DANGLING listener remains; do NOT hot-unload "
                       "s2script until the RemoveListenerEntity signature is regenerated (regenerate "
                       "gamedata for this CS2 build).\n");
    }

    // Slice 6.6: restore the DispatchTraceAttack prologue (removes the damage detour) before core teardown.
    // s2detour tracks every installed patch in one process-global list (shim/src/detour.cpp), so this
    // ALSO restores the ProcessUsercmds detour (usercmd primitive) if it was ever lazily installed —
    // no usercmd-specific teardown code is needed here.
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

    S2CrashDisarm();   // restore previous signal handlers before the core is torn down

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

// player-respawn slice: drain the pending respawn set (enqueued by s2_player_respawn from a JS native)
// OUTSIDE the JS isolate borrow — the entire point of the deferral. Respawn fires player_spawn
// SYNCHRONOUSLY, so running it here (not inline in the op) lets that event flow through the normal
// FireEvent pre-hook -> core dispatch -> every plugin's subscribers instead of being try_borrow-skipped.
// Installed eagerly at Load iff both boot gates passed; removed at Unload. Per entry: re-deref the
// handle (serial-gated at drain, not just enqueue — the controller can die in between), re-check
// m_bPawnIsAlive at alive_off (skip if the player came alive), .text-guard s_pRespawn.
void S2ScriptPlugin::Hook_GameFrameRespawnDrain(bool, bool, bool) {
    if (s_pendingRespawnCount > 0) {
        PendingRespawn batch[kRespawnPendingMax];
        int n = s_pendingRespawnCount;
        std::memcpy(batch, s_pendingRespawn, sizeof(PendingRespawn) * n);
        s_pendingRespawnCount = 0;                       // consume BEFORE calling (the call re-enters)
        const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pRespawn);
        const uint8_t* g = reinterpret_cast<const uint8_t*>(s_pSetPawn);
        if (!s_pRespawn || !s_pSetPawn || !s_serverText ||
            f < s_serverText || f >= s_serverText + s_serverTextSize ||
            g < s_serverText || g >= s_serverText + s_serverTextSize) {
            META_CONPRINTF("[s2script] player_respawn: Respawn/SetPawn fn out of libserver .text at drain — batch dropped\n");
            RETURN_META(MRES_IGNORED);
        }
        for (int i = 0; i < n; i++) {
            void* controller = s2_deref_handle(batch[i].handle);   // re-gate: it can die in between
            if (!controller) {
                META_CONPRINTF("[s2script] player_respawn: stale controller at drain — skipped\n");
                continue;
            }
            if (batch[i].aliveOff >= 0 &&
                *reinterpret_cast<const uint8_t*>(reinterpret_cast<const char*>(controller) + batch[i].aliveOff)) {
                continue;                                // came alive between enqueue and drain — skip
            }
            // SetPawn(playerPawn, true, false) THEN Respawn — SwiftlyS2/CSSharp's exact sequence, SAME
            // frame. A dead controller's active m_hPawn points at the observer pawn; SetPawn re-points it +
            // tears down observer mode + sets dirty flags — a raw m_hPawn write does NOT (live-gate proven on
            // 2000875: Respawn alone only clears the death screen, never spawns). Resolve the player pawn from
            // its handle (opaque offset; schema strings stay in games/cs2); skip SetPawn on a stale/absent
            // pawn handle (Respawn alone still runs — degrade, not crash). NOTE the engine Respawn HONORS the
            // game's respawn rules — it no-ops on a competitive mid-round server (verified) and fires in
            // gamemodes that permit respawn (warmup / TTT's own rules), which is the correct behavior.
            if (batch[i].hplayerpawnOff >= 0) {
                uint32_t hp = *reinterpret_cast<const uint32_t*>(
                    reinterpret_cast<const char*>(controller) + batch[i].hplayerpawnOff);
                void* playerPawn = s2_deref_handle(hp);
                if (playerPawn) s_pSetPawn(controller, playerPawn, /*b1*/1, /*b2*/0);   // (pawn,true,false) — SwiftlyS2/CSSharp exact
            }
            // OUTSIDE the JS isolate borrow: the synchronous player_spawn flows through the normal
            // FireEvent pre-hook -> core dispatch -> every plugin's subscribers.
            s_pRespawn(controller);
        }
    }
    RETURN_META(MRES_IGNORED);
}

void S2ScriptPlugin::Hook_GameFrameRoundDrain(bool, bool, bool) {
    if (s_pendingTerminate.armed) {
        PendingTerminate req = s_pendingTerminate;
        s_pendingTerminate.armed = false;               // consume before calling (the call re-enters gamerules)
        void* proxy = s2_deref_handle(req.proxyHandle); // re-gate: the proxy can die between enqueue and drain
        const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pTerminateRound);
        if (proxy && s_pTerminateRound && s_serverText && f >= s_serverText && f < s_serverText + s_serverTextSize) {
            void* rules = *reinterpret_cast<void**>(reinterpret_cast<char*>(proxy) + req.rulesPtrOff);
            if (rules) {
                // OUTSIDE the JS isolate borrow: the synchronous round_end flows through the normal
                // FireEvent pre-hook -> core dispatch -> every plugin's subscribers.
                s_pTerminateRound(rules, req.delay, static_cast<uint32_t>(req.reason), nullptr, 0);
            } else {
                META_CONPRINTF("[s2script] terminate_round: null rules pointer at drain — dropped\n");
            }
        } else {
            META_CONPRINTF("[s2script] terminate_round: stale proxy / fn out of .text at drain — dropped\n");
        }
    }
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

// UserMessage-interception choke point (usermsg-hook slice): every outbound event/message posts through
// here. Order: recursion guard -> degraded guard -> observe-only FIRST-FIRE validation (never suppresses)
// -> bitmap gate on m_MessageId (one virtual + one bit test; MRES_IGNORED on miss BEFORE any reflection/
// strcmp/FFI/alloc/logging) -> name-keyed core dispatch with block-scoped statics -> collapsed HookResult
// >= Handled(2) => MRES_SUPERCEDE (the message is dropped for every recipient AND any server-side local
// listener — the live gate watches for server-side fallout; fallback = recall-with-modified-mask).
void S2ScriptPlugin::Hook_PostEvent(CSplitScreenSlot nSlot, bool bLocalOnly, int nClientCount,
                                    const uint64* clients, INetworkMessageInternal* pEvent,
                                    const CNetMessage* pData, unsigned long nSize,
                                    NetChannelBufType_t bufType) {
    (void)nSlot; (void)bLocalOnly; (void)nSize; (void)bufType;
    if (s_inUserMsgDispatch) RETURN_META(MRES_IGNORED);   // recursion guard (a mid-hook send re-enters here)
    // Cheap gate FIRST: one virtual (GetNetMessageInfo) + one bitmap bit test on m_MessageId. A non-subscribed
    // message costs exactly this before ANY reflection/strcmp/FFI/alloc/logging. Doctrine note on the ONE
    // borrowed layout fact (NetMessageInfo_t::m_MessageId): it is RANGE-CHECKED fail-closed at subscribe and
    // used only for this SELF-CONSISTENT pre-filter (subscribe and dispatch read the same offset); the
    // AUTHORITATIVE dispatch key is GetUnscopedName() (a reliable virtual, no layout dependency). A drifted
    // offset therefore degrades to at-worst a wasted dispatch the name-mux drops, or a fail-closed subscribe —
    // never a false suppression.
    NetMessageInfo_t* mi = pEvent ? pEvent->GetNetMessageInfo() : nullptr;
    if (!mi || !s2_usermsg_bit((int)mi->m_MessageId)) RETURN_META(MRES_IGNORED);
    google::protobuf::Message* pb = pData
        ? reinterpret_cast<google::protobuf::Message*>(const_cast<CNetMessage*>(pData)->AsProto()) : nullptr;
    if (!pb) RETURN_META(MRES_IGNORED);
    const char* nm = pEvent->GetUnscopedName();
    // Observe-only first-fire, gated on the first SUBSCRIBED message that reaches here (deterministic — a
    // message a plugin actually asked for, NOT the arbitrary first engine post, which could be bodyless and
    // must never disable anything). Validate reflection is readable, log the operator banner, and NEVER
    // dispatch/suppress this one fire. A reflection failure skips THIS fire only (per-descriptor; the send
    // path is untouched and every other subscribed message still works) — it never globally latches.
    if (!s_userMsgFirstFireDone) {
        s_userMsgFirstFireDone = true;
        if (!pb->GetDescriptor() || !pb->GetReflection() || !nm || !*nm) {
            META_CONPRINTF("[s2script] USERMSG VALIDATION: subscribed message id=%d lacks readable protobuf "
                           "reflection — this fire skipped (send path unaffected)\n", (int)mi->m_MessageId);
            RETURN_META(MRES_IGNORED);
        }
        META_CONPRINTF("[s2script] USERMSG intercept validated (first subscribed fire: id=%d name=%s)\n",
                       (int)mi->m_MessageId, nm);
        RETURN_META(MRES_IGNORED);   // observe-only: the hook goes live on the NEXT subscribed fire
    }
    s_hookMsg = pb; s_hookClients = clients; s_hookClientCount = nClientCount;
    s_inUserMsgDispatch = true;
    int result = s2script_core_dispatch_usermsg(nm, (int)mi->m_MessageId);
    s_inUserMsgDispatch = false;
    s_hookMsg = nullptr; s_hookClients = nullptr; s_hookClientCount = 0;   // block-scope ends here
    if (result >= 2 /* HookResult.Handled */) RETURN_META(MRES_SUPERCEDE);
    RETURN_META(MRES_IGNORED);
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
//
// clientlist-fakeconvar-onmapstart slice: these bodies now ALSO drive the tracked signon array
// (s_trackedSignon) that s2_client_signon/valid/userid/name read (the offset-free replacement). State
// is set BEFORE the core dispatch so a handler observes the new signon (connect: valid==true during
// dispatch); disconnect clears AFTER dispatch so the handler still sees the client as valid.
void S2ScriptPlugin::Hook_OnClientConnected(CPlayerSlot slot, const char*, uint64, const char*, const char*, bool) {
    int s = slot.Get();
    if (s >= 0 && s < kMaxClientSlots) s_trackedSignon[s] = kSignonConnected;
    s2script_core_dispatch_client_event("connect", s);
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientPutInServer(CPlayerSlot slot, const char*, int, uint64) {
    int s = slot.Get();
    if (s >= 0 && s < kMaxClientSlots) s_trackedSignon[s] = kSignonSpawn;
    s2script_core_dispatch_client_event("putinserver", s);
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientActive(CPlayerSlot slot, bool, const char*, uint64) {
    int s = slot.Get();
    if (s >= 0 && s < kMaxClientSlots) s_trackedSignon[s] = kSignonFull;
    MaybeValidateVoiceListening();   // one-shot Get/Set round-trip once two clients are active
    s2script_core_dispatch_client_event("active", s);
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientFullyConnect(CPlayerSlot slot) {
    int s = slot.Get();
    if (s >= 0 && s < kMaxClientSlots) s_trackedSignon[s] = kSignonFull;
    s2script_core_dispatch_client_event("fullyconnect", s);
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientDisconnect(CPlayerSlot slot, ENetworkDisconnectionReason, const char*, uint64, const char*) {
    int s = slot.Get();
    s2script_core_dispatch_client_event("disconnect", s);   // dispatch FIRST — handler still sees valid
    if (s >= 0 && s < kMaxClientSlots) s_trackedSignon[s] = kSignonNone;
    if (s >= 0 && s < kMaxClientSlots) { s_voiceMuted[s] = 0; s_voiceLastNotify[s] = 0; }  // slot-reuse hygiene
    RETURN_META(MRES_IGNORED);
}
void S2ScriptPlugin::Hook_ClientSettingsChanged(CPlayerSlot slot) {
    s2script_core_dispatch_client_event("settingschanged", slot.Get());
    RETURN_META(MRES_IGNORED);
}

// Voice-control: ClientVoice fires per RECEIVED voice packet (tens/sec while a client talks — never
// for bots). Throttle per-slot to <=1 core dispatch per wall-clock second; the first packet of a
// transmission always dispatches, so a lazy mute-on-talk (the TTT PlayerMuter pattern) lands
// immediately. Notify-only (POST, MRES_IGNORED); the core side is the existing try_borrow_mut-guarded
// dispatch_client_event under the name "voice".
void S2ScriptPlugin::Hook_ClientVoice(CPlayerSlot slot) {
    int s = slot.Get();
    if (s >= 0 && s < kMaxClientSlots) {
        time_t now = time(nullptr);
        if (now != s_voiceLastNotify[s]) {
            s_voiceLastNotify[s] = now;
            s2script_core_dispatch_client_event("voice", s);
        }
    }
    RETURN_META(MRES_IGNORED);
}

// Voice-control: the enforcement hook (CSSharp voice_manager.cpp:60-63 shape). PRE hook; when the
// SENDER is muted and the game is about to store listen=true, swap the param to false with
// MRES_IGNORED + NEWPARAMS — the engine's own implementation still runs and stores our value. HOT
// PATH: plain array reads only. First fire performs the arg-sanity half of the doctrine validation
// (out-of-range slots = vtable drift -> named degrade, rewrite disabled) and logs once — that log
// line is also the live evidence for the engine's refresh cadence.
bool S2ScriptPlugin::Hook_SetClientListening(CPlayerSlot receiver, CPlayerSlot sender, bool bListen) {
    int r = receiver.Get(), s = sender.Get();
    if (!s_voiceListenSeen) {
        s_voiceListenSeen = true;
        if (r < -1 || r >= kMaxClientSlots || s < -1 || s >= kMaxClientSlots) {
            s_voiceListenDegraded = true;
            META_CONPRINTF("[s2script] VOICE VALIDATION FAILED: SetClientListening first fire has "
                           "out-of-range slots (r=%d s=%d) — vtable drift; voice mute DISABLED\n", r, s);
        } else {
            META_CONPRINTF("[s2script] voice: SetClientListening first fire (r=%d s=%d listen=%d)\n",
                           r, s, (int)bListen);
        }
    }
    if (!s_voiceListenDegraded && bListen && s >= 0 && s < kMaxClientSlots && s_voiceMuted[s]) {
        RETURN_META_VALUE_NEWPARAMS(MRES_IGNORED, bListen, &IVEngineServer2::SetClientListening,
                                    (receiver, sender, false));
    }
    RETURN_META_VALUE(MRES_IGNORED, bListen);
}

// POST StartupServer = the map is starting up on a live, named game server (CSSharp reads the map
// name in its POST hook the same way). Also doubles as the client-list slice's boot sanity line —
// a garbage GetIGameServer()/GetMapName()/GetMaxClients() vtable read would be visible here.
void S2ScriptPlugin::Hook_StartupServer(const GameSessionConfiguration_t&, ISource2WorldSession*, const char*) {
    INetworkGameServer* gs = S2_GameServer();
    const char* map = gs ? gs->GetMapName() : nullptr;
    META_CONPRINTF("[s2script] map start: %s (maxClients=%d)\n",
                   map ? map : "<null>", gs ? gs->GetMaxClients() : -1);
    // (Sound slice precache: no retry needed here — the OnPrecacheResource hook is a class-vtable swap
    // installed once at Load, since the class vtable is static data present from module load.)
    s2script_core_dispatch_map_start(map ? map : "");
    EnsureEntityListenerRegistered();   // re-assert the IEntityListener each map (idempotent Find-guard)
    RETURN_META(MRES_IGNORED);
}

// First-fire layout validation (re-strategy Rule 2 for call-context-only facts). Decides the
// semantics of the which-client int32 at s_ctiClientOff (slot vs entindex=slot+1 — CSSharp and
// Swiftly disagree) via EXCLUSIVE WITNESSES against s_trackedSignon (maintained by the client
// lifecycle hooks): each evidence info votes for exactly one interpretation only when the other
// is impossible; a mode wins only with >=1 exclusive witness and ZERO witnesses for the rival.
// Hard fail (-1) is reserved for genuine layout evidence — v far outside any client range
// ([0,128], double the slot count to absorb entindex skew) means the offset reads garbage.
// Odd-but-legitimate infos (null raw/bitvec pointer, worldspawn bit 0 clear — HLTV/replay or a
// mid-full-update client) are NON-EVIDENCE: skipped, never fatal, never a vote. Returns
// 1 = validated (mode cached), 0 = undecidable this snapshot (stay pending), -1 = hard mismatch.
static int TransmitValidateLayout(CCheckTransmitInfo** ppInfoList, int nInfoCount) {
    if (nInfoCount <= 0) return 0;
    int slotWitness = 0, entWitness = 0;
    for (int i = 0; i < nInfoCount; i++) {
        const uint8_t* raw = reinterpret_cast<const uint8_t*>(ppInfoList[i]);
        if (!raw) continue;                                     // non-evidence
        const CBitVec<16384>* bv = ppInfoList[i]->m_pTransmitEntity;
        if (!bv || !bv->IsBitSet(0)) continue;                  // non-evidence (HLTV/full-update?)
        int v = *reinterpret_cast<const int32_t*>(raw + s_ctiClientOff);
        if (v < 0 || v > 128) return -1;    // the ONLY hard fail: garbage far outside client range
        bool slotOk = (v < kMaxClientSlots && s_trackedSignon[v] != kSignonNone);
        bool entOk  = (v >= 1 && (v - 1) < kMaxClientSlots && s_trackedSignon[v - 1] != kSignonNone);
        if (slotOk && !entOk) slotWitness++;
        if (entOk && !slotOk) entWitness++;
    }
    if (slotWitness > 0 && entWitness == 0) { s_transmitClientIsEntIndex = false; return 1; }
    if (entWitness > 0 && slotWitness == 0) { s_transmitClientIsEntIndex = true;  return 1; }
    return 0;                               // no or conflicting exclusive witnesses -> retry
}

void S2ScriptPlugin::Hook_CheckTransmit(CCheckTransmitInfo** ppInfoList, int nInfoCount,
                                        CBitVec<16384>&, CBitVec<16384>&,
                                        const Entity2Networkable_t**, const uint16*, int) {
    s_transmitSnapshots++;
    if (!ppInfoList || nInfoCount <= 0) RETURN_META(MRES_IGNORED);
    if (s_transmitLayoutState == 0) {               // fail-closed gate: observe-only until validated
        // No tracked clients -> no witness data possible: stay pending WITHOUT burning the attempt
        // budget (a late-loaded shim whose lifecycle hooks missed the connects would otherwise
        // false-FAIL through 512 undecidable snapshots; a fresh connect unblocks validation).
        bool anyTracked = false;
        for (int i = 0; i < kMaxClientSlots; i++)
            if (s_trackedSignon[i] != kSignonNone) { anyTracked = true; break; }
        if (!anyTracked) RETURN_META(MRES_IGNORED);
        int r = TransmitValidateLayout(ppInfoList, nInfoCount);
        if (r == 1) {
            s_transmitLayoutState = 1;
            META_CONPRINTF("[s2script] transmit: CheckTransmitInfo layout VALIDATED (client int @%d = %s)\n",
                           s_ctiClientOff, s_transmitClientIsEntIndex ? "entindex (slot+1)" : "slot");
        } else if (r == -1 || ++s_transmitValidateAttempts >= kTransmitValidateMaxAttempts) {
            s_transmitLayoutState = -1;
            s_transmitTable.clear();
            META_CONPRINTF("[s2script]   gamedata FAIL  CheckTransmitInfo_clientEntityIndex — first-fire "
                           "validation %s (offset %d wrong for this build? re-derive; see "
                           "docs/re-strategy.md); transmit filtering DISABLED\n",
                           (r == -1) ? "MISMATCH" : "UNDECIDABLE", s_ctiClientOff);
        }
        RETURN_META(MRES_IGNORED);                  // never mutate on the validating snapshot
    }
    if (s_transmitLayoutState != 1 || s_transmitTable.empty()) RETURN_META(MRES_IGNORED);

    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    // Entries outer, infos inner: serial-gate each entry ONCE per snapshot (a single lookup — no
    // TOCTOU window inside the snapshot), then apply to every client's bitvec. A failed resolve
    // means the entity is gone FOREVER (serials never come back), so the entry is evicted —
    // the table is self-cleaning across deaths and map changes.
    for (auto it = s_transmitTable.begin(); it != s_transmitTable.end(); ) {
        if (it->first < 0 || it->first >= MAX_EDICTS) { it = s_transmitTable.erase(it); continue; }
        if (!ResolveEntityBySerial(it->first, it->second.serial)) {
            it = s_transmitTable.erase(it);
            continue;
        }
        const int      entIndex = it->first;
        const uint64_t mask     = it->second.mask;
        for (int i = 0; i < nInfoCount; i++) {
            uint8_t* raw = reinterpret_cast<uint8_t*>(ppInfoList[i]);
            if (!raw) continue;
            int v = *reinterpret_cast<const int32_t*>(raw + s_ctiClientOff);
            int slot = s_transmitClientIsEntIndex ? (v - 1) : v;
            if (slot < 0 || slot >= 64) continue;
            if ((mask >> slot) & 1ull) continue;    // visible to this viewer — leave the bit alone
            CBitVec<16384>* bv = ppInfoList[i]->m_pTransmitEntity;
            if (bv && bv->IsBitSet(entIndex)) { bv->Clear(entIndex); s_transmitBitsCleared++; }
        }
        ++it;
    }
    clock_gettime(CLOCK_MONOTONIC, &t1);
    uint64_t ns = (uint64_t)(t1.tv_sec - t0.tv_sec) * 1000000000ull
                + (uint64_t)t1.tv_nsec - (uint64_t)t0.tv_nsec;
    s_transmitNsLast = ns;
    if (ns > s_transmitNsMax) s_transmitNsMax = ns;
    RETURN_META(MRES_IGNORED);
}

// (Sound slice precache: the hook handler + installer are FREE functions — Detour_OnPrecacheResource /
// WriteVtableSlot / InstallPrecacheHook — defined up with the precache statics block, because this is
// a class-vtable slot swap, not a member SourceHook. See that block for the mechanism rationale.)

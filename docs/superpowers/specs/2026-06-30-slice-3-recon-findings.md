# Slice 3 — Engine-RE reconnaissance findings

**Task:** Pin the exact hl2sdk `cs2` APIs the Slice-3 native tasks (Tasks 3–6) will call, so
the engine glue is built on cited headers, not guesses. Companion to
`2026-06-30-slice-3-schema-typed-accessor-design.md` §11 (open items).

**Method:** static read of the vendored headers under `third_party/hl2sdk/public` +
`third_party/metamod-source`, plus a throwaway compile probe (`-fsyntax-only`,
`rust:bullseye`) proving every symbol below exists and the headers include together.
The probe printed **`PROBE_OK`**.

**Engine-generic constraint:** every mechanism below is a *Source 2* mechanism — it would
work on another Source 2 game. The CS2 names `CCSPlayerPawn`, `m_iHealth`, `m_hPlayerPawn`,
`libserver.so` appear **only as example string inputs** a later `@s2script/cs2` JS layer
passes in; none are baked into a core call.

All citations are `path:line` relative to repo root. Header paths are under
`third_party/hl2sdk/public/` unless noted.

---

## Confidence legend

- **[HC] header-confirmed** — the type/signature/field is present in a vendored header at the
  cited line; the call compiles (proven by the probe).
- **[LC] needs live confirmation** — the *symbol shape* is header-confirmed, but a runtime
  fact (a factory actually resolving an interface, a non-inline member being exported/linkable,
  an offset value, an index convention, a global's address) can only be verified against the
  live CS2 process in the Task-7 gate.

---

## Q1 — SchemaSystem offset resolve `(class, field) → offset`

**Type(s):** `ISchemaSystem`, `CSchemaSystemTypeScope` (impl of `ISchemaSystemTypeScope`),
`CSchemaClassInfo` (: `SchemaClassInfoData_t`), `SchemaClassFieldData_t`,
`SchemaMetaInfoHandle_t<T>`.

**Call sequence (each step [HC]):**

1. **Type scope for the server module.**
   `ISchemaSystem::FindTypeScopeForModule( const char* pszModuleName, const char** ppszBindingName = NULL ) → CSchemaSystemTypeScope*`
   — `schemasystem/schemasystem.h:119`.
   Example input: `"libserver.so"` (CS2 Linux server module). The module *string* is a
   gamedata/live-confirm item (see [LC] below), not a core constant.
   (`GlobalTypeScope()` at `schemasystem.h:117` is the fallback scope if per-module lookup
   returns null.)

2. **Resolve the class by name.**
   `ISchemaSystemTypeScope::FindDeclaredClass( const char* pszClassName ) → SchemaMetaInfoHandle_t<CSchemaClassInfo>`
   — `schemasystem.h:43`. Unwrap with `.Get()` (`schematypes.h:160`) → `CSchemaClassInfo*`.
   Example input: `"CCSPlayerPawn"`.
   **Direct-pointer alternative** (no handle wrapper):
   `ISchemaSystemTypeScope::FindRawClassBinding( const char* pszClassName ) → CSchemaClassInfo*`
   — `schemasystem.h:70`.

3. **Iterate fields, match name, read offset.**
   `CSchemaClassInfo : SchemaClassInfoData_t` — `schematypes.h:378`. Relevant members of
   `SchemaClassInfoData_t` (`schematypes.h:345`):
   - `SchemaClassFieldData_t* m_pFields` — `schematypes.h:364`
   - `uint16 m_nFieldCount` — `schematypes.h:355`
   - `int m_nSize` — `schematypes.h:353` (class size, useful sanity check)

   Per field `SchemaClassFieldData_t` (`schematypes.h:315`):
   - `const char* m_pszName` — `schematypes.h:317` (match against `"m_iHealth"`)
   - `CSchemaType* m_pType` — `schematypes.h:319` (type/size check; for an `int32` field this
     is a `CSchemaType_Builtin` with `SCHEMA_BUILTIN_TYPE_INT32`)
   - **`int m_nSingleInheritanceOffset` — `schematypes.h:321` ← THE OFFSET GETTER.**
     It is a plain struct field, not a method; read it directly. This is the flattened
     single-inheritance byte offset of the field within the class instance.

**Exact getter for the offset:** `field.m_nSingleInheritanceOffset` (`schematypes.h:321`).

**Tag:** signature/layout **[HC]**. **[LC]:** (a) the *module string* `"libserver.so"` that
`FindTypeScopeForModule` expects — becomes a gamedata key if it varies; (b) the *resolved
offset value* for any concrete `(class, field)` — a runtime dump target; (c) that the
type-scope/class actually resolve non-null at the moment core queries (schema must be loaded).

---

## Q2 — SchemaSystem acquisition

**Interface string:** `SCHEMASYSTEM_INTERFACE_VERSION == "SchemaSystem_001"` —
`schemasystem.h:112`. Matches `gamedata/core.gamedata.jsonc` key `"SchemaSystem"`.

**Available factories (Metamod `ISmmAPI`):** only `GetEngineFactory`
(`third_party/metamod-source/core/ISmmAPI.h:104`), `GetPhysicsFactory` (:113),
`GetFileSystemFactory` (:122), `GetServerFactory` (:131). **There is no dedicated
`schemasystem` factory in `ISmmAPI`.** `VInterfaceMatch(CreateInterfaceFn, const char*, int)`
(`ISmmAPI.h:299`, impl `metamod.cpp:726`) and `MetaFactory` (`ISmmAPI.h:217`,
`metamod.cpp:821`) are the only generic interface finders; `MetaFactory` only serves
Metamod's own interfaces (SourceHook / plugin manager), not SchemaSystem.

**Recommended shim shape — reuse the existing engine-factory path** already used for
`EngineCvar` / `NetworkServerService` in `shim/src/s2script_mm.cpp:129-130`:

```cpp
// in Load(), alongside the existing tryGet(...) calls (s2script_mm.cpp:114-131)
int ret = 0;
auto it = versions.find("SchemaSystem");                 // "SchemaSystem_001" from gamedata
const char* verStr = (it != versions.end()) ? it->second.c_str()
                                             : SCHEMASYSTEM_INTERFACE_VERSION;
ISchemaSystem* pSchema = reinterpret_cast<ISchemaSystem*>(engineFactory(verStr, &ret));
// store pSchema for the core to query; degrade-never-crash if null (spec §7)
```

`engineFactory` is `ismm->GetEngineFactory(false)` (already fetched at
`s2script_mm.cpp:96`). Equivalent robust form: `ismm->VInterfaceMatch(engineFactory,
verStr)`. This replaces the current deferral note at `s2script_mm.cpp:131`.

**Tag:** the factory *call shape* and the version string are **[HC]**. **[LC] (primary risk):**
whether the **engine factory actually resolves `SchemaSystem_001`** — SchemaSystem lives in a
separate Source 2 module (`libschemasystem.so`), and `CreateInterface` normally only serves
interfaces registered in the module that owns the factory. The CS2 Metamod community pattern
gets SchemaSystem via the engine factory and it works in practice, but it must be verified live.
**Fallback if the engine factory returns null:** `dlopen`/`dlsym` the `libschemasystem.so`
module's own `CreateInterface` and call it directly (a module lookup keyed by the module
filename — that filename would be a gamedata string). Store the resolved `ISchemaSystem*` for
core to query either way.

---

## Q3 — Entity system + entity-by-index

**Type(s):** `CGameEntitySystem` (: `CEntitySystem` : `IEntityResourceManifestBuilder`),
`CEntityIdentity`, `CEntityInstance`, `CConcreteEntityList`, `CEntityIndex`,
`IGameResourceService`.

### Obtaining `CGameEntitySystem*`

- **Declared accessor:** `extern CGameEntitySystem* GameEntitySystem();` —
  `entity2/entitysystem.h:43`. **[HC]** as a declaration. **[LC]:** it is *implemented in the
  game module*; whether the symbol is **exported and linkable** from a Metamod plugin `.so` is
  unverified — historically it is **not** reliably linkable, so do not depend on it.

- **Recommended anchor (interface + small offset):** acquire
  `IGameResourceService` via the engine factory using
  `GAMERESOURCESERVICESERVER_INTERFACE_VERSION == "GameResourceServiceServerV001"`
  — `interfaces/interfaces.h:558-559` (global `g_pGameResourceServiceServer`). The
  `CGameEntitySystem*` lives at a fixed byte offset inside that service object:
  `pEntSys = *reinterpret_cast<CGameEntitySystem**>((uintptr_t)pGameResourceService + OFFSET);`

  **This OFFSET is the one gamedata value Q3 needs.** Shape for `gamedata/core.gamedata.jsonc`:
  an `offsets`/`GameEntitySystem` entry, `{ "linuxsteamrt64": <int> }` (community value ≈ `0x58`
  Windows / `0x50` Linux — **[LC], must be dumped live**). Prefer this interface-anchored offset
  over a raw code signature: it is smaller and more update-stable.

  A whole-function **signature scan** of `libserver.so` for the routine that returns the global
  is a valid alternative shape (`{ "name": "GameEntitySystem", "module": "server",
  "signature": "<bytes>" }`) but is more brittle; use only if the offset anchor fails.

### `CEntityInstance*` by index

- **Clean accessor:** `CEntitySystem::GetEntityInstance( CEntityIndex entnum ) → CEntityInstance*`
  — inline, `entity2/entitysystem.h:285`. It calls
  `GetEntityInstance( GetEntityIdentity( entnum ) )` where
  `CEntityIdentity* GetEntityIdentity( CEntityIndex )` is **non-inline** (declared
  `entitysystem.h:281`, implemented in the game module). `CEntityIndex` is constructed from an
  `int` (`entity2/entityidentity.h:34`). **[LC]:** linking `GetEntityIdentity` from the plugin
  depends on it being exported.

- **Signature-free fallback (manual chunk walk, uses only [HC] layout):**
  `CEntitySystem::m_EntityList` is a `CConcreteEntityList` — `entitysystem.h:312`.
  `CConcreteEntityList::m_pIdentityChunks[MAX_ENTITY_LISTS]` — `concreteentitylist.h:15`.
  Constants: `MAX_ENTITIES_IN_LIST 512`, `MAX_ENTITY_LISTS 64` — `entityidentity.h:8-9`.
  So for entity index `i`: `chunk = i / 512`, `slot = i % 512`,
  `CEntityIdentity* id = &m_pIdentityChunks[chunk][slot];` then
  `CEntityInstance* e = id->m_pInstance;` (`entityidentity.h:101`). Null/invalid checks via
  `id->m_flags & EF_IS_INVALID_EHANDLE` and matching handle serial. This path needs **no
  gamedata signature** beyond the `CGameEntitySystem*` pointer itself.

**Is a gamedata signature required?** For the **entity-system global**: yes — an
interface-anchored **offset** (recommended) or a code **signature** (fallback), specified above.
For **entity-by-index** given the pointer: no — the manual chunk walk is signature-free; the
inline `GetEntityInstance` is a linkability [LC].

**Tag:** all struct layouts, constants, and inline accessors **[HC]** (compiled in probe).
**[LC]:** the entity-system global offset value; whether `GetEntityIdentity` is exported.

---

## Q4 — Handle deref (u32 → `CEntityInstance*`, null when stale)

**Type(s):** `CEntityHandle` (== `CBaseHandle`, typedef `entityhandle.h:164`), `CEntitySystem`.

- Build a handle from the raw u32 read out of a schema handle field:
  `CEntityHandle( uint32 value )` — `entityhandle.h:24`, inline impl `entityhandle.h:80-83`
  (`m_Index = value`). The u32 packs `m_EntityIndex : 15` / `m_Serial : 17`
  (`entityhandle.h:59-67`).
- Dereference through the entity system (validates serial → **null on stale**):
  `CEntitySystem::GetEntityInstance( const CEntityHandle& hEnt ) → CEntityInstance*`
  — inline, `entitysystem.h:286`; internally `GetEntityIdentity(const CEntityHandle&)`
  (`entitysystem.h:282`) returns null when the stored identity's serial ≠ the handle's serial,
  and `GetEntityInstance(nullptr)` returns null (`entitysystem.h:284`).

**Do NOT use `CEntityHandle::Get()`** (`entityhandle.h:56`) — the header comments it is
"implemented in game code (ehandle.h)" and is not vendored/linkable. Route deref through the
entity system.

**Tag:** ctor + accessor **[HC]** (compiled). **[LC]:** the *stale → null* behaviour lives in
the game-module `GetEntityIdentity` body; confirm live that a stale handle yields null (it is
the documented Source 2 semantic, but unverified in-process here).

---

## Q5 — `slot → controller → pawn`

**Type(s):** `CPlayerSlot` (`playerslot.h:10`), `CEntityIndex` (`entityidentity.h:31`); the
controller/pawn *classes* (`CBasePlayerController`, `CCSPlayerPawn`) are **CS2-specific and
deliberately absent from core** — the JS `@s2script/cs2` layer supplies the class name and the
pawn-handle field name as string inputs.

**Generic mechanism:**
1. A client slot is a `CPlayerSlot` (`playerslot.h:10-24`; `.Get()` at :17,
   `ABSOLUTE_PLAYER_LIMIT == 64` at `const.h:36`).
2. **Controller entity index convention:** controller index `== slot + 1`
   (entity index 0 is the worldspawn/null slot; player controllers occupy `1..maxplayers`).
   Resolve the controller `CEntityInstance*` with the Q3 entity-by-index path on
   `CEntityIndex(slot.Get() + 1)`.
3. **Pawn handle:** read the controller's pawn-handle field (example input `"m_hPlayerPawn"`)
   as a `uint32` via the **Q1** schema-offset path, then deref via **Q4**
   (`CEntitySystem::GetEntityInstance(CEntityHandle)`).

**Tag:** `CPlayerSlot`/`CEntityIndex` types **[HC]**. **[LC] (primary):** the **`slot + 1`
controller-index convention** — must be confirmed live (alternative is a player-manager lookup;
the `slot+1` convention is the standard CS2 mapping but is the key runtime assumption here). The
field name `m_hPlayerPawn` and its resolved offset are Q1 [LC] items.

---

## Q6 — `NetworkStateChanged` (mark field dirty for clients)

**Type(s):** `CEntityInstance`, `NetworkStateChangedData`.

- **The call:**
  `virtual void CEntityInstance::NetworkStateChanged( const NetworkStateChangedData& data )`
  — `entity2/entityinstance.h:113`. It is a **virtual** on `CEntityInstance`, so it is called
  through the vtable of the live entity — no separate export/signature needed.
- **Argument construction:** `NetworkStateChangedData` (`entityinstance.h:23`) has a
  non-explicit constructor
  `NetworkStateChangedData( uint32 nLocalOffset, int32 nArrayIndex = -1,
  ChangeAccessorFieldPathIndex_t nPathIndex = ChangeAccessorFieldPathIndex_t() )`
  — `entityinstance.h:40-44`. For a plain top-level `int32` field, pass the field's flattened
  offset (the Q1 `m_nSingleInheritanceOffset`) with the defaults:

  ```cpp
  pEntity->NetworkStateChanged( NetworkStateChangedData( (uint32)offset ) );
  ```

  `nArrayIndex = -1` (not a `CNetworkUtlVectorBase`), `nPathIndex` default `-1` (no pointer
  chain) per the header comments (`entityinstance.h:36-39`, :55-63). Because the constructor is
  non-explicit, `pEntity->NetworkStateChanged(offset)` also implicitly converts, but construct
  explicitly for clarity.

**Chain-call alternative:** `networkvar.h` (included, compiled) provides the
`CNetworkVarBase`/`CNetworkVarChainer` machinery that ultimately funnels into the same
`CEntityInstance::NetworkStateChanged`; for the Slice-3 raw-offset write, calling the virtual
directly with the offset is the minimal path and avoids needing the field's C++ networkvar type.

**Tag:** virtual + arg struct + ctor **[HC]** (compiled). **[LC]:** that passing *only* the
single flattened offset (no path index) correctly dirties a top-level field for transmit —
verify live by observing the value replicate to a client after a write.

---

## Q7 — ConCommand registration + calling client slot

**Type(s):** `ConCommand` (: `ConCommandRef`), `ConCommandCallbackInfo_t`, `FnCommandCallback_t`,
`CCommandContext`, `CCommand`, `CPlayerSlot`. All in `tier1/convar.h`.

> Naming note: the brief mentions `ConCommandRefAbstract`; the vendored header calls these
> `ConCommandRef` (`convar.h:530`) and `ConCommand` (`convar.h:583`). There is no
> `ConCommandRefAbstract` — use `ConCommand`.

- **Callback type:**
  `typedef void (*FnCommandCallback_t)( const CCommandContext& context, const CCommand& command );`
  — `convar.h:242`.
- **Registration:** construct a `ConCommand` (`convar.h:583`); its constructor
  `ConCommand( const char* pName, ConCommandCallbackInfo_t callback, const char* pHelpString,
  uint64 flags = 0, CompletionCallbackInfo_t = {} )` — `convar.h:588` — calls the internal
  `Create()` (`convar.h:602`), which self-registers into the cvar system. Wrap the function
  pointer in `ConCommandCallbackInfo_t` (implicit ctor from `FnCommandCallback_t` at
  `convar.h:388`). Typical usage is a file-scope/static `ConCommand` object, or a heap-allocated
  one owned by the plugin (destructor `~ConCommand()` calls `Destroy()`, `convar.h:596-599` —
  ledger this for teardown).
- **Calling client slot:** inside the callback,
  `CCommandContext::GetPlayerSlot() → CPlayerSlot` — `convar.h:289` (context ctor stores
  `CommandTarget_t` + `CPlayerSlot`, `convar.h:280-296`). `CPlayerSlot::Get()` → `int`
  (`playerslot.h:17`); a server-console invocation surfaces as an invalid/`-1` slot
  (`CPlayerSlot::IsValid()`, `playerslot.h:16`).
- **Args:** `CCommand` (`convar.h:299`): `ArgC()` (:307/:344), `Arg(int)`/`operator[]`
  (:312/:364), `ArgS()` (:309/:354).

**Tag:** all types/signatures **[HC]** (probe builds a `FnCommandCallback_t` lambda that reads
`ctx.GetPlayerSlot()` and `args.ArgC()/Arg(0)`, and constructs `ConCommandCallbackInfo_t`).
**[LC]:** that the `ConCommand` self-registration is *effective* from within the Metamod plugin
— it depends on the cvar system global (`g_pCVar`/`ICvar`, `EngineCvar`/`VEngineCvar007` already
in gamedata and acquired at `s2script_mm.cpp:129`) being initialized in the module at
construction time; confirm the command appears and the callback fires with the correct slot
live. (Metamod also exposes `ISmmAPI::RegisterConCommandBase`, `ISmmAPI.h:149`, but that is the
legacy `ConCommandBase` path; the Source 2 native path is the `ConCommand` ctor above.)

---

## Probe — include set + stub (reuse in Tasks 3–5)

**File:** `/tmp/s2recon/probe.cpp` (throwaway; deleted before commit). Includes all nine headers
above and references the key type/method of every finding; compiled `-fsyntax-only`.

**Result:** `PROBE_OK`.

**Exact compile command (inside `rust:bullseye`):**

```bash
docker run --rm -v "$(pwd):/repo" -v /tmp/s2recon:/tmp/s2recon -w /repo rust:bullseye bash -lc \
  'apt-get update >/dev/null && apt-get install -y g++ >/dev/null; \
   g++ -std=c++17 -fsyntax-only \
     -I third_party/hl2sdk/public \
     -I third_party/hl2sdk/public/tier0 \
     -I third_party/hl2sdk/public/tier1 \
     -I third_party/hl2sdk/public/mathlib \
     -I third_party/hl2sdk/public/appframework \
     -I shim/src/sdk_stubs \
     -I <stub-dir-for-entitydatainstantiator.h> \
     /tmp/s2recon/probe.cpp && echo PROBE_OK'
```

**Include paths that made it compile (superset of the brief's starter; mirrors
`shim/CMakeLists.txt:26-30`):**

| `-I` path | why |
|---|---|
| `third_party/hl2sdk/public` | root for `schemasystem/…`, `entity2/…`, `interfaces/…`, `entityhandle.h`, `playerslot.h`, `networkvar.h` |
| `third_party/hl2sdk/public/tier0` | **added** — `platform.h` (pulled by `tier1/generichash.h`) lives here |
| `third_party/hl2sdk/public/tier1` | `convar.h`, `utl*` |
| `third_party/hl2sdk/public/mathlib` | `vector4d.h` etc. (pulled by `convar.h`) |
| `third_party/hl2sdk/public/appframework` | `IAppSystem.h` (base of `ISchemaSystem`) |
| `shim/src/sdk_stubs` | existing `network_connection.pb.h` stub (`eiface.h` dep, pulled by `entitysystem.h`) |
| **stub dir for `entitydatainstantiator.h`** | see gap below |

**Platform defines** are set at the top of the probe `.cpp` (mirroring
`shim/CMakeLists.txt:38-50`): `LINUX`, `_LINUX`, `POSIX`, `COMPILER_GCC`, `PLATFORM_64BITS`,
`_FILE_OFFSET_BITS=64`, `_GLIBCXX_USE_CXX11_ABI=0`, `stricmp=strcasecmp` etc. Tasks 3–5 already
inherit these from the shim build; a standalone compile must supply them.

### Vendored-SDK gap — action item for Tasks 3–5

`entity2/entitysystem.h:23` does `#include "entitydatainstantiator.h"`, but **that header does
not exist anywhere under `third_party/hl2sdk/public`** (only `entity2/{concreteentitylist,
entityclass, entitycomponent, entityidentity, entityinstance, entitykeyvalues, entitysystem}.h`
are present). `IEntityDataInstantiator` is used solely as a pointer
(`IEntityDataInstantiator* m_Accessors[MAX_ACCESSORS]`, `entitysystem.h:356`), so a **one-line
forward-declare stub** satisfies it. The probe used a throwaway stub; **the real fix is to add
`shim/src/sdk_stubs/entitydatainstantiator.h`:**

```cpp
#ifndef ENTITYDATAINSTANTIATOR_H
#define ENTITYDATAINSTANTIATOR_H
class IEntityDataInstantiator;   // pointer-only use in entity2/entitysystem.h
#endif
```

Any Task-3/4/5 translation unit that includes `entity2/entitysystem.h` needs this stub on the
include path (or the vendored SDK backfilled). Flag for the maintenance-treadmill tooling.

---

## Step 3 — Live-confirmation risk register (Task-7 gate targets)

Ordered by risk. Everything else in this doc is header-confirmed and compiles.

1. **[LC] SchemaSystem acquisition path (Q2)** — does `GetEngineFactory` resolve
   `SchemaSystem_001`? If null → fall back to a `libschemasystem.so` module `CreateInterface`
   lookup. *Highest risk: gates everything downstream.*
2. **[LC] Entity-system global (Q3)** — the `IGameResourceService`-anchored **offset** to
   `CGameEntitySystem*` (recommended) — dump the value live; add to `gamedata/`. Confirms the
   entity-by-index and handle-deref paths have a valid `pEntSys`.
3. **[LC] Resolved offset value (Q1)** — the concrete `m_nSingleInheritanceOffset` for the
   example `(CCSPlayerPawn, m_iHealth)`; verify the schema is loaded and the class/field resolve
   non-null at query time. (The *mechanism* is confirmed; the *number* is runtime.)
4. **[LC] `slot + 1` controller-index convention (Q5)** — confirm the controller entity sits at
   `CEntityIndex(slot+1)`; else use a player-manager lookup.
5. **[LC] `NetworkStateChanged(offset)` effectiveness (Q6)** — confirm a single-offset call
   dirties the field and the new value replicates to a connected client.
6. **[LC] Stale-handle → null (Q4)** — confirm `GetEntityInstance(CEntityHandle)` returns null
   for a handle whose serial no longer matches.
7. **[LC] ConCommand self-registration + slot (Q7)** — confirm the command registers from the
   plugin module and the callback reports the correct calling `CPlayerSlot`.
8. **[LC] `GetEntityIdentity` linkability (Q3)** — if the inline `GetEntityInstance(index)` fails
   to link, use the signature-free manual chunk walk (already speced). Low risk (fallback exists).
9. **[build] `entitydatainstantiator.h` stub** — must be added to `shim/src/sdk_stubs/` for
   Tasks 3–5 to compile `entity2/entitysystem.h`. Deterministic, not a runtime risk.

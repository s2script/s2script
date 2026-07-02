# Slice 5B.1 Spike — SDK Schema Enumeration Findings

**Status: GO.** The live `SchemaSystem` can be fully enumerated (all declared classes → each
class's fields → name/offset/type) from the shim using only the typed hl2sdk headers already
included, with no tier0/tier1 linkage and no raw member-offset hacks. The `CSchemaType` category →
stable `kind` string mapping is confirmed live. This doc is the recipe the implementation tasks
(5B.1 Task 3's `schema_enumerate`) transcribe verbatim.

- **Date:** 2026-07-01
- **Method:** throwaway scratch enumeration + logging added to `shim/src/s2script_mm.cpp`, built
  via `scripts/build-sniper.sh` (`rust:bullseye`, GLIBC ≤ 2.31), run live on Docker CS2
  (`de_inferno`, bots armed, past the boot window). Scratch removed; the only committed artifact is
  this doc. Confirmed `bash scripts/build-sniper.sh` builds clean after removal.
- **hl2sdk headers used:** `public/schemasystem/schemasystem.h`, `public/schemasystem/schematypes.h`,
  `public/tier1/utltshash.h`, `public/tier1/utlstring.h`.

---

## Step 1 — Iterating a type scope's declared classes (CONFIRMED, SDK API)

`CSchemaSystemTypeScope` (schemasystem.h:86) exposes its declared-class table as a **public member**:

```cpp
CUtlTSHash<CSchemaClassInfo*, 256, uint> m_ClassBindings;   // schemasystem.h:108
```

`ISchemaSystemTypeScope` has **no** class-iteration virtual (only `FindDeclaredClass` /
`FindRawClassBinding` by name), so we iterate `m_ClassBindings` directly. It is a public field, and
the concrete `CSchemaSystemTypeScope*` is already returned by `ISchemaSystem::FindTypeScopeForModule`
/ `GlobalTypeScope` (both non-abstract return types) — **no raw offset needed.**

`CUtlTSHash` (utltshash.h) provides a fully-inline element/handle iteration API:

```cpp
int  Count() const;                                             // # elements
int  GetElements(int nFirst, int nCount, UtlTSHashHandle_t* out) const;  // fills handle array
T&   Element(UtlTSHashHandle_t h);                             // T = CSchemaClassInfo*
```

**Confirmed recipe** (this is exactly what Task 3's `schema_enumerate` does):

```cpp
CSchemaSystemTypeScope* scope = s_pSchemaSystem->FindTypeScopeForModule("libserver.so");
if (!scope) scope = s_pSchemaSystem->GlobalTypeScope();
int n = scope->m_ClassBindings.Count();
std::vector<UtlTSHashHandle_t> handles((size_t)n);
int got = scope->m_ClassBindings.GetElements(0, n, handles.data());
for (int i = 0; i < got; ++i) {
    CSchemaClassInfo* ci = scope->m_ClassBindings.Element(handles[i]);
    // ci->m_pszName, ci->m_pFields, ci->m_pBaseClasses, ...
}
```

**Linkage note (important):** `Count`/`GetElements`/`Element` are inline templates whose only
dependencies are `CUtlMemoryPoolBase::Count()` (inline: `return m_BlocksAllocated;`) and
`CThreadSpinRWLock::LockForRead/UnlockRead` (inline over `std::shared_mutex`). So they compile into
the shim with **no tier0/tier1 link** — the shim's existing "links only `libs2script_core.so`" setup
is unchanged. Verified: the resulting `s2script.so` needs only `GLIBC_2.14` (no new undefined
symbols).

**Live evidence:** `m_ClassBindings.Count() == 746` for the `libserver.so` scope; `GetElements`
returned all 746 handles; `Element(h)` yielded valid `CSchemaClassInfo*` (e.g. `CWeaponNOVA`,
`CPointWorldText`, `CAmbientGeneric`, `CEnvEntityMaker`, `CPointEntity`, `CFilterEnemy`, …). A full
catalog is feasible.

> **Parent-chain completeness (recommendation for Task 3):** the 746 are the classes *declared in
> the `libserver.so` module scope*. A parent (via `m_pBaseClasses[b].m_pClass`) is reachable by
> pointer regardless of scope, but a base class registered in a *different* scope may not itself
> appear in the 746, which would leave a dangling `parent` name in the catalog. Task 3 should
> guarantee the parent chain resolves — either (a) also enumerate `GlobalTypeScope()` and union, or
> (b) additionally emit any base class reached via `m_pBaseClasses` that wasn't already emitted.
> Cheap and robust; decide during Task 3. (All spike validation targets — `CBaseEntity` fields,
> `CCSPlayerPawnBase` — resolved fine via base pointers, so this is a completeness nicety, not a
> blocker.)

---

## Step 2 — Class / field / base access (CONFIRMED)

`CSchemaClassInfo : SchemaClassInfoData_t` (schematypes.h:345, 378). Members used:

| Member | Type | Use |
|---|---|---|
| `m_pszName` | `const char*` | class name |
| `m_nSize` | `int` | instance size (informational) |
| `m_nFieldCount` | `uint16` | own field count |
| `m_pFields` | `SchemaClassFieldData_t*` | own fields array |
| `m_nBaseClassCount` | `uint8` | base count |
| `m_pBaseClasses` | `SchemaBaseClassInfoData_t*` | base array |

`SchemaBaseClassInfoData_t` (schematypes.h:339): `{ uint m_nOffset; CSchemaClassInfo* m_pClass; }`.
Parent name = `info->m_pBaseClasses[0].m_pClass->m_pszName` when `m_nBaseClassCount > 0`. CS2 game
classes are single-inheritance (all sampled classes had `bases=1`, base offset 0), so the existing
`schema_find_field` flattening (`base.m_nOffset + recurse`) is correct.

`SchemaClassFieldData_t` (schematypes.h:315) — **the field struct**:

```cpp
struct SchemaClassFieldData_t {
    const char*  m_pszName;                 // field name
    CSchemaType* m_pType;                    // field type  ← NOTE: named m_pType, NOT m_pSchemaType
    int          m_nSingleInheritanceOffset; // byte offset within its declaring class
    int          m_nStaticMetadataCount;
    SchemaMetadataEntryData_t* m_pStaticMetadata;
};
```

> **Correction for the brief/plan:** the member is **`m_pType`**, not `m_pSchemaType`. Task 3's
> skeleton comment (`schema_type_to_kind(f.m_pSchemaType, ...)`) must read `f.m_pType`.

**Live evidence (CCSPlayerPawn):** `parent=CCSPlayerPawnBase`, `ownFields=105`, `bases=1`,
`size=5664`. Sample `(field, offset)`: `m_pBulletServices@4056`, `m_nCharacterDefIndex@4112`,
`m_bHasFemaleVoice@4114`, `m_bInBuyZone@4369`. Base-walk works: `m_iHealth` (declared on
`CBaseEntity`) resolved via recursion.

---

## Step 3 — `CSchemaType` category → `kind` mapping (CONFIRMED)

`CSchemaType` (schematypes.h:174) is polymorphic; the discriminators are the **last two members**
(after vtable, `CUtlString m_sTypeName`, `CSchemaSystemTypeScope* m_pTypeScope`):

```cpp
SchemaTypeCategory_t  m_eTypeCategory;   // schematypes.h:216
SchemaAtomicCategory_t m_eAtomicCategory; // schematypes.h:217
CUtlString            m_sTypeName;        // full type name; read via inline .Get()
```

`SchemaTypeCategory_t` (schematypes.h:64) values (live-confirmed via logged `cat=`):

| enum | value | `kind` | inner / name source |
|---|---|---|---|
| `SCHEMA_TYPE_BUILTIN` | 0 | `atomic` | `name` = `m_sTypeName.Get()` (e.g. `int32`, `bool`, `uint16`, `float32`) |
| `SCHEMA_TYPE_POINTER` | 1 | `ptr` | `inner` = `((CSchemaType_Ptr*)t)->m_pObjectType->m_sTypeName.Get()` |
| `SCHEMA_TYPE_BITFIELD` | 2 | `unknown` | `name` = `m_sTypeName.Get()` (not seen on sampled classes) |
| `SCHEMA_TYPE_FIXED_ARRAY` | 3 | `unknown` | `name` = `m_sTypeName.Get()` (e.g. `char[18]`) |
| `SCHEMA_TYPE_ATOMIC` | 4 | `handle` **or** `atomic` | see below |
| `SCHEMA_TYPE_DECLARED_CLASS` | 5 | `class` | `name` = `m_sTypeName.Get()` |
| `SCHEMA_TYPE_DECLARED_ENUM` | 6 | `enum` | `name` = `m_sTypeName.Get()` |
| `SCHEMA_TYPE_INVALID` | 7 | `unknown` | — |

### The CHandle special case (KEY FINDING)

A `CHandle<T>` field surfaces as **`SCHEMA_TYPE_ATOMIC` (cat=4) + `SCHEMA_ATOMIC_T` (atom=1)** whose
`m_sTypeName` begins with `"CHandle"` (e.g. `"CHandle< CCSPlayerPawn >"`). The inner class is the
template type: `((CSchemaType_Atomic_T*)t)->m_pTemplateType->m_sTypeName.Get()` → `"CCSPlayerPawn"`.

> **CRITICAL:** `CSchemaType_Atomic::m_pAtomicInfo` is **NULL live for CHandle** (logged
> `atomicName=-`). So do **NOT** detect CHandle via `m_pAtomicInfo->m_pszName == "CHandle"` (that was
> the intuitive first guess and it silently fails → everything maps to `atomic`). **Detect by the
> `m_sTypeName` prefix `"CHandle"`** instead. Other atomics (`CUtlString`, `CUtlVector<...>`) keep
> `kind=atomic`.

### Confirmed category switch (transcribe into Task 3's `schema_type_to_kind`)

```cpp
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
            // CHandle<T>: atom==T and type name starts with "CHandle". m_pAtomicInfo is NULL — do
            // not rely on it. Inner class = m_pTemplateType.
            if (t->m_eAtomicCategory == SCHEMA_ATOMIC_T && full && strncmp(full, "CHandle", 7) == 0) {
                *kind = "handle";
                auto* at = static_cast<CSchemaType_Atomic_T*>(t);
                if (at->m_pTemplateType) *inner = at->m_pTemplateType->m_sTypeName.Get();
                break;
            }
            *kind = "atomic"; *type_name = full; break;   // CUtlString, CUtlVector<...>, ...
        }
        default:  // BITFIELD, FIXED_ARRAY, INVALID
            *type_name = t->m_sTypeName.Get(); break;      // kind stays "unknown"
    }
}
```

`CUtlString::Get()` (utlstring.h:302) is inline (`return m_pString ? m_pString : "";`) — no link
dependency. `SchemaBuiltinType_t` (schematypes.h:86) is available if a finer builtin name is ever
wanted (`m_eBuiltinType`); for the catalog `m_sTypeName.Get()` already gives `"int32"` etc., so the
enum is not needed.

`SchemaAtomicCategory_t` (schematypes.h:76) — for reference: `PLAIN=0, T=1, COLLECTION_OF_T=2,
TT=3, I=4, INVALID=5`.

---

## Step 4 — Validation (CONFIRMED)

All live on `de_inferno`, `libserver.so` scope:

- **Class count large:** `746` classes → full catalog feasible.
- **`m_iHealth` = int32 at the resolver offset:**
  `m_iHealth: resolver_offset=1456 own_field_offset=1456 kind=atomic typename=int32 builtinEnum=7`
  — `s2_schema_offset("CCSPlayerPawn","m_iHealth")` returns **1456**; the field's `CSchemaType` is
  `SCHEMA_TYPE_BUILTIN`, `m_sTypeName="int32"`, `m_eBuiltinType=7` (`SCHEMA_BUILTIN_TYPE_INT32`).
  Match confirmed.
- **`CHandle<>` field → `kind="handle"` with inner class name:**
  - On `CCSPlayerPawn` (inherited): `CCSPlayerPawnBase::m_hOriginalController @4044` → `handle`,
    inner=`CCSPlayerController`.
  - On `CCSPlayerController` (direct): `m_hPlayerPawn @3004` (`"CHandle< CCSPlayerPawn >"`) →
    `handle`, inner=`CCSPlayerPawn`; `m_hObserverPawn` → inner=`CCSObserverPawn`;
    `m_hOriginalControllerOfCurrentPawn` → inner=`CCSPlayerController`.
- **Other kinds seen live:** `ptr` (`m_pBulletServices` → inner `CCSPlayer_BulletServices`),
  `atomic` (`CUtlString`, `CUtlVector< CHandle< CBaseEntity > >`), `unknown` (`char[18]` fixed array).

---

## GO / NO-GO

**GO.** Enumeration is achievable entirely in the shim through the typed SDK, with the clean
`CUtlTSHash` element/handle API (no raw offsets), and the category→`kind` mapping is fully
determined and live-confirmed. Task 3 can implement `schema_enumerate` directly from the confirmed
recipe above.

### Deltas the implementation tasks must carry (do not re-derive)

1. **Field type member is `m_pType`**, not `m_pSchemaType` (fix Task 3's skeleton comment).
2. **CHandle detection is by `m_sTypeName` prefix `"CHandle"`**, not `m_pAtomicInfo` (which is NULL).
   Inner class = `m_pTemplateType->m_sTypeName.Get()`.
3. **Iterate `scope->m_ClassBindings`** (public `CUtlTSHash`) via `Count()`/`GetElements()`/
   `Element()`; `#include <vector>` (or a stack/heap handle buffer) in the shim.
4. **Parent-chain completeness:** union `GlobalTypeScope()` or emit base classes reached via
   `m_pBaseClasses` that weren't in the module scope, so no `parent` name dangles (Task 3 decision).
5. `m_eBuiltinType` is available but unnecessary — `m_sTypeName.Get()` already yields `"int32"` etc.

### Build hygiene

`bash scripts/build-sniper.sh` builds clean after scratch removal; `git status` clean; `s2script.so`
needs only `GLIBC_2.14`, `libs2script_core.so` `GLIBC_2.30` (both ≤ 2.31). The SDK iteration adds no
new link dependency.

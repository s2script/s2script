// EKV slice — spawn-with-keyvalues (CEntityKeyValues-configured DispatchSpawn).
//
// The CEntityKeyValues* stays void* outside ekv.cpp — the ONLY TU that includes
// entity2/entitykeyvalues.h. The blast radius of the vendored SDK headers (and everything they
// transitively pull in) is one file; every other TU in the shim (including s2script_mm.cpp, which
// calls these) never sees the real type. The raw pointer never crosses to JS either: it lives and
// dies entirely inside one Shim_EntitySpawnKv call (build -> AddRef -> DispatchSpawn -> guarded
// Release), never stored, never handed out as a handle.
#pragma once

// build a CEntityKeyValues from parallel arrays. types[i]: 0=string 1=int 2=float 3=bool; values
// are the stringified forms ("1"/"0" for bool) — the shim converts (strtol/strtof) and calls the
// matching typed Set*. Returns null on any malformed entry (empty key, null value, unknown type) —
// the whole build fails closed, never a partially-populated kv object handed back.
void* S2EKV_Build(int count, const char* const* keys, const int* types, const char* const* values);

// AddRef the built kv object (refcount starts at 0 — see ekv.cpp's Shim_EntitySpawnKv contract).
void S2EKV_AddRef(void* ekv);

// Release the kv object IFF the engine isn't still holding it queued for spawn. Returns 1 if
// released (our delete ran), 0 if the engine still holds it queued (the caller deliberately leaks
// it rather than risk a UAF/cross-heap free — see the header comment on Shim_EntitySpawnKv).
int S2EKV_ReleaseIfSafe(void* ekv);

// Load-time integrity self-test: stack-allocate a CEntityKeyValues, set one of each supported
// type, read them back. Proves link + ctor + Set*/Get* layout are consistent with THIS build —
// re-run on every hl2sdk/CS2 update (the treadmill). A failure degrades kv-spawns to false; it
// disables nothing else.
bool S2EKV_SelfTest();

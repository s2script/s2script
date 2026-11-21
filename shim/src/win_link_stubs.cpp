// Windows-only definitions for symbols the Linux shim leaves to the dynamic loader or defines with
// layout-only stand-ins (tier1_shims.cpp). PE images cannot carry undefined symbols, and MSVC mangles
// a variable's type into its name, so these need the real SDK types.
#include "mathlib/vector.h"
#include "mathlib/mathlib.h"
#include "entity2/entitysystem.h"

const Quaternion quat_identity(0.0f, 0.0f, 0.0f, 1.0f);
const Vector vec3_origin(0.0f, 0.0f, 0.0f);
const QAngle vec3_angle(0.0f, 0.0f, 0.0f);
const Vector vec3_invalid(FLT_MAX, FLT_MAX, FLT_MAX);

// Reached only from CEntityKeyValues::GetEHandle on a string-typed key (an entity name). On Linux the
// call binds lazily into the game and is never hit by the shim's EKV use; server.dll does not export
// it, so on Windows a by-name EHandle read resolves to "no entity" until name lookup is ported.
CEntityHandle CEntitySystem::FindFirstEntityHandleByName(const char*, WorldGroupId_t) {
    return CEntityHandle();
}

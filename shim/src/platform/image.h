#pragma once

namespace s2platform {

// Resolve a primary class vtable from the loaded image's native RTTI representation.
// Linux uses Itanium RTTI; Windows uses MSVC CompleteObjectLocator records.
void** FindVTableByName(const char* module, const char* className);

}  // namespace s2platform

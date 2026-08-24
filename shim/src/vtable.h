#pragma once
// Native RTTI vtable-by-name resolution (ray-trace slice, Task 1).
//
// CS2 does not export game-class vtables. The platform image backend walks the loaded image's native
// RTTI chain (Itanium type_info on Linux, MSVC CompleteObjectLocator on Windows) to recover the
// primary vtable without relying on a symbol table.
//
// Engine-generic: nothing here names a CS2 class. The caller supplies the class name as a string.
namespace s2vtable {

// Resolve `className`'s PRIMARY vtable (its first virtual-function slot, vtable[0]) in the module
// matching `module`; largest executable range wins when multiple loaded paths match. `className`
// is undecorated (for example "CNavPhysicsInterface").
//
// Returns nullptr if the module, image metadata, RTTI name, or the vtable back-reference
// chain can't be located. Callers MUST treat a null return as "the class isn't resolvable on this
// binary" and degrade (no call through the vtable) — never assume a fallback index.
void** GetVTableByName(const char* module, const char* className);

} // namespace s2vtable

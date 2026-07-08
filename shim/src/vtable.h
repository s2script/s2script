#pragma once
// RTTI vtable-by-name resolution (ray-trace slice, Task 1).
//
// CS2 does not export game-class vtables via dlsym: the pinned libserver.so's .symtab is stripped
// and game classes (e.g. CNavPhysicsInterface) are not in .dynsym either. But the Itanium C++ ABI
// still emits, for every polymorphic class, an RTTI type_info object carrying a decorated
// class-name string in .rodata, PLUS a data-section pointer FROM that class's own primary vtable
// BACK to the type_info object (vtable[-1] is the type_info pointer, i.e. offset-to-top is at
// vtable[-2]). Walking that relationship backwards — string -> type_info -> vtable — needs no
// symbol table at all, only the loaded module's own bytes.
//
// Ported from DynLibUtils (github.com/FUNPLAY-pro-CS2/Ray-Trace, vendor/dynlibutils/
// module_linux.cpp + module.h + memaddr.h) and adapted to the shim's byte-scan primitives
// (s2sig::FindPattern) + its existing largest-PF_X-segment module selection (FindModuleText in
// s2script_mm.cpp — dodges the Metamod libserver.so proxy; see Slice 5D.2).
//
// Engine-generic: nothing here names a CS2 class. The caller supplies the class name as a string.
namespace s2vtable {

// Resolve `className`'s PRIMARY vtable (its first virtual-function slot, vtable[0]) by an RTTI
// type_info name scan against the ON-DISK image of the module matching `module` (a soname
// substring, resolved to the largest matching PF_X segment's owning module — same
// disambiguation as FindModuleText). `className` is UNDECORATED (no Itanium length prefix, e.g.
// "CNavPhysicsInterface" — the length prefix + trailing-NUL boundary check are computed
// internally so a shorter name can't false-positive-match as a prefix of a longer one).
//
// Returns nullptr if the module, its section headers, the RTTI name, or the vtable back-reference
// chain can't be located. Callers MUST treat a null return as "the class isn't resolvable on this
// binary" and degrade (no call through the vtable) — never assume a fallback index.
void** GetVTableByName(const char* module, const char* className);

} // namespace s2vtable

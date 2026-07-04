// Minimal self-contained x86-64 Linux inline detour (Slice 6.6).
// Overwrites a function prologue with a 14-byte absolute jump to a handler, relocating the stolen
// prologue bytes into an mmap'd trampoline (+ an absolute jump back). Uses the vendored HDE length
// disassembler to find whole-instruction boundaries and REFUSES to patch if any stolen instruction
// is relative/rip-relative or fails to decode (degrade-never-crash). x86 has a coherent icache, so
// no explicit flush is needed. All jumps are absolute, so the trampoline may live anywhere.
#pragma once

namespace s2detour {

// Install an inline detour at `target` jumping to `handler`. On success, *origTrampoline receives a
// callable pointer that runs the ORIGINAL function (relocated prologue + jump back). Returns false
// (no memory touched) on any failure — a relative prologue instruction, a decode error, mprotect/mmap
// failure. Safe to leave uninstalled: the caller degrades to "no hook".
bool Install(void* target, void* handler, void** origTrampoline);

// Restore every installed detour's original bytes and free its trampoline. Call on plugin Unload.
void RemoveAll();

}  // namespace s2detour

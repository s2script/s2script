#pragma once

#include <cstddef>
#include <cstdint>

namespace s2platform {

enum class MemoryProtection {
    ReadOnly,
    ReadWrite,
    ReadExecute,
    ReadWriteExecute,
};

size_t PageSize();

// Trampoline storage. The near form returns null unless the mapping is within rel32 reach of
// `target`; callers retain their existing far-tier fallback policy.
void* AllocateExecutable(size_t size);
void* AllocateExecutableNear(uintptr_t target, size_t size);
void FreeExecutable(void* address, size_t size);

bool ProtectMemory(void* address, size_t size, MemoryProtection protection);
void FlushInstructionCache(void* address, size_t size);

}  // namespace s2platform

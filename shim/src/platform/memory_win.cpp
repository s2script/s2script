#include "memory.h"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

namespace s2platform {
namespace {

DWORD NativeProtection(MemoryProtection protection) {
    switch (protection) {
        case MemoryProtection::ReadOnly:         return PAGE_READONLY;
        case MemoryProtection::ReadWrite:        return PAGE_READWRITE;
        case MemoryProtection::ReadExecute:      return PAGE_EXECUTE_READ;
        case MemoryProtection::ReadWriteExecute: return PAGE_EXECUTE_READWRITE;
    }
    return PAGE_NOACCESS;
}

}  // namespace

size_t PageSize() {
    SYSTEM_INFO info{};
    GetSystemInfo(&info);
    return static_cast<size_t>(info.dwPageSize);
}

void* AllocateExecutable(size_t size) {
    if (!size) return nullptr;
    return VirtualAlloc(nullptr, size, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
}

void* AllocateExecutableNear(uintptr_t target, size_t size) {
    if (!target || !size) return nullptr;
    SYSTEM_INFO info{};
    GetSystemInfo(&info);
    const uintptr_t granularity = static_cast<uintptr_t>(info.dwAllocationGranularity);
    const uintptr_t base = target & ~(granularity - 1);
    constexpr int64_t kReach = 0x7FFF0000LL;
    constexpr uintptr_t kStep = 1u << 20;

    for (uintptr_t distance = kStep; distance < static_cast<uintptr_t>(kReach);
         distance += kStep) {
        for (int above = 0; above < 2; ++above) {
            if (!above && distance > base) continue;
            const uintptr_t candidate = above ? base + distance : base - distance;
            if (candidate < 0x10000) continue;
            void* allocation = VirtualAlloc(reinterpret_cast<void*>(candidate), size,
                                            MEM_RESERVE | MEM_COMMIT,
                                            PAGE_EXECUTE_READWRITE);
            if (!allocation) continue;
            const int64_t delta =
                static_cast<int64_t>(reinterpret_cast<uintptr_t>(allocation)) -
                static_cast<int64_t>(target);
            if (delta > -kReach && delta < kReach) return allocation;
            VirtualFree(allocation, 0, MEM_RELEASE);
        }
    }
    return nullptr;
}

void FreeExecutable(void* address, size_t) {
    if (address) VirtualFree(address, 0, MEM_RELEASE);
}

bool ProtectMemory(void* address, size_t size, MemoryProtection protection) {
    if (!address || !size) return false;
    DWORD oldProtection = 0;
    return VirtualProtect(address, size, NativeProtection(protection), &oldProtection) != FALSE;
}

void FlushInstructionCache(void* address, size_t size) {
    if (address && size) ::FlushInstructionCache(GetCurrentProcess(), address, size);
}

}  // namespace s2platform

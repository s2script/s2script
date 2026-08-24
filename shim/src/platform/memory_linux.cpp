#include "memory.h"

#include <cerrno>
#include <limits>
#include <sys/mman.h>
#include <unistd.h>

namespace s2platform {
namespace {

bool g_noFixedNoReplace = false;

int NativeProtection(MemoryProtection protection) {
    switch (protection) {
        case MemoryProtection::ReadOnly:         return PROT_READ;
        case MemoryProtection::ReadWrite:        return PROT_READ | PROT_WRITE;
        case MemoryProtection::ReadExecute:      return PROT_READ | PROT_EXEC;
        case MemoryProtection::ReadWriteExecute: return PROT_READ | PROT_WRITE | PROT_EXEC;
    }
    return PROT_NONE;
}

void* TryMapAt(uintptr_t address, size_t size) {
    const int protection = PROT_READ | PROT_WRITE | PROT_EXEC;
#ifdef MAP_FIXED_NOREPLACE
    if (!g_noFixedNoReplace) {
        void* mapped = mmap(reinterpret_cast<void*>(address), size, protection,
                            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
        if (mapped != MAP_FAILED) {
            if (reinterpret_cast<uintptr_t>(mapped) == address) return mapped;
            munmap(mapped, size);
            g_noFixedNoReplace = true;
            return nullptr;
        }
        if (errno == EINVAL) g_noFixedNoReplace = true;
        return nullptr;
    }
#endif
    void* mapped = mmap(reinterpret_cast<void*>(address), size, protection,
                        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    return mapped == MAP_FAILED ? nullptr : mapped;
}

}  // namespace

size_t PageSize() {
    const long page = sysconf(_SC_PAGESIZE);
    return page > 0 ? static_cast<size_t>(page) : 0;
}

void* AllocateExecutable(size_t size) {
    if (size == 0) return nullptr;
    void* mapped = mmap(nullptr, size, PROT_READ | PROT_WRITE | PROT_EXEC,
                        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    return mapped == MAP_FAILED ? nullptr : mapped;
}

void* AllocateExecutableNear(uintptr_t target, size_t size) {
    const uintptr_t page = PageSize();
    if (!target || !size || !page) return nullptr;
    const uintptr_t base = target & ~(page - 1);
    constexpr int64_t kReach = 0x7FFF0000LL;
    constexpr uintptr_t kStep = 1u << 20;

    for (uintptr_t distance = kStep; distance < static_cast<uintptr_t>(kReach);
         distance += kStep) {
        for (int above = 0; above < 2; ++above) {
            if (!above && distance > base) continue;
            const uintptr_t candidate = above ? base + distance : base - distance;
            if (candidate < 0x10000) continue;
            void* mapped = TryMapAt(candidate, size);
            if (!mapped) continue;
            const int64_t delta = static_cast<int64_t>(reinterpret_cast<uintptr_t>(mapped)) -
                                  static_cast<int64_t>(target);
            if (delta > -kReach && delta < kReach) return mapped;
            // The plain-hint fallback placed the page outside reach and will keep doing so.
            munmap(mapped, size);
            return nullptr;
        }
    }
    return nullptr;
}

void FreeExecutable(void* address, size_t size) {
    if (address && size) munmap(address, size);
}

bool ProtectMemory(void* address, size_t size, MemoryProtection protection) {
    const uintptr_t page = PageSize();
    if (!address || !size || !page) return false;
    const uintptr_t raw = reinterpret_cast<uintptr_t>(address);
    const uintptr_t max = std::numeric_limits<uintptr_t>::max();
    if (size > max - raw) return false;
    const uintptr_t start = raw & ~(page - 1);
    uintptr_t end = raw + size;
    const uintptr_t remainder = end & (page - 1);
    if (remainder != 0) {
        const uintptr_t padding = page - remainder;
        if (end > max - padding) return false;
        end += padding;
    }
    if (end <= start) return false;
    return mprotect(reinterpret_cast<void*>(start), end - start,
                    NativeProtection(protection)) == 0;
}

void FlushInstructionCache(void* address, size_t size) {
    if (!address || !size) return;
    auto* begin = static_cast<char*>(address);
    __builtin___clear_cache(begin, begin + size);
}

}  // namespace s2platform

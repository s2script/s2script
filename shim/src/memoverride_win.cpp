// Windows: route the shim's global operator new/delete through the engine allocator (g_pMemAlloc),
// so compiled KV3/EKV code and the engine share one heap, as public/tier0/memoverride.cpp does on
// Linux. That SDK file cannot be used here: its Windows half replaces the CRT's own malloc family
// (_msize_base, _heap_alloc, ...) against CRT internals the current UCRT no longer has. Only the
// operator overrides are needed for the one-heap guarantee; plain malloc stays the CRT's.
#include <cstddef>
#include <new>

#include "tier0/memalloc.h"

namespace {
void* Alloc(size_t n) { return g_pMemAlloc->Alloc(n ? n : 1); }
void Free(void* p) { if (p) g_pMemAlloc->Free(p); }
void* AllocAligned(uint8 region, size_t n, std::align_val_t a) {
    return g_pMemAlloc->RegionAllocAligned(region, n ? n : 1, static_cast<size_t>(a));
}
void FreeAligned(uint8 region, void* p) { if (p) g_pMemAlloc->RegionFreeAligned(region, p); }
}  // namespace

void* operator new(size_t n) { return Alloc(n); }
void* operator new[](size_t n) { return Alloc(n); }
void* operator new(size_t n, const std::nothrow_t&) noexcept { return Alloc(n); }
void* operator new[](size_t n, const std::nothrow_t&) noexcept { return Alloc(n); }
void operator delete(void* p) noexcept { Free(p); }
void operator delete[](void* p) noexcept { Free(p); }
void operator delete(void* p, size_t) noexcept { Free(p); }
void operator delete[](void* p, size_t) noexcept { Free(p); }
void operator delete(void* p, const std::nothrow_t&) noexcept { Free(p); }
void operator delete[](void* p, const std::nothrow_t&) noexcept { Free(p); }

void* operator new(size_t n, std::align_val_t a) {
    return AllocAligned(MEMALLOC_REGION_ALLOC_NEW_ALIGNED, n, a);
}
void* operator new(size_t n, std::align_val_t a, const std::nothrow_t&) noexcept {
    return AllocAligned(MEMALLOC_REGION_ALLOC_NEW_ALIGNED, n, a);
}
void* operator new[](size_t n, std::align_val_t a) {
    return AllocAligned(MEMALLOC_REGION_ALLOC_NEW_ARRAY_ALIGNED, n, a);
}
void* operator new[](size_t n, std::align_val_t a, const std::nothrow_t&) noexcept {
    return AllocAligned(MEMALLOC_REGION_ALLOC_NEW_ARRAY_ALIGNED, n, a);
}
void operator delete(void* p, std::align_val_t) noexcept {
    FreeAligned(MEMALLOC_REGION_FREE_NEW_ALIGNED, p);
}
void operator delete(void* p, size_t, std::align_val_t) noexcept {
    FreeAligned(MEMALLOC_REGION_FREE_NEW_ALIGNED, p);
}
void operator delete(void* p, std::align_val_t, const std::nothrow_t&) noexcept {
    FreeAligned(MEMALLOC_REGION_FREE_NEW_ALIGNED, p);
}
void operator delete[](void* p, std::align_val_t) noexcept {
    FreeAligned(MEMALLOC_REGION_FREE_NEW_ARRAY_ALIGNED, p);
}
void operator delete[](void* p, size_t, std::align_val_t) noexcept {
    FreeAligned(MEMALLOC_REGION_FREE_NEW_ARRAY_ALIGNED, p);
}
void operator delete[](void* p, std::align_val_t, const std::nothrow_t&) noexcept {
    FreeAligned(MEMALLOC_REGION_FREE_NEW_ARRAY_ALIGNED, p);
}

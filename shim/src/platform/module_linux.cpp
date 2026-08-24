#include "module.h"

#include <cstring>
#include <link.h>
#include <limits>
#include <sys/mman.h>
#include <unistd.h>
#include <utility>

namespace s2platform {

ModuleView FindModule(const char* name) {
    if (!name || !name[0]) return {};
    struct Context {
        const char* name;
        size_t bestExecutable = 0;
        ModuleView out;
    } context{name, 0, {}};

    dl_iterate_phdr([](dl_phdr_info* info, size_t, void* opaque) -> int {
        auto* ctx = static_cast<Context*>(opaque);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, ctx->name)) return 0;

        size_t largestExecutable = 0;
        const uint8_t* text = nullptr;
        ElfW(Addr) lo = ~static_cast<ElfW(Addr)>(0);
        ElfW(Addr) hi = 0;
        for (int i = 0; i < info->dlpi_phnum; ++i) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type != PT_LOAD) continue;
            if ((ph.p_flags & PF_X) && ph.p_filesz > largestExecutable) {
                largestExecutable = ph.p_filesz;
                text = reinterpret_cast<const uint8_t*>(info->dlpi_addr + ph.p_vaddr);
            }
            if (ph.p_vaddr < lo) lo = ph.p_vaddr;
            if (ph.p_vaddr + ph.p_memsz > hi) hi = ph.p_vaddr + ph.p_memsz;
        }
        if (!text || largestExecutable <= ctx->bestExecutable) return 0;

        ctx->bestExecutable = largestExecutable;
        ctx->out.text = text;
        ctx->out.textSize = largestExecutable;
        ctx->out.lo = reinterpret_cast<const uint8_t*>(info->dlpi_addr + lo);
        ctx->out.hi = reinterpret_cast<const uint8_t*>(info->dlpi_addr + hi);
        ctx->out.imageBase = static_cast<uintptr_t>(info->dlpi_addr);
        ctx->out.path = info->dlpi_name;
        return 0;
    }, &context);
    return context.out;
}

bool ModuleViewForAddress(const void* address, ModuleView* out) {
    if (!address || !out) return false;
    struct Context {
        uintptr_t address;
        bool found = false;
        ModuleView out;
    } context{reinterpret_cast<uintptr_t>(address), false, {}};

    dl_iterate_phdr([](dl_phdr_info* info, size_t, void* opaque) -> int {
        auto* ctx = static_cast<Context*>(opaque);
        uintptr_t lo = 0, hi = 0;
        const uint8_t* text = nullptr;
        size_t textSize = 0;
        for (int i = 0; i < info->dlpi_phnum; ++i) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type != PT_LOAD) continue;
            const uintptr_t start = static_cast<uintptr_t>(info->dlpi_addr + ph.p_vaddr);
            const uintptr_t end = start + ph.p_memsz;
            if (lo == 0 || start < lo) lo = start;
            if (end > hi) hi = end;
            if ((ph.p_flags & PF_X) && ctx->address >= start && ctx->address < end) {
                text = reinterpret_cast<const uint8_t*>(start);
                textSize = ph.p_memsz;
            }
        }
        if (!text) return 0;
        ctx->found = true;
        ctx->out.text = text;
        ctx->out.textSize = textSize;
        ctx->out.lo = reinterpret_cast<const uint8_t*>(lo);
        ctx->out.hi = reinterpret_cast<const uint8_t*>(hi);
        ctx->out.imageBase = static_cast<uintptr_t>(info->dlpi_addr);
        if (info->dlpi_name) ctx->out.path = info->dlpi_name;
        return 1;
    }, &context);

    if (!context.found) return false;
    *out = std::move(context.out);
    return true;
}

bool IsExecutableAddress(const void* address) {
    ModuleView ignored;
    return ModuleViewForAddress(address, &ignored);
}

bool IsReadableAddress(const void* address, size_t size) {
    if (!address || !size) return false;
    const long pageSize = sysconf(_SC_PAGESIZE);
    if (pageSize <= 0) return false;
    const uintptr_t raw = reinterpret_cast<uintptr_t>(address);
    const uintptr_t max = std::numeric_limits<uintptr_t>::max();
    if (size - 1 > max - raw) return false;
    const uintptr_t first = raw &
                            ~static_cast<uintptr_t>(pageSize - 1);
    const uintptr_t last = (raw + size - 1) &
                           ~static_cast<uintptr_t>(pageSize - 1);
    unsigned char resident = 0;
    for (uintptr_t page = first;;) {
        if (mincore(reinterpret_cast<void*>(page), static_cast<size_t>(pageSize), &resident) != 0)
            return false;
        if (page == last) return true;
        if (page > max - static_cast<uintptr_t>(pageSize)) return false;
        page += static_cast<uintptr_t>(pageSize);
    }
}

}  // namespace s2platform

// Linux implementation of the platform seam (platform.h). Behaviour matches the call sites it
// replaced: dladdr for the shim's own path, dl_iterate_phdr PT_LOAD segments for module views,
// /proc/self/maps for executable mappings, mincore for the readable probe.
#include "platform.h"

#include <dlfcn.h>
#include <errno.h>
#include <link.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <cstdio>
#include <cstring>
#include <vector>

namespace s2plat {

const char* PlatformKey() { return "linuxsteamrt64"; }

std::string SelfModulePath() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&SelfModulePath), &info) && info.dli_fname) return info.dli_fname;
    return std::string();
}

std::string Dirname(const std::string& path) {
    const size_t slash = path.find_last_of('/');
    return slash == std::string::npos ? std::string() : path.substr(0, slash);
}

std::string Basename(const std::string& path) {
    const size_t slash = path.find_last_of('/');
    return slash == std::string::npos ? path : path.substr(slash + 1);
}

std::string AddonRoot() {
    std::string self = SelfModulePath();
    if (self.empty()) return self;
    // <root>/bin/<platform>/s2script.so: up three.
    return Dirname(Dirname(Dirname(self)));
}

void ForEachModule(const std::function<bool(const ModuleView&)>& fn) {
    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto& f = *static_cast<const std::function<bool(const ModuleView&)>*>(data);
        std::vector<Segment> segs;
        segs.reserve(info->dlpi_phnum);
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type != PT_LOAD) continue;
            segs.push_back({static_cast<uintptr_t>(info->dlpi_addr + ph.p_vaddr),
                            static_cast<size_t>(ph.p_filesz), static_cast<size_t>(ph.p_memsz),
                            (ph.p_flags & PF_X) != 0});
        }
        ModuleView view{info->dlpi_name ? info->dlpi_name : "", static_cast<uintptr_t>(info->dlpi_addr),
                        segs.data(), static_cast<int>(segs.size())};
        return f(view) ? 1 : 0;
    }, const_cast<void*>(static_cast<const void*>(&fn)));
}

void* FindExport(const char* module, const char* symbol) {
    if (!module) return dlsym(RTLD_DEFAULT, symbol);
    void* h = dlopen(module, RTLD_NOW | RTLD_NOLOAD);
    if (!h) {
        // Not on the loader's search path by that name: find the loaded copy by path.
        std::string path;
        ForEachModule([&](const ModuleView& m) {
            if (*m.path && std::strstr(m.path, module)) { path = m.path; return true; }
            return false;
        });
        if (!path.empty()) h = dlopen(path.c_str(), RTLD_NOW | RTLD_NOLOAD);
    }
    if (!h) return nullptr;
    void* sym = dlsym(h, symbol);
    dlclose(h);   // RTLD_NOLOAD took a reference to an already-loaded module; give it back
    return sym;
}

bool IsReadable(const void* p) {
    const long pageSize = sysconf(_SC_PAGESIZE);
    if (pageSize <= 0) return false;
    const uintptr_t page = reinterpret_cast<uintptr_t>(p) & ~static_cast<uintptr_t>(pageSize - 1);
    unsigned char vec = 0;
    return mincore(reinterpret_cast<void*>(page), static_cast<size_t>(pageSize), &vec) == 0;
}

void ExecutableRanges(std::function<void(Range)> fn) {
    FILE* f = fopen("/proc/self/maps", "r");
    if (!f) return;
    char line[4096];   // PATH_MAX: a longer line would split and its tail fail the parse below
    while (fgets(line, sizeof line, f)) {
        unsigned long long lo = 0, hi = 0;
        char perms[5] = {};
        if (sscanf(line, "%llx-%llx %4s", &lo, &hi, perms) == 3 && perms[2] == 'x')
            fn({static_cast<uintptr_t>(lo), static_cast<uintptr_t>(hi)});
    }
    fclose(f);
}

size_t PageSize() {
    const long pageSize = sysconf(_SC_PAGESIZE);
    return pageSize > 0 ? static_cast<size_t>(pageSize) : 4096;
}

bool PreloadSibling(const char*, std::string&) { return true; }

long ThreadId() { return static_cast<long>(::syscall(SYS_gettid)); }

bool MakeDir(const std::string& path) {
    return mkdir(path.c_str(), 0755) == 0 || errno == EEXIST;
}

}  // namespace s2plat

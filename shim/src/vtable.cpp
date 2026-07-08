#include "vtable.h"
#include "sigscan.h"

#include <link.h>       // dl_iterate_phdr, ElfW
#include <fcntl.h>      // open
#include <unistd.h>     // close
#include <sys/stat.h>   // fstat
#include <sys/mman.h>   // mmap/munmap
#include <cstring>      // strstr, strcmp, memcmp
#include <string>
#include <vector>

namespace s2vtable {

namespace {

// A named, in-memory-mapped section of a module: its runtime address (load bias + sh_addr) and
// byte size. Default-constructed (base=0) means "not found" — GetSectionRange below never
// dereferences a zero base.
struct Section {
    uintptr_t base = 0;
    size_t    size = 0;
};

// Find the module whose LARGEST PF_X (executable) segment, among all loaded modules whose soname
// contains `soname`, is the largest overall — mirrors FindModuleText's disambiguation in
// s2script_mm.cpp (Slice 5D.2: Metamod inserts its own thin libserver.so proxy via the gameinfo
// SearchPath, whose path ALSO contains the "libserver.so" substring; the proxy's .text is tiny
// next to the real ~25 MB game module, so "largest PF_X wins" reliably picks the real module).
// Unlike FindModuleText, this ALSO records the winning module's load bias + on-disk path, because
// RTTI section resolution (below) needs to mmap+parse that file's own ELF section-header table —
// section names aren't recoverable from the process image's program headers alone.
struct ModuleInfo {
    uintptr_t   base = 0;   // dl_phdr_info::dlpi_addr (the load bias) of the winning module
    std::string path;       // dl_phdr_info::dlpi_name (the on-disk path used to load it)
};

bool FindModuleInfo(const char* soname, ModuleInfo* out) {
    struct Ctx {
        const char* name;
        size_t      bestTextSize = 0;
        uintptr_t   bestBase = 0;
        std::string bestPath;
    } ctx;
    ctx.name = soname;

    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto* c = static_cast<Ctx*>(data);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) return 0;  // not a match
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type == PT_LOAD && (ph.p_flags & PF_X) && ph.p_filesz > c->bestTextSize) {
                c->bestTextSize = ph.p_filesz;
                c->bestBase     = info->dlpi_addr;
                c->bestPath     = info->dlpi_name;
            }
        }
        return 0;   // keep scanning ALL modules — the metamod proxy must not shadow the real module
    }, &ctx);

    if (ctx.bestTextSize == 0 || ctx.bestPath.empty()) return false;
    out->base = ctx.bestBase;
    out->path = ctx.bestPath;
    return true;
}

// Parse the ON-DISK ELF file at `path` (mmap'd read-only) for its named section headers, and fill
// in the runtime address (loadBase + sh_addr) + size of the three sections GetVTableByName needs.
// Bounds-checks every offset read from the file before touching it (a truncated/corrupt file
// degrades to "section not found", never an out-of-bounds read). Sections whose sh_addr is 0
// (not part of the loaded image, e.g. debug-only sections) are skipped.
bool ParseSections(const std::string& path, uintptr_t loadBase,
                    Section* rodata, Section* relro, Section* relroLocal) {
    int fd = open(path.c_str(), O_RDONLY);
    if (fd < 0) return false;

    struct stat st;
    if (fstat(fd, &st) != 0 || st.st_size <= 0) { close(fd); return false; }
    const size_t fileSize = static_cast<size_t>(st.st_size);

    void* map = mmap(nullptr, fileSize, PROT_READ, MAP_PRIVATE, fd, 0);
    close(fd);   // the fd isn't needed once mmap'd
    if (map == MAP_FAILED) return false;

    bool ok = false;
    if (fileSize >= sizeof(ElfW(Ehdr))) {
        const auto* ehdr = static_cast<const ElfW(Ehdr)*>(map);
        static const unsigned char kElfMag[4] = { 0x7f, 'E', 'L', 'F' };
        if (std::memcmp(ehdr->e_ident, kElfMag, 4) == 0 &&
            ehdr->e_shoff > 0 && ehdr->e_shnum > 0 && ehdr->e_shstrndx < ehdr->e_shnum) {
            const uintptr_t shTableEnd = ehdr->e_shoff +
                (uintptr_t)ehdr->e_shnum * ehdr->e_shentsize;
            if (shTableEnd <= fileSize) {
                const auto* shdrs = reinterpret_cast<const ElfW(Shdr)*>(
                    reinterpret_cast<uintptr_t>(map) + ehdr->e_shoff);
                const ElfW(Shdr)& strShdr = shdrs[ehdr->e_shstrndx];
                if ((uintptr_t)strShdr.sh_offset + strShdr.sh_size <= fileSize) {
                    const char* strTab = reinterpret_cast<const char*>(
                        reinterpret_cast<uintptr_t>(map) + strShdr.sh_offset);
                    for (int i = 0; i < ehdr->e_shnum; i++) {
                        const ElfW(Shdr)& shdr = shdrs[i];
                        if (shdr.sh_addr == 0) continue;              // not part of the loaded image
                        if (shdr.sh_name >= strShdr.sh_size) continue; // out-of-bounds name index
                        const char* name = strTab + shdr.sh_name;
                        Section sec{ loadBase + (uintptr_t)shdr.sh_addr, (size_t)shdr.sh_size };
                        if (std::strcmp(name, ".rodata") == 0)               *rodata     = sec;
                        else if (std::strcmp(name, ".data.rel.ro") == 0)     *relro      = sec;
                        else if (std::strcmp(name, ".data.rel.ro.local") == 0) *relroLocal = sec;
                    }
                    ok = true;
                }
            }
        }
    }
    munmap(map, fileSize);
    return ok;
}

// Build an exact (no-wildcard) byte pattern for `pattern`/`len` bytes, reusing s2sig::FindPattern
// (which already implements the per-byte wildcard-aware scan the shim's signature gate uses) so
// this file doesn't need its own scanning loop.
std::vector<int> ExactPattern(const uint8_t* bytes, size_t len) {
    std::vector<int> pat(len);
    for (size_t i = 0; i < len; i++) pat[i] = bytes[i];
    return pat;
}

std::vector<int> PointerPattern(uintptr_t value) {
    uint8_t bytes[sizeof(uintptr_t)];
    for (size_t i = 0; i < sizeof(uintptr_t); i++) bytes[i] = (uint8_t)((value >> (8 * i)) & 0xFF);
    return ExactPattern(bytes, sizeof(uintptr_t));
}

} // namespace

void** GetVTableByName(const char* module, const char* className) {
    if (!module || !className || !className[0]) return nullptr;

    ModuleInfo mi;
    if (!FindModuleInfo(module, &mi)) return nullptr;

    Section rodata, relro, relroLocal;
    if (!ParseSections(mi.path, mi.base, &rodata, &relro, &relroLocal)) return nullptr;
    if (!rodata.base || !relro.base) return nullptr;   // both required; relroLocal is optional

    // Step 1-2: decorate the name as the Itanium ABI mangler does for an RTTI type_info name —
    // "<len><name>" — and search .rodata for that exact byte string PLUS its trailing NUL (bounds
    // the match so e.g. "20CFoo" can't false-positive-match as a prefix of "23CFooBar1"'s decorated
    // name; a std::string's data() buffer is guaranteed NUL-terminated at data()[size()]).
    std::string decorated = std::to_string(std::strlen(className)) + className;
    std::vector<uint8_t> nameBytes(decorated.begin(), decorated.end());
    nameBytes.push_back(0);
    std::vector<int> namePat = ExactPattern(nameBytes.data(), nameBytes.size());

    int64_t nameOff = s2sig::FindPattern(reinterpret_cast<const uint8_t*>(rodata.base), rodata.size, namePat);
    if (nameOff == s2sig::kFail) return nullptr;
    uintptr_t typeInfoNameAddr = rodata.base + (uintptr_t)nameOff;

    // Step 3: find an 8-byte pointer in .data.rel.ro whose value == typeInfoNameAddr. That
    // location - 0x8 is the type_info object (the Itanium __class_type_info layout is
    // {vptr, name_ptr, ...} — no separate "back to name" field; the RTTI-name POINTER itself sits
    // at type_info+0x8, so the location holding a pointer TO the name string, minus 0x8, IS the
    // type_info object's address).
    std::vector<int> namePtrPat = PointerPattern(typeInfoNameAddr);
    int64_t refOff = s2sig::FindPattern(reinterpret_cast<const uint8_t*>(relro.base), relro.size, namePtrPat);
    if (refOff == s2sig::kFail) return nullptr;
    uintptr_t referenceTypeName = relro.base + (uintptr_t)refOff;
    uintptr_t typeInfo = referenceTypeName - 0x8;

    // Step 4: search .data.rel.ro then .data.rel.ro.local for an 8-byte pointer == typeInfo whose
    // PRECEDING qword (the vtable's offset-to-top slot, vtable[-2]) == 0 — that identifies the
    // PRIMARY vtable (offset-to-top is 0 only for the first/primary base in a multiple-inheritance
    // layout; a secondary vtable's offset-to-top is a nonzero adjustor). That location + 0x8 is
    // vtable[0], the first virtual function slot.
    std::vector<int> tiPtrPat = PointerPattern(typeInfo);
    Section* sections[2] = { &relro, &relroLocal };
    for (Section* sec : sections) {
        if (!sec->base) continue;
        size_t from = 0;
        while (from < sec->size) {
            int64_t off = s2sig::FindPattern(reinterpret_cast<const uint8_t*>(sec->base) + from,
                                              sec->size - from, tiPtrPat);
            if (off == s2sig::kFail) break;
            uintptr_t loc = sec->base + from + (uintptr_t)off;
            if (loc >= sec->base + 8) {   // guard the -0x8 read against the section's own start
                int64_t offsetToTop = *reinterpret_cast<const int64_t*>(loc - 8);
                if (offsetToTop == 0) {
                    return reinterpret_cast<void**>(loc + 8);
                }
            }
            from += (size_t)off + 1;   // keep scanning past this match for the next candidate
        }
    }
    return nullptr;
}

} // namespace s2vtable

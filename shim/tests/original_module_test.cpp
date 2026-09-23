#include "original_module.h"
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <string>
#include <vector>
#ifdef __linux__
#include <dlfcn.h>
#include <sys/mman.h>
#include <unistd.h>
#endif

namespace {
// Explicit ELF64 little-endian fixture fields, independent of the parser.
void put(std::vector<uint8_t>& b, size_t at, uint64_t value, size_t width) {
    for (size_t i = 0; i < width; ++i) b.at(at + i) = uint8_t(value >> (8 * i));
}
struct ElfFixture {
    std::vector<uint8_t> file = std::vector<uint8_t>(0x2100);
    s2original::Identity identity{7, 9, 0x100000, "abcd1234"};
    std::vector<s2original::Mapping> mappings{
        {0x100000, 0x101000, 0, 7, 9},
        {0x103000, 0x104000, 0x1000, 7, 9},
        {0x105000, 0x106000, 0x2000, 7, 9}};
    static constexpr uintptr_t function_live_address = 0x103000;
    static constexpr size_t ph(size_t index) { return 64 + index * 56; }
    void segment(size_t i, uint32_t type, uint32_t flags, uint64_t off,
                 uint64_t va, uint64_t size, uint64_t mem) {
        size_t p = ph(i);
        put(file, p, type, 4); put(file, p+4, flags, 4);
        put(file, p+8, off, 8); put(file, p+16, va, 8);
        put(file, p+32, size, 8); put(file, p+40, mem, 8);
        put(file, p+48, type == 1 ? 0x1000 : 4, 8);
    }
    ElfFixture() {
        std::memcpy(file.data(), "\177ELF\2\1\1", 7);
        put(file, 16, 3, 2); put(file, 18, 62, 2); put(file, 20, 1, 4);
        put(file, 32, 64, 8); put(file, 52, 64, 2);
        put(file, 54, 56, 2); put(file, 56, 5, 2);
        segment(0, 1, 4, 0, 0, 0x800, 0x800);
        segment(1, 1, 5, 0x1000, 0x3000, 16, 32);
        segment(2, 1, 5, 0x2000, 0x5000, 8, 8);
        segment(3, 4, 4, 0x300, 0x300, 20, 20);
        put(file, 0x300, 4, 4); put(file, 0x304, 4, 4); put(file, 0x308, 3, 4);
        std::memcpy(file.data()+0x30c, "GNU", 4);
        file[0x310] = 0xab; file[0x311] = 0xcd;
        file[0x312] = 0x12; file[0x313] = 0x34;
        file[0x1000] = 0x55; file[0x100f] = 0xc3; file[0x2000] = 0x90;
    }
    std::shared_ptr<const s2original::Image> open(std::string& reason) const {
        return s2original::FromElf(file, identity, identity, mappings, reason);
    }
};
void reject(const ElfFixture& f, const char* category) {
    std::string reason;
    if (f.open(reason) || reason.find(category) == std::string::npos) {
        std::fprintf(stderr, "expected refusal (%s), got: %s\n", category, reason.c_str());
        std::abort();
    }
}
void original_bytes_and_ranges() {
    ElfFixture f; std::string reason = "stale";
    std::vector<uint8_t> live(f.file.begin()+0x1000, f.file.begin()+0x1010);
    live[0] = 0xe9;
    auto image = f.open(reason); assert(image && reason.empty());
    assert(image->identity().load_bias == 0x100000);
    assert(image->executable_segments().size() == 2);
    f.file[0x1000] = 0xcc; live[0] = 0xcc;
    uint8_t byte = 0;
    assert(image->read(f.function_live_address, &byte, 1) && byte == 0x55);
    assert(image->read(0x10300f, &byte, 1) && byte == 0xc3);
    assert(image->read(0x105000, &byte, 1) && byte == 0x90);
    assert(!image->read(UINTPTR_MAX-1, &byte, 4));
    assert(!image->read(0x10300f, &byte, 2));
    assert(!image->read(0x103000, nullptr, 1));
    assert(!image->executable(0x100000)); // mapped data is not original instruction data
    assert(!image->executable(0x103010)); // memsz padding has no file bytes
    assert(!image->executable(0x104000)); // gap between executable segments
    assert(!image->executable(0x103000, 0));
}
void verified_image_reuse() {
    ElfFixture f; std::string reason;
    auto first=f.open(reason); auto same=f.open(reason);
    assert(first && first==same); // identical verified identity/bytes/ranges share immutable storage
    ++f.identity.inode; for (auto& mapping : f.mappings) ++mapping.inode;
    auto remapped=f.open(reason); assert(remapped && remapped!=first);
    --f.identity.inode; for (auto& mapping : f.mappings) --mapping.inode;
    f.file[0x1000]=0xcc;
    auto changed=f.open(reason); assert(changed && changed!=first);
    f.file[0x1000]=0x55; f.mappings[0].readable=false;
    auto permission=f.open(reason); assert(permission && permission!=first);
    assert(first->mapped(0x100000) && !permission->mapped(0x100000));
}
void readable_live_ranges() {
    ElfFixture f; std::string reason;
    f.segment(4, 1, 6, 0x2080, 0x7080, 0x80, 0x2080);
    f.mappings.push_back({0x107000,0x108000,0x2000,7,9});
    f.mappings.push_back({0x108000,0x109100,0,0,0}); // anonymous BSS owned by PT_LOAD
    auto image=f.open(reason); assert(image);
    assert(image->mapped(0x107080,1));
    assert(image->mapped(0x107ff0,32)); // file/anonymous split
    assert(image->mapped(0x1090ff,1));
    assert(!image->mapped(0x109100,1)); // outside memsz even though mapping could be larger
    assert(!image->mapped(0x107000,1)); // prefix outside PT_LOAD
    assert(!image->mapped(0x104000,1)); // mapped-module extent gap
    assert(!image->mapped(UINTPTR_MAX-2,8));
    assert(!image->mapped(0x108000,0));
    assert(!image->executable(0x108000));
    f.mappings.back().readable=false;
    image=f.open(reason); assert(image);
    assert(!image->mapped(0x108000,1));
    f.mappings.back().readable=true; f.mappings.back().inode=77;
    image=f.open(reason); assert(image);
    assert(!image->mapped(0x108000,1)); // another mapping cannot stand in for anonymous BSS
}
void identity_refusals() {
    ElfFixture f; std::string reason;
    for (int field = 0; field != 4; ++field) {
        auto wrong = f.identity;
        if (field == 0) ++wrong.device;
        if (field == 1) ++wrong.inode;
        if (field == 2) ++wrong.load_bias;
        if (field == 3) wrong.build_id = "1234";
        assert(!s2original::FromElf(f.file, f.identity, wrong, f.mappings, reason));
        assert(reason.find("identity") != std::string::npos);
    }
    f.file[0x310] ^= 1; reject(f, "build-id");
    f = ElfFixture{}; f.identity.build_id.clear(); reject(f, "build-id");
    f = ElfFixture{}; ++f.mappings[1].inode; reject(f, "identity");
    f = ElfFixture{}; ++f.mappings[1].device; reject(f, "identity");
    f = ElfFixture{}; f.mappings[1].file_offset = 0; reject(f, "mapping");
    f = ElfFixture{}; f.mappings.erase(f.mappings.begin()+1); reject(f, "mapping");
    f = ElfFixture{}; f.mappings[1].end = f.mappings[1].begin; reject(f, "mapping");
    f = ElfFixture{}; f.mappings[1].end = 0x103008;
    f.mappings.push_back({0x103008, 0x104000, 0x1008, 7, 9});
    assert(f.open(reason)); // permission splits can divide one segment
}
void malformed_elf_refusals() {
    ElfFixture f;
    f.file.resize(63); reject(f, "header");
    f = ElfFixture{}; f.file.resize(100); reject(f, "table");
    f = ElfFixture{}; put(f.file, 32, UINT64_MAX-20, 8); reject(f, "table");
    f = ElfFixture{}; f.file[5] = 2; reject(f, "ELF");
    f = ElfFixture{}; put(f.file, ElfFixture::ph(1)+32, 33, 8); reject(f, "segment");
    f = ElfFixture{}; put(f.file, ElfFixture::ph(1)+8, UINT64_MAX-7, 8); reject(f, "segment");
    f = ElfFixture{}; put(f.file, ElfFixture::ph(1)+16, UINT64_MAX-7, 8); reject(f, "segment");
    f = ElfFixture{}; put(f.file, ElfFixture::ph(1)+40, UINT64_MAX, 8); reject(f, "segment");
    f = ElfFixture{}; put(f.file, ElfFixture::ph(3)+32, 19, 8); reject(f, "note");
    f = ElfFixture{}; put(f.file, 0x304, UINT32_MAX, 4); reject(f, "note");
    f = ElfFixture{}; put(f.file, ElfFixture::ph(3), 0, 4); reject(f, "build-id");
    f = ElfFixture{}; f.segment(2, 1, 5, 0x1000, 0x3000, 16, 32); reject(f, "overlapping");
    f = ElfFixture{}; f.segment(4, 2, 4, 0x400, 0x400, 32, 32);
    put(f.file, 0x400, 22, 8); reject(f, "text relocation"); // DT_TEXTREL
    f = ElfFixture{}; f.segment(4, 2, 4, 0x400, 0x400, 32, 32);
    put(f.file, 0x400, 30, 8); put(f.file, 0x408, 4, 8);
    reject(f, "text relocation"); // DT_FLAGS / DF_TEXTREL
}
#ifdef __linux__
void loaded_module(const char* path, const char* replacement, const char* proxy_path) {
    // Load the small matching-name proxy first: first-match selection is incorrect.
    void* proxy = dlopen(proxy_path, RTLD_NOW | RTLD_LOCAL); assert(proxy);
    void* handle = dlopen(path, RTLD_NOW | RTLD_LOCAL); assert(handle);
    void* symbol = dlsym(handle, "original_module_fixture"); assert(symbol);
    std::string reason;
    auto image = s2original::OpenLoadedModule(path, reason);
    if (!image) std::fprintf(stderr, "%s\n", reason.c_str());
    assert(image);
    uintptr_t address = reinterpret_cast<uintptr_t>(symbol);
    auto selected = s2original::OpenLoadedModule("liboriginal_fixture", reason);
    assert(selected && selected->executable(address));
    auto* data=static_cast<uint8_t*>(dlsym(handle,"original_module_data")); assert(data);
    auto* bss=static_cast<uint8_t*>(dlsym(handle,"original_module_bss")); assert(bss);
    uint8_t live=0;
    assert(image->mapped(reinterpret_cast<uintptr_t>(data)));
    assert(image->read_live(reinterpret_cast<uintptr_t>(data),&live,1) && live==23);
    assert(image->mapped(reinterpret_cast<uintptr_t>(bss)+32768));
    assert(!image->executable(reinterpret_cast<uintptr_t>(bss)+32768));
    assert(image->read_live(reinterpret_cast<uintptr_t>(bss)+32768,&live,1) && live==0);
    bss[32768]=91;
    assert(image->read_live(reinterpret_cast<uintptr_t>(bss)+32768,&live,1) && live==91);
    assert(!image->read_live(1,&live,1));
    assert(!image->read_live(UINTPTR_MAX-2,&live,8));
    uint8_t original = 0; assert(image->read(address, &original, 1));
    size_t page_size = static_cast<size_t>(sysconf(_SC_PAGESIZE));
    auto page = address & ~(uintptr_t(page_size)-1);
    assert(mprotect(reinterpret_cast<void*>(page), page_size, PROT_READ|PROT_WRITE|PROT_EXEC) == 0);
    *static_cast<volatile uint8_t*>(symbol) = original ^ 0xff;
    auto patched_image = s2original::OpenLoadedModule(path, reason); assert(patched_image);
    uint8_t readback = 0; assert(patched_image->read(address, &readback, 1));
    assert(readback == original);
    *static_cast<volatile uint8_t*>(symbol) = original;
    assert(mprotect(reinterpret_cast<void*>(page), page_size, PROT_READ|PROT_EXEC) == 0);
    assert(std::rename(replacement, path) == 0);
    assert(!s2original::OpenLoadedModule(path, reason));
    assert(reason.find("identity") != std::string::npos);
    assert(std::remove(path) == 0);
    assert(!s2original::OpenLoadedModule(path, reason));
    assert(reason.find("backing") != std::string::npos);
    assert(!s2original::OpenLoadedModule("not-a-loaded-s2original-module", reason));
    assert(reason.find("not loaded") != std::string::npos);
    dlclose(handle);
    dlclose(proxy);
    std::puts("original_module: Linux loaded-library patch/replaced/missing identity checks passed");
}
#endif
}
int main(int argc, char** argv) {
    verified_image_reuse();
    readable_live_ranges();
    original_bytes_and_ranges(); identity_refusals(); malformed_elf_refusals();
    std::puts("original_module: original bytes, ELF identity and bounds fixtures passed");
#ifdef __linux__
    if (argc == 4) loaded_module(argv[1], argv[2], argv[3]);
    else std::puts("original_module: SKIP Linux loaded-library fixture (supply main/replacement/proxy library paths)");
#else
    (void)argc; (void)argv;
    std::puts("original_module: SKIP Linux loaded-library discovery (non-Linux host)");
#endif
}

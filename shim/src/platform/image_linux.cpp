#include "image.h"

#include "module.h"
#include "sigscan.h"

#include <cstring>
#include <fcntl.h>
#include <link.h>
#include <string>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

namespace s2platform {
namespace {

struct Section {
    uintptr_t base = 0;
    size_t size = 0;
};

bool ParseSections(const std::string& path, uintptr_t loadBase,
                   Section* rodata, Section* relro, Section* relroLocal) {
    const int fd = open(path.c_str(), O_RDONLY);
    if (fd < 0) return false;

    struct stat statBuffer {};
    if (fstat(fd, &statBuffer) != 0 || statBuffer.st_size <= 0) {
        close(fd);
        return false;
    }
    const size_t fileSize = static_cast<size_t>(statBuffer.st_size);
    void* mapping = mmap(nullptr, fileSize, PROT_READ, MAP_PRIVATE, fd, 0);
    close(fd);
    if (mapping == MAP_FAILED) return false;

    bool ok = false;
    if (fileSize >= sizeof(ElfW(Ehdr))) {
        const auto* header = static_cast<const ElfW(Ehdr)*>(mapping);
        static constexpr unsigned char kElfMagic[] = {0x7f, 'E', 'L', 'F'};
        if (std::memcmp(header->e_ident, kElfMagic, sizeof(kElfMagic)) == 0 &&
            header->e_shoff > 0 && header->e_shnum > 0 &&
            header->e_shstrndx < header->e_shnum) {
            const uintptr_t tableEnd = header->e_shoff +
                static_cast<uintptr_t>(header->e_shnum) * header->e_shentsize;
            if (tableEnd <= fileSize) {
                const auto* sections = reinterpret_cast<const ElfW(Shdr)*>(
                    reinterpret_cast<uintptr_t>(mapping) + header->e_shoff);
                const ElfW(Shdr)& strings = sections[header->e_shstrndx];
                if (static_cast<uintptr_t>(strings.sh_offset) + strings.sh_size <= fileSize) {
                    const char* names = reinterpret_cast<const char*>(
                        reinterpret_cast<uintptr_t>(mapping) + strings.sh_offset);
                    for (int i = 0; i < header->e_shnum; ++i) {
                        const ElfW(Shdr)& section = sections[i];
                        if (section.sh_addr == 0 || section.sh_name >= strings.sh_size) continue;
                        const char* name = names + section.sh_name;
                        const Section found{loadBase + static_cast<uintptr_t>(section.sh_addr),
                                            static_cast<size_t>(section.sh_size)};
                        if (std::strcmp(name, ".rodata") == 0) *rodata = found;
                        else if (std::strcmp(name, ".data.rel.ro") == 0) *relro = found;
                        else if (std::strcmp(name, ".data.rel.ro.local") == 0)
                            *relroLocal = found;
                    }
                    ok = true;
                }
            }
        }
    }
    munmap(mapping, fileSize);
    return ok;
}

std::vector<int> ExactPattern(const uint8_t* bytes, size_t size) {
    std::vector<int> pattern(size);
    for (size_t i = 0; i < size; ++i) pattern[i] = bytes[i];
    return pattern;
}

std::vector<int> PointerPattern(uintptr_t value) {
    uint8_t bytes[sizeof(value)];
    for (size_t i = 0; i < sizeof(value); ++i)
        bytes[i] = static_cast<uint8_t>((value >> (8 * i)) & 0xff);
    return ExactPattern(bytes, sizeof(bytes));
}

}  // namespace

void** FindVTableByName(const char* module, const char* className) {
    if (!module || !className || !className[0]) return nullptr;
    const ModuleView image = FindModule(module);
    if (!image.text || image.path.empty()) return nullptr;

    Section rodata, relro, relroLocal;
    if (!ParseSections(image.path, image.imageBase, &rodata, &relro, &relroLocal) ||
        !rodata.base || !relro.base) {
        return nullptr;
    }

    std::string decorated = std::to_string(std::strlen(className)) + className;
    std::vector<uint8_t> nameBytes(decorated.begin(), decorated.end());
    nameBytes.push_back(0);
    const std::vector<int> namePattern = ExactPattern(nameBytes.data(), nameBytes.size());
    const int64_t nameOffset = s2sig::FindPattern(
        reinterpret_cast<const uint8_t*>(rodata.base), rodata.size, namePattern);
    if (nameOffset == s2sig::kFail) return nullptr;
    const uintptr_t typeInfoName = rodata.base + static_cast<uintptr_t>(nameOffset);

    const std::vector<int> namePointerPattern = PointerPattern(typeInfoName);
    const int64_t referenceOffset = s2sig::FindPattern(
        reinterpret_cast<const uint8_t*>(relro.base), relro.size, namePointerPattern);
    if (referenceOffset == s2sig::kFail) return nullptr;
    const uintptr_t typeInfo = relro.base + static_cast<uintptr_t>(referenceOffset) - 8;

    const std::vector<int> typeInfoPattern = PointerPattern(typeInfo);
    Section* candidates[] = {&relro, &relroLocal};
    for (Section* section : candidates) {
        if (!section->base) continue;
        size_t from = 0;
        while (from < section->size) {
            const int64_t offset = s2sig::FindPattern(
                reinterpret_cast<const uint8_t*>(section->base) + from,
                section->size - from, typeInfoPattern);
            if (offset == s2sig::kFail) break;
            const uintptr_t location = section->base + from + static_cast<uintptr_t>(offset);
            if (location >= section->base + 8 &&
                *reinterpret_cast<const int64_t*>(location - 8) == 0) {
                return reinterpret_cast<void**>(location + 8);
            }
            from += static_cast<size_t>(offset) + 1;
        }
    }
    return nullptr;
}

}  // namespace s2platform

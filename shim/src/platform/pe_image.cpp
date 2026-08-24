#include "pe_image.h"

#include <algorithm>
#include <cstring>
#include <string>
#include <utility>
#include <vector>

namespace s2platform {
namespace {

constexpr uint32_t kSectionExecute = 0x20000000u;
constexpr uint32_t kSectionRead = 0x40000000u;

struct Section {
    const uint8_t* base = nullptr;
    size_t size = 0;
    uint32_t rva = 0;
    uint32_t characteristics = 0;
};

struct ParsedPe {
    const uint8_t* image = nullptr;
    size_t imageSize = 0;
    std::vector<Section> sections;
};

bool HasRange(size_t available, size_t offset, size_t size) {
    return offset <= available && size <= available - offset;
}

template <typename T>
bool Read(const uint8_t* image, size_t available, size_t offset, T* out) {
    if (!out || !HasRange(available, offset, sizeof(T))) return false;
    std::memcpy(out, image + offset, sizeof(T));
    return true;
}

bool Parse(const uint8_t* image, size_t available, ParsedPe* out) {
    if (!image || !out || available < 0x40 || image[0] != 'M' || image[1] != 'Z') return false;

    uint32_t peOffset = 0;
    if (!Read(image, available, 0x3c, &peOffset) || !HasRange(available, peOffset, 24)) return false;
    if (std::memcmp(image + peOffset, "PE\0\0", 4) != 0) return false;

    uint16_t sectionCount = 0;
    uint16_t optionalSize = 0;
    if (!Read(image, available, peOffset + 6, &sectionCount) ||
        !Read(image, available, peOffset + 20, &optionalSize) ||
        sectionCount == 0 || sectionCount > 96) {
        return false;
    }

    const size_t optionalOffset = static_cast<size_t>(peOffset) + 24;
    if (!HasRange(available, optionalOffset, optionalSize) || optionalSize < 60) return false;
    uint16_t magic = 0;
    uint32_t sizeOfImage = 0;
    if (!Read(image, available, optionalOffset, &magic) || magic != 0x20b ||
        !Read(image, available, optionalOffset + 56, &sizeOfImage) ||
        sizeOfImage == 0 || sizeOfImage > available) {
        return false;
    }

    const size_t table = optionalOffset + optionalSize;
    if (!HasRange(available, table, static_cast<size_t>(sectionCount) * 40)) return false;

    ParsedPe parsed;
    parsed.image = image;
    parsed.imageSize = sizeOfImage;
    parsed.sections.reserve(sectionCount);
    for (uint16_t i = 0; i < sectionCount; ++i) {
        const size_t header = table + static_cast<size_t>(i) * 40;
        uint32_t virtualSize = 0, virtualAddress = 0, rawSize = 0, characteristics = 0;
        if (!Read(image, available, header + 8, &virtualSize) ||
            !Read(image, available, header + 12, &virtualAddress) ||
            !Read(image, available, header + 16, &rawSize) ||
            !Read(image, available, header + 36, &characteristics)) {
            return false;
        }
        const size_t sectionSize = std::max<size_t>(virtualSize, rawSize);
        if (sectionSize == 0) continue;
        if (virtualAddress >= sizeOfImage || sectionSize > sizeOfImage - virtualAddress) return false;
        parsed.sections.push_back(
            Section{image + virtualAddress, sectionSize, virtualAddress, characteristics});
    }
    *out = std::move(parsed);
    return true;
}

bool IsReadableData(const Section& section) {
    return (section.characteristics & kSectionRead) &&
           !(section.characteristics & kSectionExecute);
}

bool InReadableSection(const ParsedPe& pe, size_t offset, size_t size) {
    for (const Section& section : pe.sections) {
        if (!IsReadableData(section)) continue;
        const size_t start = section.rva;
        if (offset >= start && size <= section.size && offset - start <= section.size - size)
            return true;
    }
    return false;
}

size_t FindBytes(const ParsedPe& pe, const Section& section, const std::string& needle,
                 size_t from) {
    if (!IsReadableData(section) || needle.empty() || from > section.size ||
        needle.size() > section.size - from) {
        return pe.imageSize;
    }
    for (size_t i = from; i <= section.size - needle.size(); ++i) {
        if (std::memcmp(section.base + i, needle.data(), needle.size()) == 0)
            return section.rva + i;
    }
    return pe.imageSize;
}

}  // namespace

bool ParsePeImage(const uint8_t* image, size_t available, ModuleView* out) {
    if (!out) return false;
    ParsedPe pe;
    if (!Parse(image, available, &pe)) return false;

    const Section* best = nullptr;
    for (const Section& section : pe.sections) {
        if ((section.characteristics & kSectionExecute) &&
            (!best || section.size > best->size)) {
            best = &section;
        }
    }
    if (!best) return false;

    ModuleView view;
    view.text = best->base;
    view.textSize = best->size;
    view.lo = image;
    view.hi = image + pe.imageSize;
    view.imageBase = reinterpret_cast<uintptr_t>(image);
    *out = std::move(view);
    return true;
}

bool PeModuleViewForAddress(const uint8_t* image, size_t available, const void* address,
                            ModuleView* out) {
    if (!address || !out) return false;
    ParsedPe pe;
    if (!Parse(image, available, &pe)) return false;

    const uintptr_t target = reinterpret_cast<uintptr_t>(address);
    for (const Section& section : pe.sections) {
        if (!(section.characteristics & kSectionExecute)) continue;
        const uintptr_t start = reinterpret_cast<uintptr_t>(section.base);
        if (target < start || target - start >= section.size) continue;

        ModuleView view;
        view.text = section.base;
        view.textSize = section.size;
        view.lo = image;
        view.hi = image + pe.imageSize;
        view.imageBase = reinterpret_cast<uintptr_t>(image);
        *out = std::move(view);
        return true;
    }
    return false;
}

void** FindMsvcVTableInPe(const uint8_t* image, size_t available, const char* className) {
    if (!className || !className[0]) return nullptr;
    ParsedPe pe;
    if (!Parse(image, available, &pe)) return nullptr;

    const std::string suffix = std::string(className) + "@@";
    const std::string decorated[] = {".?AV" + suffix, ".?AU" + suffix};
    for (const std::string& name : decorated) {
        std::string terminated = name;
        terminated.push_back('\0');
        for (const Section& nameSection : pe.sections) {
            if (!IsReadableData(nameSection)) continue;
            size_t nameFrom = 0;
            while (nameFrom < nameSection.size) {
                const size_t nameOffset = FindBytes(pe, nameSection, terminated, nameFrom);
                if (nameOffset == pe.imageSize) break;
                nameFrom = nameOffset - nameSection.rva + 1;
                if (nameOffset < 2 * sizeof(uintptr_t)) continue;
                const size_t typeDescriptor = nameOffset - 2 * sizeof(uintptr_t);
                if (!InReadableSection(pe, typeDescriptor,
                                       2 * sizeof(uintptr_t) + terminated.size())) {
                    continue;
                }
                const uint32_t typeDescriptorRva = static_cast<uint32_t>(typeDescriptor);

                for (const Section& locatorSection : pe.sections) {
                    if (!IsReadableData(locatorSection)) continue;
                    size_t locatorOffset =
                        (static_cast<size_t>(locatorSection.rva) + alignof(uint32_t) - 1) &
                        ~(alignof(uint32_t) - 1);
                    const size_t locatorEnd =
                        static_cast<size_t>(locatorSection.rva) + locatorSection.size;
                    for (; locatorOffset <= locatorEnd && locatorEnd - locatorOffset >= 24;
                         locatorOffset += alignof(uint32_t)) {
                        uint32_t signature = 0, objectOffset = 0, typeRva = 0, selfRva = 0;
                        std::memcpy(&signature, image + locatorOffset, sizeof(signature));
                        std::memcpy(&objectOffset, image + locatorOffset + 4,
                                    sizeof(objectOffset));
                        std::memcpy(&typeRva, image + locatorOffset + 12, sizeof(typeRva));
                        std::memcpy(&selfRva, image + locatorOffset + 20, sizeof(selfRva));
                        if (signature != 1 || objectOffset != 0 ||
                            typeRva != typeDescriptorRva || selfRva != locatorOffset) {
                            continue;
                        }

                        const uintptr_t locator =
                            reinterpret_cast<uintptr_t>(image + locatorOffset);
                        for (const Section& pointerSection : pe.sections) {
                            if (!IsReadableData(pointerSection)) continue;
                            size_t pointerOffset =
                                (static_cast<size_t>(pointerSection.rva) +
                                 alignof(uintptr_t) - 1) & ~(alignof(uintptr_t) - 1);
                            const size_t pointerEnd =
                                static_cast<size_t>(pointerSection.rva) + pointerSection.size;
                            for (; pointerOffset <= pointerEnd &&
                                   pointerEnd - pointerOffset >= 2 * sizeof(uintptr_t);
                                 pointerOffset += alignof(uintptr_t)) {
                                uintptr_t candidate = 0;
                                std::memcpy(&candidate, image + pointerOffset, sizeof(candidate));
                                if (candidate == locator) {
                                    return reinterpret_cast<void**>(
                                        const_cast<uint8_t*>(image + pointerOffset +
                                                             sizeof(uintptr_t)));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    return nullptr;
}

}  // namespace s2platform

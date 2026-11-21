// A minimal PE32+ x64 image for the original-module and RTTI fixtures: headers, .text, .rdata
// (holding the CodeView RSDS record) and .data (file-backed, then zero fill past its raw size).
// Fields are written explicitly, independent of the parser. `live` is what the loaded image looks
// like at `base`; the loader's ImageBase rewrite is applied to it.
#pragma once
#include "original_module.h"
#include <cstring>
#include <vector>

namespace pefixture {
inline void put(std::vector<uint8_t>& b, size_t at, uint64_t value, size_t width) {
    for (size_t i = 0; i < width; ++i) b.at(at + i) = uint8_t(value >> (8 * i));
}
struct PeFixture {
    static constexpr uintptr_t base = 0x10000000;
    static constexpr size_t nt = 0x80, opt = nt + 24, table = opt + 0xf0;
    static constexpr size_t text_raw = 0x400, rdata_raw = 0x600, data_raw = 0x800;
    static constexpr size_t codeview_raw = rdata_raw + 0x40;
    static constexpr uint32_t text_rva = 0x1000, rdata_rva = 0x2000, data_rva = 0x3000;
    static constexpr size_t image_size = 0x5000;
    std::vector<uint8_t> file = std::vector<uint8_t>(0xa00);
    std::vector<uint8_t> live = std::vector<uint8_t>(image_size);
    s2original::Identity identity{3, 5, base, "", s2original::Format::Pe};

    void section(size_t i, const char* name, uint32_t vaddr, uint32_t vsize, uint32_t raw,
                 uint32_t rawsize, uint32_t flags) {
        size_t p = table + i * 40;
        std::memcpy(file.data() + p, name, std::strlen(name));
        put(file, p + 8, vsize, 4); put(file, p + 12, vaddr, 4);
        put(file, p + 16, rawsize, 4); put(file, p + 20, raw, 4); put(file, p + 36, flags, 4);
    }
    PeFixture() {
        file[0] = 'M'; file[1] = 'Z'; put(file, 0x3c, nt, 4);
        std::memcpy(file.data() + nt, "PE\0\0", 4);
        put(file, nt + 4, 0x8664, 2); put(file, nt + 6, 3, 2); put(file, nt + 8, 0x5f000000, 4);
        put(file, nt + 20, 0xf0, 2);
        put(file, opt, 0x20b, 2); put(file, opt + 24, 0x180000000ull, 8);
        put(file, opt + 56, image_size, 4); put(file, opt + 60, 0x400, 4);
        put(file, opt + 108, 16, 4);
        put(file, opt + 112 + 6 * 8, rdata_rva, 4); put(file, opt + 112 + 6 * 8 + 4, 28, 4);
        section(0, ".text", text_rva, 0x10, text_raw, 0x200, 0x60000020);
        section(1, ".rdata", rdata_rva, 0x100, rdata_raw, 0x200, 0x40000040);
        section(2, ".data", data_rva, 0x2000, data_raw, 0x200, 0xc0000040);
        // Debug directory entry -> CodeView record "RSDS" + GUID + age.
        put(file, rdata_raw + 12, 2, 4); put(file, rdata_raw + 16, 32, 4);
        put(file, rdata_raw + 20, rdata_rva + 0x40, 4); put(file, rdata_raw + 24, codeview_raw, 4);
        std::memcpy(file.data() + codeview_raw, "RSDS", 4);
        for (int i = 0; i < 16; ++i) file[codeview_raw + 4 + i] = uint8_t(0xa0 + i);
        put(file, codeview_raw + 20, 7, 4);
        file[text_raw] = 0x55; file[text_raw + 0xf] = 0xc3; file[text_raw + 0x10] = 0xcc;
        file[data_raw] = 23;
        load();
    }
    // Map the file the way the loader does, relocated to `base`.
    void load() {
        std::fill(live.begin(), live.end(), 0);
        std::copy(file.begin(), file.begin() + 0x400, live.begin());
        put(live, opt + 24, base, 8);
        std::copy(file.begin() + text_raw, file.begin() + text_raw + 0x200, live.begin() + text_rva);
        std::copy(file.begin() + rdata_raw, file.begin() + rdata_raw + 0x200, live.begin() + rdata_rva);
        std::copy(file.begin() + data_raw, file.begin() + data_raw + 0x200, live.begin() + data_rva);
    }
    s2original::LiveRead reader() const {
        return [this](uintptr_t at, void* out, size_t n) {
            if (at < base || at - base > live.size() || n > live.size() - (at - base)) return false;
            std::memcpy(out, live.data() + (at - base), n);
            return true;
        };
    }
    std::shared_ptr<const s2original::Image> open(std::string& reason) const {
        return s2original::FromPe(file, identity, reader(), reason);
    }
};
} // namespace pefixture

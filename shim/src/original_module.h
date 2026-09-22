#pragma once
#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace s2original {
struct Identity {
    uint64_t device = 0, inode = 0;
    uintptr_t load_bias = 0;
    std::string build_id;
};
struct Segment {
    uintptr_t live_begin = 0;
    std::vector<uint8_t> bytes;
};
struct Mapping { uintptr_t begin, end; uint64_t file_offset, device, inode; };
// Owned, immutable instruction bytes indexed by engine PCs, never by buffer addresses.
// Relocated data/vtables/strings are live facts and deliberately outside this interface.
class Image {
public:
    const Identity& identity() const noexcept;
    const std::vector<Segment>& executable_segments() const noexcept;
    bool read(uintptr_t live, void* out, size_t length) const noexcept;
    bool executable(uintptr_t live, size_t length = 1) const noexcept;
private:
    Image(Identity identity, std::vector<Segment> segments);
    const Identity identity_;
    const std::vector<Segment> segments_;
    friend std::shared_ptr<const Image> FromElf(
        const std::vector<uint8_t>&, const Identity&, const Identity&,
        const std::vector<Mapping>&, std::string&);
};
std::shared_ptr<const Image> OpenLoadedModule(const char* module, std::string& reason);
std::shared_ptr<const Image> FromElf(
    const std::vector<uint8_t>& file, const Identity& expected,
    const Identity& observed, const std::vector<Mapping>& mappings,
    std::string& reason);
} // namespace s2original

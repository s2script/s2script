#pragma once
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace s2original {
// Which object format (and so which C++ ABI's RTTI) the image carries.
enum class Format : uint8_t { Elf, Pe };
struct Identity {
    uint64_t device = 0, inode = 0;   // PE: volume serial number, file index
    uintptr_t load_bias = 0;          // PE: the image base
    std::string build_id;             // PE: CodeView RSDS GUID + age, hex
    Format format = Format::Elf;
};
struct Segment {
    uintptr_t live_begin = 0;
    std::vector<uint8_t> bytes;
};
struct Mapping { uintptr_t begin, end; uint64_t file_offset, device, inode; bool readable = true; };
struct ReadableRange { uintptr_t begin, end; };
using LiveRead = std::function<bool(uintptr_t, void*, size_t)>;
// Owned, immutable instruction bytes indexed by engine PCs, never by buffer addresses.
// Relocated data/vtables/strings are separate live facts, available only through
// readable_ranges/mapped/read_live, never through the original instruction read.
class Image {
public:
    const Identity& identity() const noexcept;
    const std::vector<Segment>& executable_segments() const noexcept;
    bool read(uintptr_t live, void* out, size_t length) const noexcept;
    const std::vector<ReadableRange>& readable_ranges() const noexcept;
    bool mapped(uintptr_t live, size_t length = 1) const noexcept;
    // Kernel-mediated live data read. Never supplies the immutable instruction view.
    bool read_live(uintptr_t live, void* out, size_t length) const noexcept;
    bool executable(uintptr_t live, size_t length = 1) const noexcept;
private:
    Image(Identity identity, std::vector<Segment> segments, std::vector<ReadableRange> ranges);
    const Identity identity_;
    const std::vector<Segment> segments_;
    const std::vector<ReadableRange> readable_;
    static std::shared_ptr<const Image> Intern(const Identity&, std::vector<Segment>,
                                               std::vector<ReadableRange>, std::string&);
    friend std::shared_ptr<const Image> FromElf(
        const std::vector<uint8_t>&, const Identity&, const Identity&,
        const std::vector<Mapping>&, std::string&);
    friend std::shared_ptr<const Image> FromPe(
        const std::vector<uint8_t>&, const Identity&, const LiveRead&, std::string&);
};
std::shared_ptr<const Image> OpenLoadedModule(const char* module, std::string& reason);
std::shared_ptr<const Image> FromElf(
    const std::vector<uint8_t>& file, const Identity& expected,
    const Identity& observed, const std::vector<Mapping>& mappings,
    std::string& reason);
// A PE32+ x64 image: `file` is the DLL on disk, `expected` the loaded module's identity (its
// image base, file identity and, once parsed, build id). `read_live` reads the LOADED image; the
// headers, section table and CodeView record it returns must match the file's, so a different
// DLL at that base (or one patched on disk after load) is refused. Executable sections become
// the original instruction segments, from the file bytes; every readable section is a readable
// range. The build id is parsed here and written into the returned identity.
std::shared_ptr<const Image> FromPe(
    const std::vector<uint8_t>& file, const Identity& expected,
    const LiveRead& read_live, std::string& reason);
} // namespace s2original

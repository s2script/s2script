#include "original_module.h"
#include <algorithm>
#include <cstring>
#include <limits>
#include <utility>
#ifdef __linux__
#include <cstdio>
#include <fcntl.h>
#include <fstream>
#include <link.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>
#endif

namespace s2original {
namespace {
// Decode explicitly: the fixture/parser is portable even on hosts without <elf.h>.
struct ProgramHeader {
    uint32_t type, flags;
    uint64_t offset, vaddr, filesz, memsz, align;
};
bool fits(uint64_t start, uint64_t length, uint64_t limit) {
    return start <= limit && length <= limit - start;
}
uint64_t le(const std::vector<uint8_t>& b, size_t at, size_t count) {
    uint64_t value = 0;
    for (size_t i = 0; i < count; ++i) value |= uint64_t(b[at+i]) << (8*i);
    return value;
}
bool refuse(std::string& reason, const char* message) { reason = message; return false; }
bool parse(const std::vector<uint8_t>& file, std::vector<ProgramHeader>& headers,
           std::string& build_id, std::string& reason) {
    if (file.size() < 64) return refuse(reason, "ELF header truncated");
    if (std::memcmp(file.data(), "\177ELF\2\1\1", 7) != 0 ||
        (le(file, 16, 2) != 3 && le(file, 16, 2) != 2) ||
        le(file, 18, 2) != 62 || le(file, 20, 4) != 1 || le(file, 52, 2) != 64)
        return refuse(reason, "unsupported ELF header (requires ELF64 little-endian x86-64)");
    uint64_t table = le(file, 32, 8), count = le(file, 56, 2);
    if (!count || count == 0xffff || le(file, 54, 2) != 56 ||
        !fits(table, count * 56, file.size()))
        return refuse(reason, "ELF program-header table invalid or truncated");
    for (uint64_t i = 0; i < count; ++i) {
        size_t p = static_cast<size_t>(table + i*56);
        ProgramHeader h{uint32_t(le(file,p,4)), uint32_t(le(file,p+4,4)),
            le(file,p+8,8), le(file,p+16,8), le(file,p+32,8),
            le(file,p+40,8), le(file,p+48,8)};
        if (!fits(h.offset, h.filesz, file.size()))
            return refuse(reason, "ELF segment file range invalid");
        if (h.type == 1 && (h.filesz > h.memsz ||
            !fits(h.vaddr, h.memsz, UINTPTR_MAX) ||
            (h.align > 1 && ((h.align & (h.align-1)) ||
                             h.vaddr % h.align != h.offset % h.align))))
            return refuse(reason, "ELF load segment bounds/alignment invalid");
        headers.push_back(h);
        if (h.type == 4) {
            uint64_t pnote = h.offset, end = h.offset + h.filesz;
            while (pnote < end) {
                if (!fits(pnote, 12, end)) return refuse(reason, "ELF note header truncated");
                uint64_t namesz = le(file, pnote, 4), descsz = le(file, pnote+4, 4);
                uint64_t type = le(file, pnote+8, 4);
                uint64_t namepad = (namesz+3) & ~uint64_t(3);
                uint64_t descpad = (descsz+3) & ~uint64_t(3);
                pnote += 12;
                if (!fits(pnote, namepad, end) || !fits(pnote+namepad, descpad, end))
                    return refuse(reason, "ELF note payload truncated");
                if (type == 3 && namesz == 4 &&
                    std::memcmp(file.data()+pnote, "GNU", 4) == 0) {
                    if (!descsz) return refuse(reason, "ELF build-id note empty");
                    std::string id;
                    static constexpr char hex[] = "0123456789abcdef";
                    for (uint64_t j = 0; j < descsz; ++j) {
                        uint8_t byte = file[pnote+namepad+j];
                        id += hex[byte >> 4]; id += hex[byte & 15];
                    }
                    if (!build_id.empty() && build_id != id)
                        return refuse(reason, "ELF build-id notes conflict");
                    build_id = std::move(id);
                }
                pnote += namepad + descpad;
            }
        }
        if (h.type == 2) {
            if (h.filesz % 16) return refuse(reason, "ELF dynamic segment truncated");
            bool terminated = false;
            for (uint64_t pos = h.offset; pos < h.offset+h.filesz; pos += 16) {
                uint64_t tag = le(file, pos, 8), value = le(file, pos+8, 8);
                if (!tag) { terminated = true; break; }
                if (tag == 22 || (tag == 30 && (value & 4)))
                    return refuse(reason, "unsupported text relocation in ELF dynamic segment");
            }
            if (!terminated) return refuse(reason, "ELF dynamic segment missing terminator");
        }
    }
    if (build_id.empty()) return refuse(reason, "ELF build-id missing");
    return true;
}
// A PT_LOAD may be split across maps (e.g. RELRO). Every backed byte must have
// matching file identity AND matching file offset, including nonzero load bias.
bool mapped(const ProgramHeader& h, const Identity& id,
            const std::vector<Mapping>& maps, std::string& reason) {
    if (!fits(id.load_bias, h.vaddr, UINTPTR_MAX) ||
        !fits(id.load_bias+h.vaddr, h.memsz, UINTPTR_MAX))
        return refuse(reason, "ELF live segment arithmetic overflow");
    uintptr_t begin = id.load_bias + h.vaddr;
    uint64_t done = 0;
    while (done < h.filesz) {
        uintptr_t cursor = begin + done;
        const Mapping* match = nullptr;
        for (const auto& m : maps) {
            if (cursor >= m.begin && cursor < m.end) {
                if (match) return refuse(reason, "overlapping loaded mappings");
                match = &m;
            }
        }
        if (!match) return refuse(reason, "ELF segment mapping missing");
        if (match->device != id.device || match->inode != id.inode)
            return refuse(reason, "loaded mapping identity mismatch");
        uint64_t delta = cursor - match->begin;
        if (!fits(match->file_offset, delta, UINT64_MAX) ||
            match->file_offset + delta != h.offset + done)
            return refuse(reason, "ELF segment mapping file offset mismatch");
        done += std::min<uint64_t>(h.filesz-done, match->end-cursor);
    }
    return true;
}
} // namespace

Image::Image(Identity identity, std::vector<Segment> segments)
    : identity_(std::move(identity)), segments_(std::move(segments)) {}
const Identity& Image::identity() const noexcept { return identity_; }
const std::vector<Segment>& Image::executable_segments() const noexcept { return segments_; }
bool Image::executable(uintptr_t live, size_t length) const noexcept {
    if (!length || !fits(live, length, UINTPTR_MAX)) return false;
    for (const auto& segment : segments_)
        if (live >= segment.live_begin &&
            fits(live-segment.live_begin, length, segment.bytes.size())) return true;
    return false;
}
bool Image::read(uintptr_t live, void* out, size_t length) const noexcept {
    if (!out || !executable(live, length)) return false;
    for (const auto& segment : segments_) {
        if (live >= segment.live_begin &&
            fits(live-segment.live_begin, length, segment.bytes.size())) {
            std::memcpy(out, segment.bytes.data()+(live-segment.live_begin), length);
            return true;
        }
    }
    return false;
}
std::shared_ptr<const Image> FromElf(
    const std::vector<uint8_t>& file, const Identity& expected,
    const Identity& observed, const std::vector<Mapping>& mappings, std::string& reason) {
    reason.clear();
    if (expected.device != observed.device || expected.inode != observed.inode ||
        expected.load_bias != observed.load_bias || expected.build_id != observed.build_id) {
        reason = "module identity mismatch"; return nullptr;
    }
    std::vector<ProgramHeader> headers;
    std::string build_id;
    if (!parse(file, headers, build_id, reason)) return nullptr;
    if (expected.build_id.empty() || build_id != expected.build_id) {
        reason = "module build-id identity mismatch"; return nullptr;
    }
    for (const auto& m : mappings)
        if (m.begin >= m.end) { reason = "invalid loaded mapping bounds"; return nullptr; }
    std::vector<Segment> segments;
    std::vector<std::pair<uintptr_t, uintptr_t>> load_ranges;
    for (const auto& h : headers) {
        if (h.type != 1) continue;
        if (!mapped(h, expected, mappings, reason)) return nullptr;
        uintptr_t begin = expected.load_bias+h.vaddr, end = begin+h.memsz;
        for (auto range : load_ranges)
            if (begin < range.second && range.first < end) {
                reason = "overlapping ELF load segments"; return nullptr;
            }
        if (h.memsz) load_ranges.emplace_back(begin, end);
        if ((h.flags & 1) && h.filesz)
            segments.push_back({begin, std::vector<uint8_t>(file.begin()+h.offset,
                                                          file.begin()+h.offset+h.filesz)});
    }
    if (segments.empty()) { reason = "ELF has no file-backed executable segments"; return nullptr; }
    std::sort(segments.begin(), segments.end(),
              [](const Segment& a, const Segment& b) { return a.live_begin < b.live_begin; });
    return std::shared_ptr<const Image>(new Image(expected, std::move(segments)));
}

#ifdef __linux__
namespace {
struct Fd {
    int value;
    explicit Fd(int fd) : value(fd) {}
    ~Fd() { if (value >= 0) close(value); }
    Fd(const Fd&) = delete;
    Fd& operator=(const Fd&) = delete;
};
struct Loaded {
    const char* query;
    std::string path;
    uintptr_t bias = 0;
    uint64_t largest_text = 0;
    std::vector<ProgramHeader> headers;
};
int discover(dl_phdr_info* info, size_t, void* opaque) {
    auto& result = *static_cast<Loaded*>(opaque);
    if (!info->dlpi_name || !std::strstr(info->dlpi_name, result.query)) return 0;
    uint64_t largest = 0;
    for (size_t i = 0; i < info->dlpi_phnum; ++i)
        if (info->dlpi_phdr[i].p_type == PT_LOAD && (info->dlpi_phdr[i].p_flags & PF_X))
            largest = std::max<uint64_t>(largest, info->dlpi_phdr[i].p_memsz);
    // Preserve engine selection when Metamod also loads a tiny same-name proxy.
    if (largest <= result.largest_text) return 0;
    result.largest_text = largest; result.path = info->dlpi_name;
    result.bias = info->dlpi_addr; result.headers.clear();
    for (size_t i = 0; i < info->dlpi_phnum; ++i) {
        const auto& p = info->dlpi_phdr[i];
        result.headers.push_back({p.p_type, p.p_flags, p.p_offset,
                                  p.p_vaddr, p.p_filesz, p.p_memsz, p.p_align});
    }
    return 0;
}
bool read_at(int fd, uint64_t offset, uint8_t* out, size_t length) {
    if (!fits(offset, length, std::numeric_limits<off_t>::max())) return false;
    size_t done = 0;
    while (done < length) {
        ssize_t n = pread(fd, out+done, std::min<size_t>(length-done, 1u<<20),
                          static_cast<off_t>(offset+done));
        if (n <= 0) return false;
        done += static_cast<size_t>(n);
    }
    return true;
}
bool same_header(const ProgramHeader& a, const ProgramHeader& b) {
    return a.type == b.type && a.flags == b.flags && a.offset == b.offset &&
           a.vaddr == b.vaddr && a.filesz == b.filesz && a.memsz == b.memsz && a.align == b.align;
}
} // namespace
#endif

std::shared_ptr<const Image> OpenLoadedModule(const char* module, std::string& reason) {
    reason.clear();
#ifdef __linux__
    if (!module || !*module) { reason = "module name empty"; return nullptr; }
    Loaded loaded{module, {}, 0, 0, {}};
    dl_iterate_phdr(discover, &loaded);
    if (loaded.path.empty()) { reason = "module not loaded: " + std::string(module); return nullptr; }
    auto fail = [&](const std::string& why) -> std::shared_ptr<const Image> {
        reason = loaded.path + ": " + why; return nullptr;
    };
    std::ifstream maps_file("/proc/self/maps");
    if (!maps_file) return fail("loaded mappings unavailable");
    std::vector<Mapping> mappings;
    std::string line;
    while (std::getline(maps_file, line)) {
        unsigned long long begin, end, offset, inode;
        unsigned device_major, device_minor;
        char permissions[5] = {};
        if (std::sscanf(line.c_str(), "%llx-%llx %4s %llx %x:%x %llu",
                        &begin, &end, permissions, &offset,
                        &device_major, &device_minor, &inode) != 7)
            return fail("loaded mapping parse failure");
        mappings.push_back({static_cast<uintptr_t>(begin), static_cast<uintptr_t>(end),
                            offset, static_cast<uint64_t>(makedev(device_major, device_minor)), inode});
    }
    Fd backing(open(loaded.path.c_str(), O_RDONLY | O_CLOEXEC));
    if (backing.value < 0) return fail("backing file missing or unverifiable");
    struct stat before{};
    if (fstat(backing.value, &before) || !S_ISREG(before.st_mode) || before.st_size < 64)
        return fail("backing file unverifiable");
    Identity observed{static_cast<uint64_t>(before.st_dev), static_cast<uint64_t>(before.st_ino),
                      loaded.bias, {}};
    // Validate identity BEFORE reading any loaded note or instruction address.
    for (const auto& h : loaded.headers)
        if (h.type == 1 && !mapped(h, observed, mappings, reason)) return fail(reason);
    if (static_cast<uint64_t>(before.st_size) > std::numeric_limits<size_t>::max())
        return fail("backing file size overflow");
    std::vector<uint8_t> file(static_cast<size_t>(before.st_size));
    if (!read_at(backing.value, 0, file.data(), file.size())) return fail("backing file read failed");
    std::vector<ProgramHeader> file_headers;
    if (!parse(file, file_headers, observed.build_id, reason)) return fail(reason);
    if (file_headers.size() != loaded.headers.size()) return fail("loaded ELF header identity mismatch");
    for (size_t i = 0; i < file_headers.size(); ++i)
        if (!same_header(file_headers[i], loaded.headers[i]))
            return fail("loaded ELF segment identity mismatch");
    // Read only verified, file-backed PT_NOTE spans through the kernel. A bad live
    // pointer cannot fault the process, and executable bytes are never read live.
    Fd memory(open("/proc/self/mem", O_RDONLY | O_CLOEXEC));
    if (memory.value < 0) return fail("loaded build-id unverifiable");
    bool verified_note = false;
    for (const auto& note : file_headers) {
        if (note.type != 4 || !note.filesz) continue;
        bool backed = false;
        for (const auto& load : file_headers) {
            if (load.type == 1 && note.vaddr >= load.vaddr &&
                fits(note.vaddr-load.vaddr, note.filesz, load.filesz) &&
                note.offset == load.offset+(note.vaddr-load.vaddr)) backed = true;
        }
        if (!backed) return fail("loaded build-id note is not file-backed");
        std::vector<uint8_t> live_note(static_cast<size_t>(note.filesz));
        if (!read_at(memory.value, loaded.bias+note.vaddr, live_note.data(), live_note.size()) ||
            !std::equal(live_note.begin(), live_note.end(), file.begin()+note.offset))
            return fail("loaded build-id note identity mismatch");
        verified_note = true;
    }
    if (!verified_note) return fail("loaded build-id missing");
    struct stat after{};
    if (fstat(backing.value, &after) || before.st_dev != after.st_dev ||
        before.st_ino != after.st_ino || before.st_size != after.st_size ||
        before.st_mtim.tv_sec != after.st_mtim.tv_sec || before.st_mtim.tv_nsec != after.st_mtim.tv_nsec ||
        before.st_ctim.tv_sec != after.st_ctim.tv_sec || before.st_ctim.tv_nsec != after.st_ctim.tv_nsec)
        return fail("backing file changed during verification");
    auto image = FromElf(file, observed, observed, mappings, reason);
    if (!image) return fail(reason);
    return image;
#else
    (void)module;
    reason = "loaded module discovery unsupported on this platform (Linux required)";
    return nullptr;
#endif
}
} // namespace s2original

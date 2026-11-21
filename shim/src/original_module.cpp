#include "original_module.h"
#include <algorithm>
#include <cstring>
#include <limits>
#include <mutex>
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
#ifdef _WIN32
#include "platform.h"
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
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

Image::Image(Identity identity, std::vector<Segment> segments, std::vector<ReadableRange> ranges)
    : identity_(std::move(identity)), segments_(std::move(segments)), readable_(std::move(ranges)) {}
const Identity& Image::identity() const noexcept { return identity_; }
const std::vector<Segment>& Image::executable_segments() const noexcept { return segments_; }
const std::vector<ReadableRange>& Image::readable_ranges() const noexcept { return readable_; }
bool Image::mapped(uintptr_t live, size_t length) const noexcept {
    if (!length || !fits(live,length,UINTPTR_MAX)) return false;
    for (const auto& range : readable_)
        if (live >= range.begin && live < range.end && length <= range.end-live) return true;
    return false;
}
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
    std::vector<ReadableRange> ranges;
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
        if (h.flags & 4) {
            for (const auto& m : mappings) {
                uintptr_t lo = std::max(begin,m.begin), hi = std::min(end,m.end);
                if (lo >= hi || !m.readable) continue;
                // File-backed data and page-tail BSS share module identity. Anonymous
                // BSS is admitted only beyond filesz and inside this selected PT_LOAD.
                bool own = m.device == expected.device && m.inode == expected.inode;
                if (!own && (m.device || m.inode || lo < begin+h.filesz)) continue;
                ranges.push_back({lo,hi});
            }
        }
        if ((h.flags & 1) && h.filesz)
            segments.push_back({begin, std::vector<uint8_t>(file.begin()+h.offset,
                                                          file.begin()+h.offset+h.filesz)});
    }
    if (segments.empty()) { reason = "ELF has no file-backed executable segments"; return nullptr; }
    return Image::Intern(expected, std::move(segments), std::move(ranges), reason);
}

std::shared_ptr<const Image> Image::Intern(const Identity& expected, std::vector<Segment> segments,
                                           std::vector<ReadableRange> ranges, std::string& reason) {
    std::sort(segments.begin(), segments.end(),
              [](const Segment& a, const Segment& b) { return a.live_begin < b.live_begin; });
    std::sort(ranges.begin(),ranges.end(),[](const ReadableRange& a,const ReadableRange& b) {
        return a.begin < b.begin;
    });
    std::vector<ReadableRange> merged;
    for (const auto& range : ranges) {
        if (!merged.empty() && range.begin < merged.back().end) {
            reason = "overlapping readable mappings"; return nullptr;
        }
        if (!merged.empty() && range.begin == merged.back().end) merged.back().end=range.end;
        else merged.push_back(range);
    }
    // Share only fully verified immutable images, never a pathname or recipe result.
    // Re-verification above is intentional: inode reuse, changed bytes, or permission
    // changes must not inherit stale data bounds from a still-retained image.
    static std::mutex cache_mutex;
    static std::vector<std::weak_ptr<const Image>> cache;
    std::lock_guard<std::mutex> lock(cache_mutex);
    for (auto it=cache.begin(); it!=cache.end();) {
        auto old=it->lock();
        if (!old) { it=cache.erase(it); continue; }
        ++it;
        const auto& id=old->identity();
        if (id.device!=expected.device || id.inode!=expected.inode || id.load_bias!=expected.load_bias ||
            id.build_id!=expected.build_id || id.format!=expected.format ||
            old->segments_.size()!=segments.size() || old->readable_.size()!=merged.size()) continue;
        bool same=true;
        for (size_t i=0; i<segments.size(); ++i)
            if (old->segments_[i].live_begin!=segments[i].live_begin || old->segments_[i].bytes!=segments[i].bytes) { same=false; break; }
        for (size_t i=0; same && i<merged.size(); ++i)
            if (old->readable_[i].begin!=merged[i].begin || old->readable_[i].end!=merged[i].end) same=false;
        if (same) return old;
    }
    auto image=std::shared_ptr<const Image>(new Image(expected,std::move(segments),std::move(merged)));
    cache.push_back(image);
    return image;
}

namespace {
// PE32+ layout, decoded explicitly (no <windows.h>) so the parser and its fixtures are portable.
constexpr uint32_t kScnExecute = 0x20000000, kScnRead = 0x40000000;
struct PeSection { uint32_t vaddr, vsize, raw, rawsize, flags; };
struct PeHeaders {
    size_t nt = 0, opt = 0, opt_size = 0, table = 0, headers_size = 0;
    uint64_t image_size = 0;
    std::vector<PeSection> sections;
    uint32_t codeview_rva = 0, codeview_size = 0;
};
bool parse_pe(const std::vector<uint8_t>& file, PeHeaders& h, std::string& build_id,
              std::string& reason) {
    if (file.size() < 0x40 || file[0] != 'M' || file[1] != 'Z')
        return refuse(reason, "PE DOS header truncated or invalid");
    h.nt = static_cast<size_t>(le(file, 0x3c, 4));
    if (!fits(h.nt, 24, file.size()) || std::memcmp(file.data()+h.nt, "PE\0\0", 4) != 0)
        return refuse(reason, "PE signature missing");
    if (le(file, h.nt+4, 2) != 0x8664) return refuse(reason, "unsupported PE machine (requires x64)");
    const uint64_t count = le(file, h.nt+6, 2);
    h.opt = h.nt + 24;
    h.opt_size = static_cast<size_t>(le(file, h.nt+20, 2));
    if (h.opt_size < 112 || !fits(h.opt, h.opt_size, file.size()) || le(file, h.opt, 2) != 0x20b)
        return refuse(reason, "unsupported PE optional header (requires PE32+)");
    h.image_size = le(file, h.opt+56, 4);
    h.headers_size = static_cast<size_t>(le(file, h.opt+60, 4));
    const uint64_t dirs = le(file, h.opt+108, 4);
    h.table = h.opt + h.opt_size;
    if (!count || !fits(h.table, count*40, file.size()) || h.headers_size < h.table + count*40 ||
        h.headers_size > file.size() || !h.image_size)
        return refuse(reason, "PE section table invalid or truncated");
    for (uint64_t i = 0; i < count; ++i) {
        const size_t p = static_cast<size_t>(h.table + i*40);
        PeSection s{uint32_t(le(file,p+12,4)), uint32_t(le(file,p+8,4)), uint32_t(le(file,p+20,4)),
                    uint32_t(le(file,p+16,4)), uint32_t(le(file,p+36,4))};
        if (!s.vsize) s.vsize = s.rawsize;
        if (!fits(s.vaddr, s.vsize, h.image_size) || s.vaddr < h.headers_size ||
            (s.rawsize && !fits(s.raw, s.rawsize, file.size())))
            return refuse(reason, "PE section bounds invalid");
        for (const auto& o : h.sections)
            if (s.vaddr < uint64_t(o.vaddr)+o.vsize && o.vaddr < uint64_t(s.vaddr)+s.vsize)
                return refuse(reason, "overlapping PE sections");
        h.sections.push_back(s);
    }
    auto file_offset = [&](uint64_t rva, uint64_t length, uint64_t& out) {
        for (const auto& s : h.sections)
            if (rva >= s.vaddr && fits(rva-s.vaddr, length, std::min(s.vsize, s.rawsize))) {
                out = s.raw + (rva-s.vaddr); return true;
            }
        return false;
    };
    // IMAGE_DIRECTORY_ENTRY_DEBUG (6): the CodeView RSDS record names the build (GUID + age).
    if (dirs > 6 && h.opt_size >= 112 + 7*8) {
        const uint64_t rva = le(file, h.opt+112+6*8, 4), size = le(file, h.opt+112+6*8+4, 4);
        uint64_t at = 0;
        if (size && (size % 28 || !file_offset(rva, size, at)))
            return refuse(reason, "PE debug directory invalid");
        for (uint64_t e = 0; size && e < size; e += 28) {
            if (le(file, at+e+12, 4) != 2) continue;   // IMAGE_DEBUG_TYPE_CODEVIEW
            const uint64_t length = le(file, at+e+16, 4), record_rva = le(file, at+e+20, 4);
            const uint64_t record = le(file, at+e+24, 4);
            uint64_t mapped_at = 0;
            if (length < 24 || !fits(record, length, file.size()) ||
                !file_offset(record_rva, length, mapped_at) || mapped_at != record ||
                std::memcmp(file.data()+record, "RSDS", 4) != 0)
                return refuse(reason, "PE CodeView record invalid");
            std::string id;
            static constexpr char hex[] = "0123456789abcdef";
            for (uint64_t j = 4; j < 24; ++j) {
                uint8_t byte = file[static_cast<size_t>(record+j)];
                id += hex[byte >> 4]; id += hex[byte & 15];
            }
            if (!build_id.empty() && build_id != id) return refuse(reason, "PE CodeView records conflict");
            build_id = std::move(id);
            h.codeview_rva = uint32_t(record_rva); h.codeview_size = uint32_t(length);
        }
    }
    if (build_id.empty()) return refuse(reason, "PE CodeView build id missing");
    return true;
}
} // namespace

std::shared_ptr<const Image> FromPe(const std::vector<uint8_t>& file, const Identity& expected,
                                    const LiveRead& read_live, std::string& reason) {
    reason.clear();
    PeHeaders h;
    std::string build_id;
    if (!parse_pe(file, h, build_id, reason)) return nullptr;
    if (!expected.build_id.empty() && expected.build_id != build_id) {
        reason = "module build-id identity mismatch"; return nullptr;
    }
    const uintptr_t base = expected.load_bias;
    if (!base || !fits(base, h.image_size, UINTPTR_MAX)) {
        reason = "PE live image arithmetic overflow"; return nullptr;
    }
    if (!read_live) { reason = "PE loaded image unverifiable"; return nullptr; }
    // The loaded headers must be this file's. The loader rewrites only ImageBase (opt+24) when it
    // relocates the image; every other header byte, and the section table, must match.
    std::vector<uint8_t> live(h.headers_size);
    if (!read_live(base, live.data(), live.size())) {
        reason = "PE loaded headers unreadable"; return nullptr;
    }
    const size_t table_end = h.table + h.sections.size()*40;
    for (size_t i = 0; i < table_end; ++i) {
        if (i >= h.opt+24 && i < h.opt+32) continue;
        if (live[i] != file[i]) { reason = "loaded PE header identity mismatch"; return nullptr; }
    }
    std::vector<uint8_t> record(h.codeview_size);
    if (!read_live(base+h.codeview_rva, record.data(), record.size())) {
        reason = "loaded PE build id unreadable"; return nullptr;
    }
    for (const auto& s : h.sections)
        if (h.codeview_rva >= s.vaddr && h.codeview_rva - s.vaddr < s.vsize &&
            !std::equal(record.begin(), record.end(), file.begin()+s.raw+(h.codeview_rva-s.vaddr))) {
            reason = "loaded PE build id identity mismatch"; return nullptr;
        }
    Identity identity = expected;
    identity.build_id = build_id;
    identity.format = Format::Pe;
    std::vector<Segment> segments;
    std::vector<ReadableRange> ranges{{base, base+h.headers_size}};
    for (const auto& s : h.sections) {
        const uintptr_t begin = base + s.vaddr;
        if (s.flags & kScnRead) ranges.push_back({begin, begin+s.vsize});
        const uint32_t backed = std::min(s.vsize, s.rawsize);
        if ((s.flags & kScnExecute) && backed)
            segments.push_back({begin, std::vector<uint8_t>(file.begin()+s.raw, file.begin()+s.raw+backed)});
    }
    if (segments.empty()) { reason = "PE has no file-backed executable sections"; return nullptr; }
    return Image::Intern(identity, std::move(segments), std::move(ranges), reason);
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

bool Image::read_live(uintptr_t live, void* out, size_t length) const noexcept {
    if (!out || !mapped(live,length)) return false;
#ifdef __linux__
    Fd memory(open("/proc/self/mem", O_RDONLY | O_CLOEXEC));
    return memory.value >= 0 && read_at(memory.value,live,static_cast<uint8_t*>(out),length);
#elif defined(_WIN32)
    // Kernel-mediated like /proc/self/mem: an unmapped or guard page fails the read, never faults.
    SIZE_T done = 0;
    return ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<LPCVOID>(live), out, length, &done) &&
           done == length;
#else
    return false; // portable tests inject their own bounded live data
#endif
}

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
                            offset, static_cast<uint64_t>(makedev(device_major, device_minor)), inode, permissions[0] == 'r'});
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
#elif defined(_WIN32)
    if (!module || !*module) { reason = "module name empty"; return nullptr; }
    // Same selection as the platform seam's callers: among modules whose path contains `module`,
    // the one with the largest executable section wins (Metamod's same-name proxy is tiny).
    uintptr_t base = 0; size_t largest = 0; std::string path;
    s2plat::ForEachModule([&](const s2plat::ModuleView& m) {
        if (!std::strstr(m.path, module)) return false;
        for (int i = 0; i < m.count; ++i)
            if (m.segs[i].exec && m.segs[i].fileSize > largest) {
                largest = m.segs[i].fileSize; base = m.base; path = m.path;
            }
        return false;
    });
    if (!base) { reason = "module not loaded: " + std::string(module); return nullptr; }
    auto fail = [&](const std::string& why) -> std::shared_ptr<const Image> {
        reason = path + ": " + why; return nullptr;
    };
    std::vector<wchar_t> wide(32768);
    const DWORD n = GetModuleFileNameW(reinterpret_cast<HMODULE>(base), wide.data(),
                                       static_cast<DWORD>(wide.size()));
    if (!n || n >= wide.size()) return fail("backing file path unavailable");
    HANDLE backing = CreateFileW(wide.data(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_DELETE,
                                 nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (backing == INVALID_HANDLE_VALUE) return fail("backing file missing or unverifiable");
    struct Closer { HANDLE h; ~Closer() { CloseHandle(h); } } closer{backing};
    BY_HANDLE_FILE_INFORMATION before{}, after{};
    if (!GetFileInformationByHandle(backing, &before) ||
        (before.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY))
        return fail("backing file unverifiable");
    const uint64_t size = (uint64_t(before.nFileSizeHigh) << 32) | before.nFileSizeLow;
    if (size < 0x40 || size > 0x7fffffff) return fail("backing file size invalid");
    std::vector<uint8_t> file(static_cast<size_t>(size));
    for (size_t done = 0; done < file.size();) {
        DWORD got = 0;
        const DWORD want = static_cast<DWORD>(std::min<size_t>(file.size()-done, 1u << 20));
        if (!ReadFile(backing, file.data()+done, want, &got, nullptr) || !got)
            return fail("backing file read failed");
        done += got;
    }
    if (!GetFileInformationByHandle(backing, &after) ||
        before.dwVolumeSerialNumber != after.dwVolumeSerialNumber ||
        before.nFileIndexHigh != after.nFileIndexHigh || before.nFileIndexLow != after.nFileIndexLow ||
        before.nFileSizeHigh != after.nFileSizeHigh || before.nFileSizeLow != after.nFileSizeLow ||
        CompareFileTime(&before.ftLastWriteTime, &after.ftLastWriteTime) != 0)
        return fail("backing file changed during verification");
    Identity observed;
    observed.device = before.dwVolumeSerialNumber;
    observed.inode = (uint64_t(before.nFileIndexHigh) << 32) | before.nFileIndexLow;
    observed.load_bias = base;
    observed.format = Format::Pe;
    auto live = [](uintptr_t at, void* out, size_t length) {
        SIZE_T done = 0;
        return ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<LPCVOID>(at), out, length,
                                 &done) && done == length;
    };
    auto image = FromPe(file, observed, live, reason);
    if (!image) return fail(reason);
    return image;
#else
    (void)module;
    reason = "loaded module discovery unsupported on this platform";
    return nullptr;
#endif
}
} // namespace s2original

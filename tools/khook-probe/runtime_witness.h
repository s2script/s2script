#pragma once

// Test-probe identity evidence only. A pathname/stat alone does not identify the
// file already loaded by the process: an operator may have replaced that path.
#include <array>
#include <charconv>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <functional>
#include <map>
#include <optional>
#include <sstream>
#include <set>
#include <string>
#include <vector>
#include <sys/stat.h>
#if defined(__linux__)
#include <link.h>
#include <sys/sysmacros.h>
#endif

namespace s2khook::runtime {

inline constexpr std::array<const char*, 5> kRoles{
    "probe", "shim", "core", "metamod", "metamod_loader"
};

struct FileIdentity {
    std::string path;
    uint64_t major = 0, minor = 0, inode = 0;
    std::string Device() const {
        std::ostringstream out;
        out << std::hex << major << ':' << minor;
        return out.str();
    }
    std::string Inode() const { return std::to_string(inode); }
};
struct Segment { uint64_t address, offset; };
struct Image { std::string path; std::vector<Segment> segments; };
struct Mapping {
    uint64_t start, end, offset, major, minor, inode;
    bool deleted;
    std::string path;
};
using Modules = std::map<std::string, std::vector<std::optional<FileIdentity>>>;

inline bool EndsWith(const std::string& value, const std::string& suffix) {
    return value.size() >= suffix.size() &&
        value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}

inline std::string NormalizePath(const std::string& path) {
    if (path.empty()) return {};
    std::error_code error;
    const auto absolute = std::filesystem::absolute(path, error);
    return error ? std::string{} : absolute.lexically_normal().string();
}

inline std::string RoleForPath(const std::string& path) {
    const std::string normalized = NormalizePath(path);
    if (EndsWith(normalized, "/s2_khook_probe.so")) return "probe";
    if (EndsWith(normalized, "/s2script.so")) return "shim";
    if (EndsWith(normalized, "/libs2script_core.so")) return "core";
    // Engine libserver.so has the same basename and bin suffix. Only the
    // supported Metamod addon layout is a host witness.
    if (EndsWith(normalized, "/addons/metamod/bin/linuxsteamrt64/metamod.2.cs2.so")) return "metamod";
    if (EndsWith(normalized, "/addons/metamod/bin/linuxsteamrt64/libserver.so")) return "metamod_loader";
    return {};
}

inline bool Number(const std::string& token, int base, uint64_t& out) {
    if (token.empty()) return false;
    const auto result = std::from_chars(token.data(), token.data() + token.size(), out, base);
    return result.ec == std::errc{} && result.ptr == token.data() + token.size();
}

inline std::vector<Mapping> ParseMaps(std::istream& input) {
    std::vector<Mapping> result;
    std::string line;
    while (std::getline(input, line)) {
        std::istringstream row(line);
        std::string range, permissions, offset, device, inode, path;
        if (!(row >> range >> permissions >> offset >> device >> inode)) continue;
        std::getline(row, path);
        const auto dash = range.find('-'), colon = device.find(':');
        Mapping parsed{};
        if (dash == std::string::npos || colon == std::string::npos ||
            !Number(range.substr(0, dash), 16, parsed.start) ||
            !Number(range.substr(dash + 1), 16, parsed.end) || parsed.start >= parsed.end ||
            !Number(offset, 16, parsed.offset) ||
            !Number(device.substr(0, colon), 16, parsed.major) ||
            !Number(device.substr(colon + 1), 16, parsed.minor) ||
            !Number(inode, 10, parsed.inode)) continue;
        parsed.deleted = EndsWith(path, " (deleted)");
        const auto first = path.find_first_not_of(' ');
        parsed.path = first == std::string::npos ? std::string{} : path.substr(first);
        if (parsed.deleted) parsed.path.resize(parsed.path.size() - std::string(" (deleted)").size());
        result.push_back(parsed);
    }
    return result;
}

inline std::optional<FileIdentity> ReadFileIdentity(const std::string& path) {
    char* resolved = ::realpath(path.c_str(), nullptr);
    if (!resolved) return std::nullopt;
    const std::string canonical(resolved);
    std::free(resolved);
    struct stat file{};
    if (::stat(canonical.c_str(), &file) != 0 || !S_ISREG(file.st_mode) ||
        file.st_ino == 0 || file.st_nlink == 0) return std::nullopt;
    return FileIdentity{canonical, static_cast<uint64_t>(major(file.st_dev)),
        static_cast<uint64_t>(minor(file.st_dev)), static_cast<uint64_t>(file.st_ino)};
}

inline std::optional<FileIdentity> MatchImage(const Image& image,
        const std::vector<Mapping>& maps, const std::optional<FileIdentity>& file) {
    if (!file || image.segments.empty()) return std::nullopt;
    for (const auto& segment : image.segments) {
        const Mapping* match = nullptr;
        for (const auto& mapping : maps) {
            if (segment.address < mapping.start || segment.address >= mapping.end) continue;
            if (match) return std::nullopt;
            match = &mapping;
        }
        if (!match || match->deleted || !match->inode ||
            match->major != file->major || match->minor != file->minor ||
            match->inode != file->inode || segment.offset < match->offset ||
            segment.address - match->start != segment.offset - match->offset) return std::nullopt;
    }
    return file;
}

inline void AddImage(Modules& modules, const Image& image,
        const std::vector<Mapping>& maps,
        const std::function<std::optional<FileIdentity>(const std::string&)>& read = ReadFileIdentity) {
    const auto file = read(image.path);
    const auto reported_role = RoleForPath(image.path);
    const auto canonical_role = file ? RoleForPath(file->path) : std::string{};
    const auto role = reported_role.empty() ? canonical_role : reported_role;
    if (role.empty()) {
        // A deleted/retargeted load-time alias may conceal the known filename.
        // Kernel mapping paths can identify that role for rejection, but never
        // produce a ready row without a resolved/stat-verified load path.
        std::set<std::string> mapped_roles;
        for (const auto& segment : image.segments) {
            for (const auto& mapping : maps) {
                if (segment.address < mapping.start || segment.address >= mapping.end) continue;
                const auto mapped_role = RoleForPath(mapping.path);
                if (!mapped_role.empty()) mapped_roles.insert(mapped_role);
            }
        }
        for (const auto& mapped_role : mapped_roles) modules[mapped_role].push_back(std::nullopt);
        return;
    }
    // Count images even when their path is missing/deleted or identity is wrong;
    // ignoring them could falsely turn an ambiguous role into a unique one.
    const bool conflict = !reported_role.empty() && !canonical_role.empty() &&
        reported_role != canonical_role;
    modules[role].push_back(conflict ? std::nullopt : MatchImage(image, maps, file));
    if (conflict) modules[canonical_role].push_back(std::nullopt);
}

inline std::optional<FileIdentity> UniqueModule(const Modules& modules, const std::string& role) {
    const auto found = modules.find(role);
    return found != modules.end() && found->second.size() == 1 ? found->second.front() : std::nullopt;
}

inline Modules CollectModules() {
    Modules modules;
#if defined(__linux__)
    std::ifstream input("/proc/self/maps");
    if (!input) return modules;
    const auto maps = ParseMaps(input);
    struct Context { Modules& modules; const std::vector<Mapping>& maps; } context{modules, maps};
    ::dl_iterate_phdr([](dl_phdr_info* info, size_t, void* data) -> int {
        if (!info->dlpi_name || !info->dlpi_name[0]) return 0;
        Image image{info->dlpi_name, {}};
        for (size_t i = 0; i < info->dlpi_phnum; ++i) {
            const auto& segment = info->dlpi_phdr[i];
            if (segment.p_type == PT_LOAD && segment.p_filesz > 0) {
                image.segments.push_back({info->dlpi_addr + segment.p_vaddr, segment.p_offset});
            }
        }
        auto& context = *static_cast<Context*>(data);
        AddImage(context.modules, image, context.maps);
        return 0;
    }, &context);
#endif
    return modules;
}

} // namespace s2khook::runtime

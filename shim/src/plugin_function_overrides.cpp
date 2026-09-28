#include "plugin_function_overrides.h"
#include "sha256.h"
#include <json.hpp>
#include <algorithm>
#include <array>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <dirent.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <filesystem>
#include <memory>
#include <stdexcept>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>
namespace s2::function_overrides {
namespace {
using nlohmann::json;
struct Fd {
    int value;
    explicit Fd(int v) : value(v) {}
    ~Fd() { if (value >= 0) close(value); }
    Fd(const Fd&) = delete;
    Fd& operator=(const Fd&) = delete;
};
[[noreturn]] void fail(const std::string& message) { throw std::runtime_error(message); }
using s2::digest::sha256;
bool same(const struct stat& a,const struct stat& b){
#if defined(__APPLE__)
    return a.st_dev==b.st_dev&&a.st_ino==b.st_ino&&a.st_size==b.st_size&&a.st_mtimespec.tv_sec==b.st_mtimespec.tv_sec&&a.st_mtimespec.tv_nsec==b.st_mtimespec.tv_nsec&&a.st_ctimespec.tv_sec==b.st_ctimespec.tv_sec&&a.st_ctimespec.tv_nsec==b.st_ctimespec.tv_nsec;
#else
    return a.st_dev==b.st_dev&&a.st_ino==b.st_ino&&a.st_size==b.st_size&&a.st_mtim.tv_sec==b.st_mtim.tv_sec&&a.st_mtim.tv_nsec==b.st_mtim.tv_nsec&&a.st_ctim.tv_sec==b.st_ctim.tv_sec&&a.st_ctim.tv_nsec==b.st_ctim.tv_nsec;
#endif
}
int directory(int parent, const std::string& name, bool missing) {
    int fd = openat(parent, name.c_str(), O_RDONLY | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW);
    if (fd < 0 && !(missing && errno == ENOENT)) {
        fail("cannot safely open directory " + name + ": " + std::strerror(errno));
    }
    return fd;
}
} // namespace

std::string PluginDirectory(const std::string& id) {
    static constexpr char table[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    std::string out = "id-";
    uint32_t value = 0;
    unsigned bits = 0;
    for (unsigned char c : id) {
        value = (value << 8) | c;
        bits += 8;
        while (bits >= 6) {
            bits -= 6;
            out += table[(value >> bits) & 63];
        }
    }
    if (bits) out += table[(value << (6 - bits)) & 63];
    return out;
}

std::string Snapshot(const std::string& addonRoot, const std::string& id) {
    try {
        if (id.empty() || id.find('\0') != std::string::npos) fail("invalid owner id");
        auto encoded = PluginDirectory(id);
        if (encoded.size() > 255) fail("encoded plugin id exceeds filesystem component limit");
        // Validate UTF-8 before accessing files. No case folding or normalization.
        (void)json(id).dump();
        auto root = std::filesystem::absolute(addonRoot);
        Fd current(open("/", O_RDONLY | O_CLOEXEC | O_DIRECTORY));
        if (current.value < 0) fail("cannot open filesystem root");
        // Walk the entire path by directory descriptors: no ancestor can redirect through a symlink.
        for (const auto& part : root.relative_path()) {
            auto name = part.string();
            if (name == "." || name == ".." || name.empty()) fail("unsafe addon root");
            int next = directory(current.value, name, false);
            close(current.value);
            current.value = next;
        }
        json records = json::array();
        const std::string relative = "gamedata/plugins/" + encoded + "/custom";
        for (const auto& name : {std::string("gamedata"), std::string("plugins"), encoded, std::string("custom")}) {
            int next = directory(current.value, name, true);
            if (next < 0) return json{{"records", records}, {"error", nullptr}}.dump();
            close(current.value);
            current.value = next;
        }
        struct stat before{};
        if (fstat(current.value, &before) != 0) fail("cannot stat override directory");
        int scan = dup(current.value);
        if (scan < 0) fail("cannot duplicate directory handle");
        DIR* raw = fdopendir(scan);
        if (!raw) {
            close(scan);
            fail("cannot enumerate overrides");
        }
        std::unique_ptr<DIR, int(*)(DIR*)> entries(raw, closedir);
        std::vector<std::string> names;
        size_t count = 0;
        for (;;) {
            errno = 0;
            auto* entry = readdir(entries.get());
            if (!entry) {
                if (errno) fail("override enumeration failed");
                break;
            }
            std::string name = entry->d_name;
            if (name == "." || name == "..") continue;
            if (++count > MaxDirectoryEntries) fail("override directory entry limit");
            if (name.size() < 6 || name.substr(name.size() - 6) != ".jsonc") continue;
            if (names.size() >= MaxFiles) fail("override file count limit");
            names.push_back(name);
        }
        std::sort(names.begin(), names.end());
        size_t total = 0;
        for (const auto& name : names) {
            // NONBLOCK prevents a substituted FIFO/device from blocking before the regular-file gate.
            Fd file(openat(current.value, name.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK));
            if (file.value < 0) fail("cannot safely open override " + name);
            struct stat first{}, last{};
            if (fstat(file.value, &first) != 0 || !S_ISREG(first.st_mode)) {
                fail("override must be a regular file: " + name);
            }
            if (first.st_size < 0 || uint64_t(first.st_size) > MaxFileBytes) {
                fail("override file byte limit: " + name);
            }
            std::string content;
            char buffer[8192];
            for (;;) {
                ssize_t n = read(file.value, buffer, sizeof(buffer));
                if (n < 0) {
                    if (errno == EINTR) continue;
                    fail("override read failed: " + name);
                }
                if (n == 0) break;
                if (content.size() + size_t(n) > MaxFileBytes || total + size_t(n) > MaxTotalBytes) {
                    fail("override byte limit: " + name);
                }
                content.append(buffer, size_t(n));
                total += size_t(n);
            }
            if (fstat(file.value, &last) != 0 || !same(first, last) || content.size() != size_t(last.st_size)) {
                fail("override changed during snapshot: " + name);
            }
            records.push_back({{"relative_path", relative + "/" + name}, {"sha256", sha256(content)}, {"content", content}});
        }
        struct stat after{};
        if (fstat(current.value, &after) != 0 || !same(before, after)) {
            fail("override directory changed during snapshot");
        }
        return json{{"records", records}, {"error", nullptr}}.dump();
    } catch (const std::exception& e) {
        return json{{"records", json::array()}, {"error", e.what()}}.dump();
    }
}
} // namespace s2::function_overrides

const char* s2_plugin_function_overrides(const char* id) {
    static thread_local std::string result;
    try {
        Dl_info info{};
        if (!id || !dladdr(reinterpret_cast<void*>(&s2_plugin_function_overrides), &info) || !info.dli_fname) {
            result = R"({"records":[],"error":"addon root discovery failed"})";
        } else {
            // The shim is addons/s2script/bin/linuxsteamrt64/s2script.so.
            auto root = std::filesystem::path(info.dli_fname).parent_path().parent_path().parent_path();
            result = s2::function_overrides::Snapshot(root.string(), id);
        }
        return result.c_str();
    } catch (...) {
        return R"({"records":[],"error":"override snapshot failed"})";
    }
}

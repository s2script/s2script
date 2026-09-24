#include "plugin_function_overrides.h"
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
uint32_t rotate(uint32_t x, unsigned n) { return (x >> n) | (x << (32 - n)); }
// FIPS 180-4 SHA-256 over the exact file bytes; independent of JSON parsing/canonicalization.
std::string sha256(const std::string& text) {
    static constexpr uint32_t k[64] = {
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    };
    uint32_t h[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                     0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
    std::vector<uint8_t> bytes(text.begin(), text.end());
    uint64_t bits = bytes.size() * 8;
    bytes.push_back(0x80);
    while (bytes.size() % 64 != 56) bytes.push_back(0);
    for (int i = 7; i >= 0; --i) bytes.push_back(static_cast<uint8_t>(bits >> (i * 8)));
    for (size_t off = 0; off < bytes.size(); off += 64) {
        uint32_t w[64];
        for (size_t i = 0; i < 16; ++i) {
            w[i] = (uint32_t(bytes[off + 4*i]) << 24) | (uint32_t(bytes[off + 4*i + 1]) << 16)
                 | (uint32_t(bytes[off + 4*i + 2]) << 8) | bytes[off + 4*i + 3];
        }
        for (size_t i = 16; i < 64; ++i) {
            auto x = w[i-15], y = w[i-2];
            w[i] = w[i-16] + (rotate(x, 7) ^ rotate(x, 18) ^ (x >> 3)) + w[i-7]
                 + (rotate(y, 17) ^ rotate(y, 19) ^ (y >> 10));
        }
        uint32_t a=h[0], b=h[1], c=h[2], d=h[3], e=h[4], f=h[5], g=h[6], v=h[7];
        for (size_t i = 0; i < 64; ++i) {
            uint32_t t1 = v + (rotate(e, 6) ^ rotate(e, 11) ^ rotate(e, 25))
                        + ((e & f) ^ (~e & g)) + k[i] + w[i];
            uint32_t t2 = (rotate(a, 2) ^ rotate(a, 13) ^ rotate(a, 22))
                        + ((a & b) ^ (a & c) ^ (b & c));
            v=g; g=f; f=e; e=d+t1; d=c; c=b; b=a; a=t1+t2;
        }
        h[0]+=a; h[1]+=b; h[2]+=c; h[3]+=d; h[4]+=e; h[5]+=f; h[6]+=g; h[7]+=v;
    }
    std::string out;
    const char* hex = "0123456789abcdef";
    for (auto x : h) {
        for (int i = 7; i >= 0; --i) out += hex[(x >> (i * 4)) & 15];
    }
    return out;
}
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

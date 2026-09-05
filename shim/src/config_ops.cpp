#include "config_ops.h"

#include <dlfcn.h>
#include <libgen.h>

#include <cstdio>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>

// ---------------------------------------------------------------------------
// ConfigPath: resolve addons/s2script/configs/<sanitized id>.json via dladdr
// (mirrors PluginsDir).  Non-[A-Za-z0-9._-] chars in `id` are replaced with '_'.
// ---------------------------------------------------------------------------
static std::string ConfigPath(const char* id) {
    // Sanitize id: non-[A-Za-z0-9._-] → '_' (matches the CLI's .s2sp id sanitization).
    std::string safe_id;
    for (const char* p = id; *p; ++p) {
        char c = *p;
        if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
            || c == '.' || c == '_' || c == '-') {
            safe_id += c;
        } else {
            safe_id += '_';
        }
    }
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&ConfigPath), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir + "/configs/" + safe_id + ".json";
    }
    // Fallback: relative to the server's cwd.
    return "addons/s2script/configs/" + safe_id + ".json";
}

// Narrow loader ABI: transient storage is main-thread-only and core copies it immediately.
static std::string s_configPathResolverBuf;
const char* s2_config_path_resolve(const char* id) {
    if (!id) return nullptr;
    s_configPathResolverBuf = ConfigPath(id);
    return s_configPathResolverBuf.c_str();
}

// ---------------------------------------------------------------------------
// Config ops (Slice 5E.2): read/auto-write the admin override file.
// ---------------------------------------------------------------------------
static std::string s_configReadBuf;
const char* s2_config_read(const char* id) {
    if (!id) return nullptr;
    std::ifstream f(ConfigPath(id));
    if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf();
    s_configReadBuf = ss.str();
    return s_configReadBuf.c_str();
}
int s2_config_write(const char* id, const char* content) {
    if (!id || !content) return 0;
    std::string path = ConfigPath(id);
    std::error_code ec; std::filesystem::create_directories(std::filesystem::path(path).parent_path(), ec);
    std::ofstream f(path); if (!f) return 0; f << content; return f.good() ? 1 : 0;
}
// ConfigFilePath: like ConfigPath but the name INCLUDES its extension (no .json append). Reuses the same
// sanitize (non-[A-Za-z0-9._-] -> '_', which neutralizes '/'); additionally refuses names containing ".."
// or empty (returns "" -> read/write fail) so there is no traversal.
static std::string ConfigFilePath(const char* name) {
    if (!name || !*name) return "";
    if (std::string(name).find("..") != std::string::npos) return "";
    std::string safe;
    for (const char* p = name; *p; ++p) {
        char c = *p;
        safe += ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
                 || c == '.' || c == '_' || c == '-') ? c : '_';
    }
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&ConfigFilePath), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf); snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);             snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);
        return dir + "/configs/" + safe;
    }
    return "addons/s2script/configs/" + safe;
}
static std::string s_configFileReadBuf;
const char* s2_config_read_file(const char* name) {
    std::string path = ConfigFilePath(name);
    if (path.empty()) return nullptr;
    std::ifstream f(path); if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf(); s_configFileReadBuf = ss.str();
    return s_configFileReadBuf.c_str();
}
int s2_config_write_file(const char* name, const char* content) {
    std::string path = ConfigFilePath(name); if (path.empty() || !content) return 0;
    std::error_code ec; std::filesystem::create_directories(std::filesystem::path(path).parent_path(), ec);
    std::ofstream f(path); if (!f) return 0; f << content; return f.good() ? 1 : 0;
}

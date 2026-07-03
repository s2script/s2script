#include "gamedata.h"
#include "../third_party/json.hpp"
#include <fstream>

std::map<std::string, std::string> LoadInterfaceVersions(const std::string& path, std::string& error) {
    std::map<std::string, std::string> out;
    std::ifstream f(path);
    if (!f) {
        error = "gamedata file not found: " + path;
        return out;
    }
    try {
        // ignore_comments = true → JSONC support
        auto j = nlohmann::json::parse(f, nullptr, /*allow_exceptions=*/true, /*ignore_comments=*/true);
        for (auto& [k, v] : j.at("interfaces").items()) {
            out[k] = v.get<std::string>();
        }
    } catch (const std::exception& e) {
        error = std::string("gamedata parse error: ") + e.what();
        out.clear();
    }
    return out;
}

std::map<std::string, int> LoadOffsets(const std::string& path,
                                        const std::string& platform,
                                        std::string& error) {
    std::map<std::string, int> out;
    std::ifstream f(path);
    if (!f) {
        error = "gamedata file not found: " + path;
        return out;
    }
    try {
        auto j = nlohmann::json::parse(f, nullptr, /*allow_exceptions=*/true, /*ignore_comments=*/true);
        // "offsets" section is optional — not present is not an error.
        if (!j.contains("offsets")) return out;
        for (auto& [key, platforms] : j.at("offsets").items()) {
            if (platforms.contains(platform)) {
                out[key] = platforms.at(platform).get<int>();
            }
        }
    } catch (const std::exception& e) {
        error = std::string("gamedata parse error: ") + e.what();
        out.clear();
    }
    return out;
}

std::map<std::string, SigSpec> LoadSignatures(const std::string& path,
                                              const std::string& platform,
                                              std::string& error) {
    std::map<std::string, SigSpec> out;
    std::ifstream f(path);
    if (!f) {
        error = "gamedata file not found: " + path;
        return out;
    }
    try {
        auto j = nlohmann::json::parse(f, nullptr, /*allow_exceptions=*/true, /*ignore_comments=*/true);
        if (!j.contains("signatures")) return out;      // absent is not an error
        for (auto& [key, platforms] : j.at("signatures").items()) {
            if (!platforms.contains(platform)) continue;
            auto& p = platforms.at(platform);
            SigSpec s;
            s.module  = p.value("module", "");
            s.pattern = p.value("pattern", "");
            s.resolve = p.value("resolve", "");
            out[key] = s;
        }
    } catch (const std::exception& e) {
        error = std::string("gamedata parse error: ") + e.what();
        out.clear();
    }
    return out;
}

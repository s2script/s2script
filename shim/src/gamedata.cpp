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

#include "gamedata.h"
#include "../third_party/json.hpp"
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <vector>

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

namespace {

// Does a master entry's condition field match? Absent condition = matches. Value may be a string
// or an array of strings (SourceMod's repeated-key idiom; JSON has no duplicate keys).
bool ConditionMatches(const nlohmann::json& entry, const char* field, const std::string& actual) {
    if (!entry.contains(field)) return true;
    const auto& v = entry.at(field);
    if (v.is_string()) return v.get<std::string>() == actual;
    if (v.is_array()) {
        for (const auto& e : v)
            if (e.is_string() && e.get<std::string>() == actual) return true;
        return false;
    }
    return false;
}

// Merge one file's sections into `gc`. `isOverride` marks touched entries as operator-supplied.
// Replacement is at the named-entry level: assigning over the map key drops the old value whole.
void MergeFile(const nlohmann::json& j, const std::string& platform, bool isOverride,
               GameConfig& gc) {
    auto mark = [&](const std::string& name) { if (isOverride) gc.overridden.insert(name); };

    if (j.contains("interfaces"))
        for (auto& [k, v] : j.at("interfaces").items())
            if (v.is_string()) { gc.interfaces[k] = v.get<std::string>(); mark(k); }

    if (j.contains("offsets"))
        for (auto& [k, platforms] : j.at("offsets").items())
            if (platforms.contains(platform)) {
                gc.offsets[k] = platforms.at(platform).get<int>();
                mark(k);
            }

    if (j.contains("signatures"))
        for (auto& [k, platforms] : j.at("signatures").items())
            if (platforms.contains(platform)) {
                const auto& p = platforms.at(platform);
                SigSpec s;
                s.module  = p.value("module", "");
                s.pattern = p.value("pattern", "");
                s.resolve = p.value("resolve", "");
                gc.signatures[k] = s;
                mark(k);
            }

    if (j.contains("keys"))
        for (auto& [k, v] : j.at("keys").items())
            if (v.is_string()) { gc.keys[k] = v.get<std::string>(); mark(k); }
}

// Parse one JSONC file. Returns false and sets `error` on read/parse failure.
bool ParseFile(const std::filesystem::path& p, nlohmann::json& out, std::string& error) {
    std::ifstream f(p);
    if (!f) { error = "gamedata file not found: " + p.string(); return false; }
    try {
        out = nlohmann::json::parse(f, nullptr, /*allow_exceptions=*/true, /*ignore_comments=*/true);
    } catch (const std::exception& e) {
        error = "gamedata parse error in " + p.string() + ": " + e.what();
        return false;
    }
    return true;
}

}  // namespace

GameConfig LoadGameConfig(const std::string& gamedataRoot,
                          const std::string& owner,
                          const std::string& engine,
                          const std::string& game,
                          const std::string& platform,
                          std::string& error) {
    namespace fs = std::filesystem;
    GameConfig gc;
    error.clear();

    const fs::path ownerDir = fs::path(gamedataRoot) / owner;
    const fs::path masterPath = ownerDir / "master.gamedata.jsonc";

    nlohmann::json master;
    if (!ParseFile(masterPath, master, error)) {
        // A missing/broken master is a NAMED hard error for this owner — never a silent empty
        // namespace. ParseFile already set `error` naming the master path.
        return gc;
    }
    if (!master.contains("files") || !master.at("files").is_array()) {
        error = "gamedata master has no \"files\" array: " + masterPath.string();
        return gc;
    }

    // Shipped files, in ARRAY order. Array, not object: nlohmann::json's object type is std::map,
    // so object keys come back alphabetically sorted and apply order would be decided by filename
    // spelling. Array order is the apply order, full stop.
    for (const auto& entry : master.at("files")) {
        if (!entry.contains("file") || !entry.at("file").is_string()) {
            error = "gamedata master entry without a \"file\": " + masterPath.string();
            return gc;
        }
        const std::string name = entry.at("file").get<std::string>();
        if (!ConditionMatches(entry, "engine", engine)) continue;
        if (!ConditionMatches(entry, "game", game)) continue;

        nlohmann::json j;
        if (!ParseFile(ownerDir / name, j, error)) return gc;
        MergeFile(j, platform, /*isOverride=*/false, gc);
        gc.filesLoaded.push_back(name);
    }

    // Operator overrides, applied LAST, in sorted filename order for determinism (SourceMod reads
    // the directory in OS order; sorted is the same idea with a reproducible result). An absent
    // custom/ directory is the normal case and is not an error.
    const fs::path customDir = ownerDir / "custom";
    std::error_code ec;
    if (fs::is_directory(customDir, ec)) {
        std::vector<fs::path> customFiles;
        for (const auto& de : fs::directory_iterator(customDir, ec)) {
            if (!de.is_regular_file()) continue;
            const std::string ext = de.path().extension().string();
            if (ext != ".jsonc" && ext != ".json") continue;
            customFiles.push_back(de.path());
        }
        std::sort(customFiles.begin(), customFiles.end());
        for (const auto& p : customFiles) {
            nlohmann::json j;
            if (!ParseFile(p, j, error)) return gc;
            MergeFile(j, platform, /*isOverride=*/true, gc);
            gc.filesLoaded.push_back("custom/" + p.filename().string());
        }
    }

    return gc;
}

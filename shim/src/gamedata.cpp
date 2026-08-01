#include "gamedata.h"
#include "../third_party/json.hpp"
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <vector>

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
//
// A single wrongly-typed entry (e.g. a quoted number where an offset wants an int, or a scalar
// where a signature wants an object) must degrade THAT ENTRY ONLY, never the whole file or the
// whole load — "degrade per-descriptor, never crash globally". Each entry's conversion is
// therefore its own try/catch: on failure the entry is left out of `gc` and `error` is set to a
// reason naming the section and key, but the loop moves on to the next entry.
void MergeFile(const nlohmann::json& j, const std::string& platform, bool isOverride,
               GameConfig& gc, std::string& error) {
    auto mark = [&](const std::string& name) { if (isOverride) gc.overridden.insert(name); };

    if (j.contains("interfaces"))
        for (auto& [k, v] : j.at("interfaces").items()) {
            if (v.is_string()) { gc.interfaces[k] = v.get<std::string>(); mark(k); }
            else error = "gamedata interfaces." + k + " has the wrong type (expected a string)";
        }

    if (j.contains("offsets"))
        for (auto& [k, platforms] : j.at("offsets").items())
            if (platforms.contains(platform)) {
                try {
                    gc.offsets[k] = platforms.at(platform).get<int>();
                    mark(k);
                } catch (const std::exception& e) {
                    error = "gamedata offsets." + k + " has the wrong type for platform \"" +
                            platform + "\": " + e.what();
                }
            }

    if (j.contains("signatures"))
        for (auto& [k, platforms] : j.at("signatures").items())
            if (platforms.contains(platform)) {
                try {
                    const auto& p = platforms.at(platform);
                    SigSpec s;
                    s.module  = p.value("module", "");
                    s.pattern = p.value("pattern", "");
                    s.resolve = p.value("resolve", "");
                    gc.signatures[k] = s;
                    mark(k);
                } catch (const std::exception& e) {
                    error = "gamedata signatures." + k + " has the wrong type for platform \"" +
                            platform + "\": " + e.what();
                }
            }

    if (j.contains("keys"))
        for (auto& [k, v] : j.at("keys").items()) {
            if (v.is_string()) { gc.keys[k] = v.get<std::string>(); mark(k); }
            else error = "gamedata keys." + k + " has the wrong type (expected a string)";
        }
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
        MergeFile(j, platform, /*isOverride=*/false, gc, error);
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
            // custom/ is operator-writable: a dangling symlink or a file removed mid-iteration
            // must not crash the process. The error_code overload reports the stat failure
            // instead of throwing; treat "couldn't tell" the same as "not a regular file".
            std::error_code fileEc;
            bool isRegular = de.is_regular_file(fileEc);
            if (fileEc || !isRegular) continue;
            const std::string ext = de.path().extension().string();
            if (ext != ".jsonc" && ext != ".json") continue;
            customFiles.push_back(de.path());
        }
        // The directory_iterator constructor above was also given `ec`; a failure opening/
        // reading the directory (permission change after the is_directory check, etc.) leaves it
        // silently short of entries unless we check here — surface it as a named, non-fatal
        // reason rather than a quietly-incomplete override set.
        if (ec) {
            error = "gamedata custom overrides directory could not be fully read: " +
                    customDir.string() + ": " + ec.message();
        }
        std::sort(customFiles.begin(), customFiles.end());
        for (const auto& p : customFiles) {
            nlohmann::json j;
            if (!ParseFile(p, j, error)) return gc;
            MergeFile(j, platform, /*isOverride=*/true, gc, error);
            gc.filesLoaded.push_back("custom/" + p.filename().string());
        }
    }

    return gc;
}

#include "gamedata.h"
#include "../third_party/json.hpp"
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <vector>

namespace {

// Does a master entry's condition field match? Absent condition = matches. Value may be a string
// or an array of strings (SourceMod's repeated-key idiom; JSON has no duplicate keys).
//
// A condition of any OTHER type (`"game": 123`) can never match, so the file it guards would be
// deselected — silently, and identically to a deliberate non-match, which is the worst possible
// way to lose a whole file of engine facts. Every non-matching shape therefore sets a NAMED
// `error` before returning false. A genuine non-match (a well-formed condition for another game)
// leaves `error` untouched.
bool ConditionMatches(const nlohmann::json& entry, const char* field, const std::string& actual,
                      const std::string& file, std::string& error) {
    if (!entry.contains(field)) return true;
    const auto& v = entry.at(field);
    if (v.is_string()) return v.get<std::string>() == actual;
    if (v.is_array()) {
        bool matched = false;
        for (const auto& e : v) {
            if (!e.is_string()) {
                error = "gamedata master entry for " + file + " has a non-string member in its \"" +
                        std::string(field) + "\" condition — it can never match";
                continue;
            }
            if (e.get<std::string>() == actual) matched = true;
        }
        return matched;
    }
    error = "gamedata master entry for " + file + " has a \"" + std::string(field) +
            "\" condition that is neither a string nor an array of strings — the file is skipped";
    return false;
}

// Does this dumped `validate` text declare an ACTUAL validator? "" / `null` / `{}` are all the
// "no semantic gate" spellings (call_validate.cpp's ParseValidate treats them identically), and the
// override-inheritance rule below has to tell "declares one" from "declares none" without knowing a
// single validator NAME — the vocabulary is closed in call_validate.cpp and stays there.
bool DeclaresValidator(const std::string& dumped) {
    if (dumped.empty()) return false;
    auto v = nlohmann::json::parse(dumped, nullptr, /*allow_exceptions=*/false);
    return !v.is_discarded() && v.is_object() && !v.empty();
}

// Merge one file's sections into `gc`. `isOverride` marks touched entries as operator-supplied.
// Replacement is at the named-entry level: assigning over the map key drops the old value whole.
// ONE EXCEPTION, in the signatures branch below: a custom/ override that omits `validate` inherits
// the previous entry's rather than deleting the semantic gate — see SigSpec::validate.
// Returns the number of entries actually applied — zero from a file that parsed is itself a
// reportable condition (see GameConfig::filesEmpty).
//
// A single wrongly-typed entry (e.g. a quoted number where an offset wants an int, or a scalar
// where a signature wants an object) must degrade THAT ENTRY ONLY, never the whole file or the
// whole load — "degrade per-descriptor, never crash globally". Each entry's conversion is
// therefore its own try/catch: on failure the entry is left out of `gc` and `error` is set to a
// reason naming the section and key, but the loop moves on to the next entry.
//
// The `offsets`/`signatures` sections are PLATFORM-KEYED. A value written in the flat shape an
// operator reaches for first — `"offsets": { "Shipped": 99 }` instead of
// `"offsets": { "Shipped": { "linuxsteamrt64": 99 } }` — has no `platform` member, so the
// entry would simply be skipped: no error, no `overridden` mark, and the STALE shipped value
// still in use behind a banner that says the override file loaded. That is the project's worst
// failure mode (a stale value reading garbage while reporting success) on the one surface a
// server admin hand-edits under time pressure. Reject a non-object by NAME instead.
size_t MergeFile(const nlohmann::json& j, const std::string& platform, bool isOverride,
                 GameConfig& gc, std::string& error) {
    size_t applied = 0;
    auto mark = [&](const std::string& name) { applied++; if (isOverride) gc.overridden.insert(name); };

    if (j.contains("interfaces"))
        for (auto& [k, v] : j.at("interfaces").items()) {
            if (v.is_string()) { gc.interfaces[k] = v.get<std::string>(); mark(k); }
            else error = "gamedata interfaces." + k + " has the wrong type (expected a string)";
        }

    if (j.contains("offsets"))
        for (auto& [k, platforms] : j.at("offsets").items()) {
            if (!platforms.is_object()) {
                error = "gamedata offsets." + k + " is not a platform-keyed object (expected "
                        "{ \"" + platform + "\": <int> })";
                continue;
            }
            if (!platforms.contains(platform)) continue;
            try {
                gc.offsets[k] = platforms.at(platform).get<int>();
                mark(k);
            } catch (const std::exception& e) {
                error = "gamedata offsets." + k + " has the wrong type for platform \"" +
                        platform + "\": " + e.what();
            }
        }

    if (j.contains("signatures"))
        for (auto& [k, platforms] : j.at("signatures").items()) {
            if (!platforms.is_object()) {
                error = "gamedata signatures." + k + " is not a platform-keyed object (expected "
                        "{ \"" + platform + "\": { \"module\": …, \"pattern\": … } })";
                continue;
            }
            if (!platforms.contains(platform)) continue;
            try {
                const auto& p = platforms.at(platform);
                SigSpec s;
                s.module  = p.value("module", "");
                s.pattern = p.value("pattern", "");
                s.resolve = p.value("resolve", "");

                // THE ONE FIELD AN OVERRIDE DOES NOT REPLACE BY OMISSION — see SigSpec::validate for
                // why. `validate` crosses verbatim and unparsed; an entry with no validator leaves
                // it empty, which is what "declares none" means everywhere downstream.
                //
                // Shipped-tier merges keep the plain named-entry replacement semantics: our own
                // files are reviewed, and a later shipped file dropping a validator is a code
                // review's problem, not a live server's. The inheritance is scoped to `isOverride`
                // precisely because custom/ is the one tier nobody reviews.
                const auto prevIt = gc.signatures.find(k);
                const bool prevHadValidator =
                    prevIt != gc.signatures.end() && DeclaresValidator(prevIt->second.validate);
                if (p.contains("validate")) {
                    s.validate = p.at("validate").dump();
                    // An EXPLICIT empty value is the operator saying "no gate, I mean it". Honoured,
                    // but never silently: it is a banner line of its own.
                    if (isOverride && prevHadValidator && !DeclaresValidator(s.validate))
                        gc.validatorsDisarmed.push_back(k);
                } else if (isOverride && prevHadValidator) {
                    // Carried, not dropped. Copied out of the previous entry here, before the
                    // assignment below overwrites it.
                    s.validate = prevIt->second.validate;
                    gc.validatorsCarried.push_back(k);
                }
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

    // `calls` is NOT platform-keyed at the entry level: a descriptor's platform-specific details
    // sit one level down (`target[platform]` for a vtable slot; the signature it names for a byte
    // pattern), and core's flatten step lifts them. The entry crosses this loader verbatim — the
    // only thing checked here is that it IS an object, because a scalar could never be a
    // descriptor and would otherwise reach core as an unexplained "malformed descriptor".
    if (j.contains("calls"))
        for (auto& [k, v] : j.at("calls").items()) {
            if (v.is_object()) { gc.calls[k] = v.dump(); mark(k); }
            else error = "gamedata calls." + k + " has the wrong type (expected an object)";
        }

    return applied;
}

// Re-serialise the merged view into the JSON text core consumes (GameConfig::mergedJson).
//
// `signatures` is re-NESTED under the platform key its details were lifted from: core's
// `flatten_decl` reads `signatures[name][platform]`, and that platform id is core's own constant.
// If the two ever diverge, every signature-targeted descriptor degrades with core's named
// "no '<platform>' entry" reason — loud, per-descriptor, never a silent wrong resolve.
std::string SerializeMerged(const GameConfig& gc, const std::string& platform) {
    if (gc.signatures.empty() && gc.calls.empty()) return std::string();
    nlohmann::json doc = nlohmann::json::object();
    if (!gc.signatures.empty()) {
        nlohmann::json sigs = nlohmann::json::object();
        for (const auto& [name, s] : gc.signatures) {
            nlohmann::json entry{
                {"module", s.module}, {"pattern", s.pattern}, {"resolve", s.resolve}};
            // The semantic gate travels WITH the pattern (see SigSpec::validate). Text this file
            // dumped from an already-parsed object, so the re-parse cannot fail — but a discarded
            // parse must degrade THIS entry's validator loudly rather than emit `"validate": null`,
            // which core would hand the shim as a descriptor with no gate.
            if (!s.validate.empty()) {
                auto v = nlohmann::json::parse(s.validate, nullptr, /*allow_exceptions=*/false);
                if (!v.is_discarded()) entry["validate"] = std::move(v);
            }
            sigs[name][platform] = std::move(entry);
        }
        doc["signatures"] = std::move(sigs);
    }
    if (!gc.calls.empty()) {
        nlohmann::json calls = nlohmann::json::object();
        for (const auto& [name, text] : gc.calls) {
            // Text this file dumped from an already-parsed object, so a re-parse cannot fail —
            // but degrade THAT ENTRY rather than throw out of the loader if it ever did.
            auto v = nlohmann::json::parse(text, nullptr, /*allow_exceptions=*/false);
            if (v.is_discarded()) continue;
            calls[name] = std::move(v);
        }
        doc["calls"] = std::move(calls);
    }
    return doc.dump();
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

// The merge itself. Wrapped by LoadGameConfig below, which serialises the result exactly once —
// this function has five early returns and a per-return `mergedJson` build is a missed one waiting
// to happen (a degraded owner would hand core an EMPTY string that reads as "no descriptors").
GameConfig MergeOwner(const std::string& gamedataRoot,
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

    // Every abort below records the master in filesFailed, so the invariant the shim gates on is
    // uniform: filesFailed non-empty <=> something shipped was selected and could not be applied.
    const std::string masterName = "master.gamedata.jsonc";

    nlohmann::json master;
    if (!ParseFile(masterPath, master, error)) {
        // A missing/broken master is a NAMED hard error for this owner — never a silent empty
        // namespace. ParseFile already set `error` naming the master path.
        gc.filesFailed.push_back(masterName);
        return gc;
    }
    if (!master.contains("files") || !master.at("files").is_array()) {
        error = "gamedata master has no \"files\" array: " + masterPath.string();
        gc.filesFailed.push_back(masterName);
        return gc;
    }

    // Shipped files, in ARRAY order. Array, not object: nlohmann::json's object type is std::map,
    // so object keys come back alphabetically sorted and apply order would be decided by filename
    // spelling. Array order is the apply order, full stop.
    for (const auto& entry : master.at("files")) {
        if (!entry.contains("file") || !entry.at("file").is_string()) {
            error = "gamedata master entry without a \"file\": " + masterPath.string();
            gc.filesFailed.push_back(masterName);
            return gc;
        }
        const std::string name = entry.at("file").get<std::string>();
        if (!ConditionMatches(entry, "engine", engine, name, error)) continue;
        if (!ConditionMatches(entry, "game", game, name, error)) continue;

        nlohmann::json j;
        if (!ParseFile(ownerDir / name, j, error)) {
            // Selected but unapplicable: record it before returning, so the caller can tell this
            // (our shipped tree is broken) from a merely malformed entry. Returning here keeps the
            // pre-existing fail-fast behaviour for the shipped tier.
            gc.filesFailed.push_back(name);
            return gc;
        }
        if (MergeFile(j, platform, /*isOverride=*/false, gc, error) == 0)
            gc.filesEmpty.push_back(name);
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
            const std::string label = "custom/" + p.filename().string();
            nlohmann::json j;
            // Deliberately NOT recorded in filesFailed: the shipped tree is intact, so a typo in
            // an operator's hot-fix must not disable the engine surface. It is a named `error`,
            // which the boot banner reports.
            if (!ParseFile(p, j, error)) return gc;
            if (MergeFile(j, platform, /*isOverride=*/true, gc, error) == 0)
                gc.filesEmpty.push_back(label);
            gc.filesLoaded.push_back(label);
        }
    }

    return gc;
}

}  // namespace

GameConfig LoadGameConfig(const std::string& gamedataRoot,
                          const std::string& owner,
                          const std::string& engine,
                          const std::string& game,
                          const std::string& platform,
                          std::string& error) {
    GameConfig gc = MergeOwner(gamedataRoot, owner, engine, game, platform, error);
    // Serialised on EVERY path, including a degraded one: whatever merged before the failure is
    // what the owner's consumers get, and a partially-merged view must degrade per-descriptor
    // downstream rather than silently arrive as "this owner declared nothing".
    gc.mergedJson = SerializeMerged(gc, platform);
    return gc;
}

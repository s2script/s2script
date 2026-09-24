#include "gamedata.h"
#include "../third_party/json.hpp"
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <functional>
#include <set>
#include <stdexcept>
#include <unordered_map>
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
                 const std::string& fileLabel, GameConfig& gc, std::string& error) {
    size_t applied = 0;
    auto mark = [&](const std::string& name) { applied++; if (isOverride) gc.overridden.insert(name); };

    // A present but scalar section must be visible. Otherwise .items() can turn it into a
    // successful empty file, hiding an entire authored section behind filesEmpty alone.
    for (const char* section : {"interfaces", "offsets", "signatures", "keys", "calls", "hooks"})
        if (j.contains(section) && !j.at(section).is_object())
            error = "gamedata " + fileLabel + " section " + section + " has the wrong type (expected an object)";

    if (j.contains("interfaces") && j.at("interfaces").is_object())
        for (auto& [k, v] : j.at("interfaces").items()) {
            if (v.is_string()) { gc.interfaces[k] = v.get<std::string>(); mark(k); }
            else error = "gamedata interfaces." + k + " has the wrong type (expected a string)";
        }

    if (j.contains("offsets") && j.at("offsets").is_object())
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

    if (j.contains("signatures") && j.at("signatures").is_object())
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

    if (j.contains("keys") && j.at("keys").is_object())
        for (auto& [k, v] : j.at("keys").items()) {
            if (v.is_string()) { gc.keys[k] = v.get<std::string>(); mark(k); }
            else error = "gamedata keys." + k + " has the wrong type (expected a string)";
        }

    // `calls` is NOT platform-keyed at the entry level: a descriptor's platform-specific details
    // sit one level down (`target[platform]` for a vtable slot; the signature it names for a byte
    // pattern), and core's flatten step lifts them. The entry crosses this loader verbatim — the
    // only thing checked here is that it IS an object, because a scalar could never be a
    // descriptor and would otherwise reach core as an unexplained "malformed descriptor".
    if (j.contains("calls") && j.at("calls").is_object())
        for (auto& [k, v] : j.at("calls").items()) {
            if (v.is_object()) { gc.calls[k] = v.dump(); mark(k); }
            else error = "gamedata calls." + k + " has the wrong type (expected an object)";
        }

    // `hooks` — an INBOUND descriptor (a declared engine detour). Carried verbatim, exactly like
    // `calls`: same reason (the grammar is core's), same entry-level replacement, same is-it-an-
    // object check and nothing more.
    if (j.contains("hooks") && j.at("hooks").is_object())
        for (auto& [k, v] : j.at("hooks").items()) {
            if (v.is_object()) { gc.hooks[k] = v.dump(); mark(k); }
            else error = "gamedata hooks." + k + " has the wrong type (expected an object)";
        }

    // WHAT THIS MERGE DID NOT READ. Everything above is opt-in by name, so a section this loader
    // has no branch for simply evaporates — which is how `hooks` shipped inert: authored, validated
    // on disk, consumed by core, and dropped here without a word. Naming the leftovers turns the
    // next such omission into a boot line instead of a live-server mystery.
    //
    // Deliberately a report, not an error: an older shim reading a newer tree is a legitimate
    // downgrade, and it must degrade loudly rather than refuse to boot.
    // `is_object()` guarded: `it.key()` THROWS on a non-object root, and a gamedata file whose top
    // level is an array or a scalar is a legal parse. Every section branch above already no-ops on
    // one (`contains` is false), so this must too.
    if (j.is_object())
        for (auto it = j.begin(); it != j.end(); ++it) {
            const std::string& k = it.key();
            if (k == "interfaces" || k == "offsets" || k == "signatures" || k == "keys" ||
                k == "calls" || k == "hooks")
                continue;
            gc.sectionsIgnored.push_back(fileLabel + ": " + k);
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
    // Widened for `hooks`: an owner declaring ONLY hooks (no signatures, no calls) is a legitimate
    // shape — an inbound-only game package — and returning an empty string for it would hand core
    // "this owner declared nothing", which is the same silent nothing the missing `hooks` member
    // itself produced.
    if (gc.signatures.empty() && gc.calls.empty() && gc.hooks.empty()) return std::string();
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
    // `hooks`, on the same terms as `calls`: verbatim, entry by entry, degrading one entry rather
    // than throwing out of the loader. Core's `gamedata_hooks::register_owner` reads this key; a
    // build that emits everything BUT this one registers no detours at all and says so only on a
    // live server, which is exactly what happened before this block existed.
    if (!gc.hooks.empty()) {
        nlohmann::json hooks = nlohmann::json::object();
        for (const auto& [name, text] : gc.hooks) {
            auto v = nlohmann::json::parse(text, nullptr, /*allow_exceptions=*/false);
            if (v.is_discarded()) continue;
            hooks[name] = std::move(v);
        }
        doc["hooks"] = std::move(hooks);
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
using DocumentReader = std::function<bool(const std::string&, nlohmann::json&, std::string&)>;

GameConfig MergeOwner(const std::string& gamedataRoot,
                      const std::string& owner,
                      const std::string& engine,
                      const std::string& game,
                      const std::string& platform,
                      const DocumentReader& readShipped,
                      PackageGamedataProvenance* provenance,
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
    if (!readShipped(masterName, master, error)) {
        // A missing/broken master is a NAMED hard error for this owner — never a silent empty
        // namespace. ParseFile already set `error` naming the master path.
        gc.filesFailed.push_back(masterName);
        return gc;
    }
    if (!master.is_object() || !master.contains("files") || !master.at("files").is_array()) {
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
        if (!readShipped(name, j, error)) {
            // Selected but unapplicable: record it before returning, so the caller can tell this
            // (our shipped tree is broken) from a merely malformed entry. Returning here keeps the
            // pre-existing fail-fast behaviour for the shipped tier.
            gc.filesFailed.push_back(name);
            return gc;
        }
        if (MergeFile(j, platform, /*isOverride=*/false, name, gc, error) == 0)
            gc.filesEmpty.push_back(name);
        gc.filesLoaded.push_back(name);
        if (provenance) {
            provenance->shippedPaths.push_back(name);
            provenance->appliedPaths.push_back(name);
        }
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
            if (MergeFile(j, platform, /*isOverride=*/true, label, gc, error) == 0)
                gc.filesEmpty.push_back(label);
            gc.filesLoaded.push_back(label);
            if (provenance) {
                provenance->customPaths.push_back(label);
                provenance->appliedPaths.push_back(label);
            }
        }
    }

    return gc;
}

// The artifact builder uses confined normalized relative names. Recheck the envelope at the
// consumption boundary: a malformed or mismatched verified artifact must not select another file.
bool SafeEmbeddedPath(const std::string& path) {
    if (path.empty() || path.size() > 240 || path.front() == '/' ||
        path.find('\\') != std::string::npos || path.find(':') != std::string::npos ||
        path.find('\0') != std::string::npos) return false;
    size_t start = 0;
    while (start < path.size()) {
        size_t end = path.find('/', start);
        if (end == std::string::npos) end = path.size();
        const auto part = path.substr(start, end - start);
        if (part.empty() || part == "." || part == "..") return false;
        start = end + 1;
    }
    return path.back() != '/';
}

bool ValidMasterCondition(const nlohmann::json& entry, const char* field) {
    if (!entry.contains(field)) return true;
    const auto& value = entry.at(field);
    if (value.is_string()) return true;
    if (!value.is_array()) return false;
    return std::all_of(value.begin(), value.end(),
                       [](const nlohmann::json& item) { return item.is_string(); });
}

GameConfig BundleFailure(const std::string& file, const std::string& reason,
                         const PackageGamedataProvenance& provenance, std::string& error) {
    GameConfig gc;
    gc.packageProvenance = provenance;
    gc.filesFailed.push_back(file);
    error = "gamedata package " + file + ": " + reason;
    return gc;
}

}  // namespace

GameConfig LoadGameConfig(const std::string& gamedataRoot,
                          const std::string& owner,
                          const std::string& engine,
                          const std::string& game,
                          const std::string& platform,
                          std::string& error) {
    const auto reader = [&](const std::string& name, nlohmann::json& doc, std::string& failure) {
        return ParseFile(std::filesystem::path(gamedataRoot) / owner / name, doc, failure);
    };
    GameConfig gc = MergeOwner(gamedataRoot, owner, engine, game, platform, reader, nullptr, error);
    // Serialised on EVERY path, including a degraded one: whatever merged before the failure is
    // what the owner's consumers get, and a partially-merged view must degrade per-descriptor
    // downstream rather than silently arrive as "this owner declared nothing".
    gc.mergedJson = SerializeMerged(gc, platform);
    return gc;
}

GameConfig LoadGameConfigFromBundle(const std::string& verifiedBundleJson,
                                    const std::string& owner,
                                    const std::string& gamedataRoot,
                                    const std::string& engine,
                                    const std::string& game,
                                    const std::string& platform,
                                    const std::string& verifiedSha256,
                                    std::string& error) {
    constexpr size_t kMaxBytes = 4 * 1024 * 1024;
    constexpr size_t kMaxFiles = 128;
    PackageGamedataProvenance provenance{owner, engine, game, platform, verifiedSha256, {}, {}, {}};
    if (verifiedBundleJson.size() > kMaxBytes)
        return BundleFailure("gamedata.json", "bundle exceeds 4 MiB", provenance, error);
    if (owner.empty() || owner.find_first_not_of("abcdefghijklmnopqrstuvwxyz0123456789-") != std::string::npos)
        return BundleFailure("gamedata.json", "invalid selected owner", provenance, error);
    if (verifiedSha256.size() != 64 || verifiedSha256.find_first_not_of("0123456789abcdef") != std::string::npos)
        return BundleFailure("gamedata.json", "invalid verified SHA-256 identity", provenance, error);

    // nlohmann::json's default object parser overwrites duplicate keys. Reject them at parse time
    // so an ambiguous schemaVersion/owner/path/document cannot be silently selected.
    bool duplicateKey = false;
    std::vector<std::set<std::string>> objectKeys;
    // Reject recursion at container start, before the parser can construct/copy a hostile tree.
    // The v1 artifact needs only shallow master/layout documents; 128 containers is generous for
    // ordinary nested call/hook descriptors while bounding parser, copy and destructor stack use.
    constexpr int kMaxContainerDepth = 128;
    const auto callback = [&](int depth, nlohmann::json::parse_event_t event, nlohmann::json& value) {
        if ((event == nlohmann::json::parse_event_t::object_start ||
             event == nlohmann::json::parse_event_t::array_start) && depth >= kMaxContainerDepth)
            throw std::runtime_error("JSON nesting exceeds 128 containers");
        if (event == nlohmann::json::parse_event_t::object_start) objectKeys.emplace_back();
        else if (event == nlohmann::json::parse_event_t::object_end) objectKeys.pop_back();
        else if (event == nlohmann::json::parse_event_t::key &&
                 !objectKeys.back().insert(value.get<std::string>()).second) duplicateKey = true;
        return true;
    };
    nlohmann::json envelope;
    try { envelope = nlohmann::json::parse(verifiedBundleJson, callback); }
    catch (const std::exception& e) {
        return BundleFailure("gamedata.json", std::string("invalid JSON: ") + e.what(), provenance, error);
    }
    if (duplicateKey)
        return BundleFailure("gamedata.json", "duplicate JSON object key", provenance, error);
    if (!envelope.is_object() || envelope.size() != 3 ||
        !envelope.contains("schemaVersion") || !envelope.at("schemaVersion").is_number_integer() ||
        envelope.at("schemaVersion") != 1 || !envelope.contains("owner") ||
        !envelope.at("owner").is_string() || envelope.at("owner") != owner ||
        !envelope.contains("files") || !envelope.at("files").is_array())
        return BundleFailure("gamedata.json", "invalid v1 envelope or owner mismatch", provenance, error);

    const auto& files = envelope.at("files");
    if (files.empty() || files.size() > kMaxFiles)
        return BundleFailure("gamedata.json", "embedded file count outside 1..128", provenance, error);
    std::unordered_map<std::string, nlohmann::json> documents;
    documents.reserve(files.size());
    for (const auto& entry : files) {
        if (!entry.is_object() || entry.size() != 2 || !entry.contains("path") ||
            !entry.at("path").is_string() || !entry.contains("document"))
            return BundleFailure("gamedata.json", "invalid embedded file record", provenance, error);
        const auto name = entry.at("path").get<std::string>();
        if (!SafeEmbeddedPath(name))
            return BundleFailure(name, "unsafe embedded path", provenance, error);
        if (!documents.emplace(name, entry.at("document")).second)
            return BundleFailure(name, "duplicate embedded path", provenance, error);
    }
    const std::string masterName = "master.gamedata.jsonc";
    if (documents.count(masterName) != 1)
        return BundleFailure(masterName, "exactly one master is required", provenance, error);
    const auto& master = documents.at(masterName);
    if (!master.is_object() || !master.contains("files") || !master.at("files").is_array())
        return BundleFailure(masterName, "master has no files array", provenance, error);
    if (master.at("files").size() > kMaxFiles)
        return BundleFailure(masterName, "master entry count exceeds 128", provenance, error);
    for (const auto& entry : master.at("files")) {
        if (!entry.is_object() || !entry.contains("file") || !entry.at("file").is_string() ||
            !SafeEmbeddedPath(entry.at("file").get<std::string>()) ||
            !ValidMasterCondition(entry, "engine") || !ValidMasterCondition(entry, "game"))
            return BundleFailure(masterName, "malformed master entry", provenance, error);
    }
    const auto reader = [&](const std::string& name, nlohmann::json& doc, std::string& failure) {
        if (!SafeEmbeddedPath(name)) {
            failure = "gamedata package unsafe selected path: " + name;
            return false;
        }
        const auto it = documents.find(name);
        if (it == documents.end()) {
            failure = "gamedata package selected document not found: " + name;
            return false;
        }
        if (!it->second.is_object()) {
            failure = "gamedata package selected document is not an object: " + name;
            return false;
        }
        doc = it->second;
        return true;
    };
    GameConfig gc = MergeOwner(gamedataRoot, owner, engine, game, platform, reader, &provenance, error);
    gc.packageProvenance = std::move(provenance);
    // A selected shipped failure must never expose a partially merged package to Task 5's
    // activation path. Diagnostics still name every previously applied and failed document.
    if (!gc.filesFailed.empty()) {
        gc.interfaces.clear(); gc.offsets.clear(); gc.signatures.clear(); gc.keys.clear();
        gc.calls.clear(); gc.hooks.clear();
        return gc;
    }
    gc.mergedJson = SerializeMerged(gc, platform);
    return gc;
}

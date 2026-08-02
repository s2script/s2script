// Unit test for the owner-scoped gamedata loader (A5a Task 1-2).
// Self-contained: builds fixture trees under a temp dir, no repo data, no SDK.
#include "../src/gamedata.h"
// Only to READ BACK the merged view the loader hands core (GameConfig::mergedJson) — the header
// itself stays nlohmann-free on purpose.
#include "../third_party/json.hpp"
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

namespace fs = std::filesystem;

static int g_fail = 0;
#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }      \
        else         { std::cout << "ok:   " << (msg) << "\n"; }                \
    } while (0)

static void put(const fs::path& p, const std::string& body) {
    fs::create_directories(p.parent_path());
    std::ofstream f(p);
    f << body;
}

// A temp root cleaned up on scope exit.
struct TempRoot {
    fs::path path;
    TempRoot() {
        char tmpl[] = "/tmp/s2gd_XXXXXX";
        path = mkdtemp(tmpl);
    }
    ~TempRoot() { std::error_code ec; fs::remove_all(path, ec); }
};

static void test_master_selects_by_condition() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc", R"({
      "files": [
        { "file": "common.gamedata.jsonc" },
        { "file": "engine.source2.jsonc", "engine": "source2" },
        { "file": "game.cs2.jsonc",       "game":   "csgo" },
        { "file": "game.other.jsonc",     "game":   "dota" }
      ]
    })");
    put(root.path / "core" / "common.gamedata.jsonc",
        R"({ "interfaces": { "FromCommon": "C001" } })");
    put(root.path / "core" / "engine.source2.jsonc",
        R"({ "interfaces": { "FromEngine": "E001" } })");
    put(root.path / "core" / "game.cs2.jsonc",
        R"({ "interfaces": { "FromGame": "G001" } })");
    put(root.path / "core" / "game.other.jsonc",
        R"({ "interfaces": { "FromOther": "X001" } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(err.empty(), "matching master parses without error");
    CHECK(gc.interfaces.count("FromCommon") == 1, "unconditional file applies");
    CHECK(gc.interfaces.count("FromEngine") == 1, "engine-matched file applies");
    CHECK(gc.interfaces.count("FromGame") == 1, "game-matched file applies");
    CHECK(gc.interfaces.count("FromOther") == 0, "non-matching game file is skipped");
}

static void test_array_order_is_apply_order() {
    // "z" sorts AFTER "a"; if the loader ever iterates an object instead of the array,
    // the later-listed "a" file would lose. This test is the regression guard for that.
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc", R"({
      "files": [
        { "file": "z-first.gamedata.jsonc" },
        { "file": "a-second.gamedata.jsonc" }
      ]
    })");
    put(root.path / "core" / "z-first.gamedata.jsonc",
        R"({ "offsets": { "Shared": { "linuxsteamrt64": 1 } } })");
    put(root.path / "core" / "a-second.gamedata.jsonc",
        R"({ "offsets": { "Shared": { "linuxsteamrt64": 2 } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets["Shared"] == 2, "array order wins over alphabetical filename order");
}

static void test_condition_accepts_an_array() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc", R"({
      "files": [ { "file": "multi.gamedata.jsonc", "game": ["dota", "csgo"] } ]
    })");
    put(root.path / "core" / "multi.gamedata.jsonc",
        R"({ "interfaces": { "Multi": "M001" } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.interfaces.count("Multi") == 1, "array-valued condition matches any member");
}

static void test_override_is_per_named_entry() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc", R"({
      "files": [
        { "file": "base.gamedata.jsonc" },
        { "file": "over.gamedata.jsonc" }
      ]
    })");
    put(root.path / "core" / "base.gamedata.jsonc", R"({
      "signatures": {
        "Keep":    { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct" } },
        "Replace": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "BB", "resolve": "direct" } }
      }
    })");
    put(root.path / "core" / "over.gamedata.jsonc", R"({
      "signatures": {
        "Replace": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "CC", "resolve": "lea-disp" } }
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.signatures["Keep"].pattern == "AA", "untouched entry survives an override file");
    CHECK(gc.signatures["Replace"].pattern == "CC", "named entry is replaced wholesale");
    CHECK(gc.signatures["Replace"].resolve == "lea-disp", "replacement is not deep-merged");
}

static void test_other_platform_is_ignored() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "core" / "game.cs2.jsonc", R"({
      "offsets": { "Mine": { "linuxsteamrt64": 7 }, "Theirs": { "windows64": 9 } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets.count("Mine") == 1, "matching platform entry is loaded");
    CHECK(gc.offsets.count("Theirs") == 0, "other-platform entry is skipped");
}

static void test_keys_section_is_parsed() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc",
        R"({ "keys": { "SpriteBeam": "sprites/laserbeam.vmt" } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.keys["SpriteBeam"] == "sprites/laserbeam.vmt", "keys section is parsed");
}

// --- A5b: the merged view core consumes (GameConfig::mergedJson, spec §9.1b) ---------------
// The shim owns the ONE loader; core registers the game package's `calls` from this string. What
// these assert is the CONTRACT between the two: the sections core reads, in the nesting core's
// flatten step expects, with custom/ overrides already applied.

static void test_merged_json_carries_calls_and_platform_nested_signatures() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc", "game": "csgo" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "signatures": {
        "DoThing": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "55 48", "resolve": "direct" } }
      },
      "calls": {
        "doThing": { "receiver": { "kind": "entity" },
                     "target": { "kind": "signature", "name": "DoThing" },
                     "args": ["int"], "returns": "void" }
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(err.empty(), "a calls section parses without error");
    CHECK(gc.calls.count("doThing") == 1, "calls entry is merged");
    CHECK(!gc.mergedJson.empty(), "the merged view is serialised for core");

    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded(), "the merged view is valid JSON");
    // Re-NESTED under the platform key: core's flatten step reads signatures[name][platform], so a
    // flat (already-lifted) shape here would degrade every signature-targeted descriptor.
    CHECK(doc["signatures"]["DoThing"]["linuxsteamrt64"]["pattern"] == "55 48",
          "signatures are re-nested under the platform key core flattens against");
    CHECK(doc["calls"]["doThing"]["target"]["name"] == "DoThing",
          "the descriptor crosses to core verbatim");
    CHECK(doc["calls"]["doThing"]["args"][0] == "int", "the arg vocabulary is untouched");
}

// A signature's `validate` object is the SEMANTIC gate — the thing that separates a unique-but-WRONG
// match from the real function. It is authored next to the pattern it guards, and it must survive
// the merge: dropping it here would leave the descriptor resolving with a .text-range check and
// nothing else, silently, which is the exact failure the validator vocabulary exists to prevent.
// It also has to survive VERBATIM and un-flattened — core's flatten step lifts it onto the target,
// and the shim deliberately does not know the vocabulary.
static void test_merged_json_carries_a_signature_validator() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "signatures": {
        "Guarded": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "55 48", "resolve": "direct",
                     "validate": { "string-xref": { "at": 11, "dispOff": 3, "instrLen": 7, "expect": "Scope" } } } },
        "Bare":    { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct" } }
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(err.empty(), "a signature carrying a validator parses without error");
    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded(), "the merged view is valid JSON");
    const auto& v = doc["signatures"]["Guarded"]["linuxsteamrt64"]["validate"];
    CHECK(v["string-xref"]["at"] == 11 && v["string-xref"]["expect"] == "Scope",
          "the validator crosses to core verbatim, un-flattened");
    // An entry with no validator must not sprout a null one: core forwards whatever is here to the
    // shim, and `"validate": null` reads as a descriptor whose gate was declared and lost.
    CHECK(!doc["signatures"]["Bare"]["linuxsteamrt64"].contains("validate"),
          "a signature declaring no validator emits no validate key");
}

// An operator's after-a-CS2-update hot-fix replaces the whole named entry — pattern AND validator
// together. That atomicity is the reason the validator is authored beside the pattern rather than
// on the descriptor: a new pattern with the SHIPPED instruction offsets would be checked against a
// layout that no longer exists.
static void test_custom_override_replaces_pattern_and_validator_together() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "signatures": { "Guarded": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct",
                      "validate": { "string-xref": { "at": 11, "dispOff": 3, "instrLen": 7, "expect": "Scope" } } } } }
    })");
    put(root.path / "cs2" / "custom" / "fix.jsonc", R"({
      "signatures": { "Guarded": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "BB", "resolve": "direct",
                      "validate": { "string-xref": { "at": 19, "dispOff": 3, "instrLen": 7, "expect": "Scope" } } } } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    const auto& plat = doc["signatures"]["Guarded"]["linuxsteamrt64"];
    CHECK(!doc.is_discarded() && plat["pattern"] == "BB",
          "the operator's pattern wins");
    CHECK(plat["validate"]["string-xref"]["at"] == 19,
          "and so does the validator offset it was re-derived with");
}

// THE ONE EXCEPTION to named-entry replacement, and the reason for it.
//
// docs/INSTALL.md tells an operator to hot-fix a moved signature by dropping module/pattern/resolve
// into custom/. Nobody re-types the `validate` block with it — it is not what "the signature" means
// to the person doing the fix. Taking that omission literally DELETES the semantic gate: uniqueness
// passes, the .text-range check passes, and call_validate.cpp's `if (v.empty()) return true;` waves
// the entry through. gamedata/cs2/game.cs2.jsonc records by name a borrowed TerminateRound signature
// that matches UNIQUELY at the WRONG function, so that path ends in a call through a bogus address.
// Neither gate can see it: check-call-descriptors.sh skips custom/ by design and the boot banner
// only counts overrides.
//
// So the override INHERITS the shipped validator, and says so.
static void test_custom_override_without_validate_inherits_the_shipped_one() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "signatures": { "Guarded": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct",
                      "validate": { "string-xref": { "at": 11, "dispOff": 3, "instrLen": 7, "expect": "TerminateRound" } } } } }
    })");
    // The operator's hot-fix, written the way the install doc's example is written: the three keys
    // that spell "the signature", and nothing else.
    put(root.path / "cs2" / "custom" / "fix.jsonc", R"({
      "signatures": { "Guarded": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "BB", "resolve": "direct" } } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.signatures["Guarded"].pattern == "BB", "the operator's pattern still wins");
    CHECK(!gc.signatures["Guarded"].validate.empty(),
          "an override that omits validate does NOT disarm the shipped validator");

    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded(), "the merged view is valid JSON");
    const auto& plat = doc["signatures"]["Guarded"]["linuxsteamrt64"];
    CHECK(plat.contains("validate") &&
              plat["validate"]["string-xref"]["expect"] == "TerminateRound",
          "and the carried validator reaches the descriptor core resolves");
    CHECK(gc.validatorsCarried.size() == 1 && gc.validatorsCarried[0] == "Guarded",
          "the carry-forward is REPORTED by name, never silent");
    CHECK(gc.validatorsDisarmed.empty(), "and is not reported as a deliberate disarm");
}

// The escape hatch, and it has to be spelled out loud. An operator who genuinely wants no semantic
// gate writes an explicit empty `validate`; it is honoured, and it is a banner line of its own.
static void test_custom_override_can_disarm_a_validator_explicitly() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "signatures": { "Guarded": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct",
                      "validate": { "vtable-member": "CCSPlayerController" } } } }
    })");
    put(root.path / "cs2" / "custom" / "fix.jsonc", R"({
      "signatures": { "Guarded": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "BB", "resolve": "direct",
                      "validate": {} } } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded(), "the merged view is valid JSON");
    const auto& plat = doc["signatures"]["Guarded"]["linuxsteamrt64"];
    // `{}` is one of the "no validators declared" spellings call_validate.cpp already accepts, so
    // emitting it verbatim is the same thing as emitting nothing — and it PROVES the shipped
    // validator did not come back.
    CHECK(!plat.contains("validate") || plat["validate"].empty(),
          "an explicit empty validate is honoured, not overwritten by the shipped one");
    CHECK(gc.validatorsDisarmed.size() == 1 && gc.validatorsDisarmed[0] == "Guarded",
          "a deliberate disarm is REPORTED by name");
    CHECK(gc.validatorsCarried.empty(), "and is not reported as a carry-forward");
}

// Scope guard for the exception. A later SHIPPED file replacing an earlier one keeps plain
// named-entry semantics: our own tree is reviewed, and quietly resurrecting a validator a shipped
// file deliberately dropped would be its own silent-wrong-data bug.
static void test_shipped_replacement_does_not_inherit_a_validator() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc", R"({
      "files": [
        { "file": "base.gamedata.jsonc" },
        { "file": "later.gamedata.jsonc" }
      ]
    })");
    put(root.path / "core" / "base.gamedata.jsonc", R"({
      "signatures": { "Sig": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct",
                      "validate": { "prologue": "55 48" } } } }
    })");
    put(root.path / "core" / "later.gamedata.jsonc", R"({
      "signatures": { "Sig": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "BB", "resolve": "direct" } } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.signatures["Sig"].pattern == "BB", "the later shipped file wins");
    CHECK(gc.signatures["Sig"].validate.empty(),
          "shipped-over-shipped is still a whole-entry replacement");
    CHECK(gc.validatorsCarried.empty() && gc.validatorsDisarmed.empty(),
          "and neither override-only report fires for a shipped merge");
}

static void test_merged_json_reflects_a_custom_override() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "signatures": { "DoThing": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct" } } },
      "calls": { "doThing": { "target": { "kind": "signature", "name": "DoThing" }, "returns": "void" } }
    })");
    // The operator's after-a-CS2-update hot-fix: one entry, replacing the shipped pattern.
    put(root.path / "cs2" / "custom" / "fix.jsonc",
        R"({ "signatures": { "DoThing": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "BB", "resolve": "direct" } } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded() && doc["signatures"]["DoThing"]["linuxsteamrt64"]["pattern"] == "BB",
          "a custom/ override reaches the descriptor core resolves");
    CHECK(gc.overridden.count("DoThing") == 1, "and is still reported as operator-supplied");
}

static void test_calls_entry_of_the_wrong_type_degrades_only_that_entry() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "calls": { "good": { "returns": "void" }, "bad": 7 }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.calls.count("good") == 1, "the well-formed descriptor still merges");
    CHECK(gc.calls.count("bad") == 0, "a scalar descriptor is left out, not crashed on");
    CHECK(!err.empty() && err.find("bad") != std::string::npos, "the error names the entry");
}

// --- Declarative inbound hooks: the section must SURVIVE the merge ------------------------------
// This is the test whose absence let a whole feature ship inert. `gamedata/cs2/game.cs2.jsonc`
// declared two hooks, `scripts/check-call-descriptors.sh` validated them ON DISK, and core's
// registry consumed them correctly — but this loader had no `hooks` member and no parse branch, so
// the section evaporated between the file and core. On a live server every hook reported
// "not declared in this owner's gamedata", and the merged JSON was byte-for-byte the size it had
// been before the section was ever written.
static void test_merged_json_carries_hooks() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc", "game": "csgo" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "signatures": {
        "DoThing": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "55 48", "resolve": "direct",
                     "validate": { "prologue": "55" } } }
      },
      "calls": {
        "doThing": { "receiver": { "kind": "none" },
                     "target": { "kind": "signature", "name": "DoThing" }, "returns": "void" }
      },
      "hooks": {
        "onThing": { "target": { "kind": "signature", "name": "DoThing" },
                     "shape": "this_f32_i32_i32_i32",
                     "params": ["delay", "reason", "u3", "u4"], "mutable": ["delay", "reason"],
                     "bypassWith": "doThing", "expose": { "ctx": "g" } }
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(err.empty(), "a hooks section parses without error");
    CHECK(gc.hooks.count("onThing") == 1, "the hooks entry is merged");

    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded(), "the merged view is valid JSON");
    CHECK(doc.contains("hooks"), "the merged view core reads CARRIES the hooks section");
    // Every field core's registry validates has to arrive intact — the shape name, the params and
    // their `mutable` subset, the bypass pairing, and the ctx namespace. A section that crossed
    // with any of these missing would degrade by name in core instead of working.
    CHECK(doc["hooks"]["onThing"]["shape"] == "this_f32_i32_i32_i32", "the shape name crosses");
    CHECK(doc["hooks"]["onThing"]["params"][1] == "reason", "params cross in author order");
    CHECK(doc["hooks"]["onThing"]["mutable"][0] == "delay", "the mutable allow-list crosses");
    CHECK(doc["hooks"]["onThing"]["bypassWith"] == "doThing", "the bypass pairing crosses");
    CHECK(doc["hooks"]["onThing"]["expose"]["ctx"] == "g", "the ctx namespace crosses");
    // The validator a hook target MUST carry (core rejects a hook without one) rides on the
    // SIGNATURE the target names, so the two have to survive together.
    CHECK(doc["signatures"]["DoThing"]["linuxsteamrt64"]["validate"]["prologue"] == "55",
          "the mandatory validator reaches core with the pattern it guards");
}

// An owner that declares ONLY hooks still hands core something. Without widening the early return
// in SerializeMerged, an inbound-only owner would serialise to "" — the same silent nothing.
static void test_a_hooks_only_owner_still_serialises() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "hooks": { "onThing": { "shape": "this_void", "expose": { "ctx": "g" } } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(!gc.mergedJson.empty(), "an owner with hooks and nothing else is not dropped");
    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded() && doc.contains("hooks"), "and its hooks reach core");
}

static void test_hooks_entry_of_the_wrong_type_degrades_only_that_entry() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "hooks": { "good": { "shape": "this_void" }, "bad": 7 }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.hooks.count("good") == 1, "the well-formed descriptor still merges");
    CHECK(gc.hooks.count("bad") == 0, "a scalar descriptor is left out, not crashed on");
    CHECK(!err.empty() && err.find("hooks.bad") != std::string::npos, "the error names the entry");
}

// A custom/ override reaches a hook exactly as it reaches a call: named-entry replacement.
static void test_custom_override_replaces_a_hook_entry() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc",
        R"({ "hooks": { "onThing": { "shape": "this_void", "expose": { "ctx": "g" } } } })");
    put(root.path / "cs2" / "custom" / "fix.jsonc",
        R"({ "hooks": { "onThing": { "shape": "this_f32_i32_i32_i32", "expose": { "ctx": "g" } } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    auto doc = nlohmann::json::parse(gc.mergedJson, nullptr, false);
    CHECK(!doc.is_discarded() && doc["hooks"]["onThing"]["shape"] == "this_f32_i32_i32_i32",
          "an operator override reaches the hook core resolves");
    CHECK(gc.overridden.count("onThing") == 1, "and is reported as operator-supplied");
}

// --- An unconsumed section is LOUD ---------------------------------------------------------------
// The second half of the `hooks` fix, and the more important half: a top-level key this build has
// no branch for used to evaporate in silence, which is what made a dropped section indistinguishable
// from a working one. Naming it turns the next such omission into a boot line.
static void test_an_unconsumed_section_is_reported_by_name() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "keys": { "Real": "yes" },
      "forwards": { "onSomething": { "shape": "this_void" } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    bool named = false;
    for (const auto& s : gc.sectionsIgnored)
        if (s.find("forwards") != std::string::npos && s.find("game.cs2.jsonc") != std::string::npos)
            named = true;
    CHECK(named, "an unknown top-level section is reported, naming BOTH the file and the section");
    CHECK(gc.sectionsIgnored.size() == 1, "and only the unknown one — no false positives");
    CHECK(err.empty(), "it is a WARN, not an error: an older shim reading a newer tree must boot");
    CHECK(gc.keys["Real"] == "yes", "the sections it DOES consume are unaffected");
}

// Every section this build consumes must be absent from the report — otherwise the WARN becomes
// noise nobody reads, which is the same failure as no WARN at all.
static void test_every_consumed_section_is_silent() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc", R"({
      "interfaces": { "I": "I001" },
      "offsets":    { "O": { "linuxsteamrt64": 4 } },
      "signatures": { "S": { "linuxsteamrt64": { "module": "m.so", "pattern": "55" } } },
      "keys":       { "K": "v" },
      "calls":      { "c": { "returns": "void" } },
      "hooks":      { "h": { "shape": "this_void" } }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.sectionsIgnored.empty(), "no consumed section is reported as ignored");
}

static void test_merged_json_is_empty_when_nothing_declares_one() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc",
        R"({ "offsets": { "Some": { "linuxsteamrt64": 4 } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.mergedJson.empty(),
          "an owner with neither signatures nor calls hands core nothing to register");
}

static void test_missing_master_is_a_named_error() {
    TempRoot root;
    fs::create_directories(root.path / "core");
    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(!err.empty(), "absent master yields an error, not a silent empty namespace");
    CHECK(err.find("master") != std::string::npos, "the error names the master file");
}

static void test_missing_listed_file_is_a_named_error() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "absent.gamedata.jsonc" } ] })");
    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(!err.empty(), "a listed-but-absent file is an error");
    CHECK(err.find("absent.gamedata.jsonc") != std::string::npos, "the error names the file");
}

static void test_owners_do_not_share_a_namespace() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "core" / "game.cs2.jsonc",
        R"({ "offsets": { "Same": { "linuxsteamrt64": 1 } } })");
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "cs2" / "game.cs2.jsonc",
        R"({ "offsets": { "Same": { "linuxsteamrt64": 2 } } })");

    std::string e1, e2;
    GameConfig a = LoadGameConfig(root.path.string(), "core", "source2", "csgo", "linuxsteamrt64", e1);
    GameConfig b = LoadGameConfig(root.path.string(), "cs2",  "source2", "csgo", "linuxsteamrt64", e2);
    CHECK(a.offsets["Same"] == 1 && b.offsets["Same"] == 2, "same key in two owners does not collide");
}

static void test_bad_offset_type_degrades_only_that_entry() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "mixed.gamedata.jsonc" } ] })");
    // "BadOffset" is a quoted number — wrong type for an offset, which wants an int.
    put(root.path / "core" / "mixed.gamedata.jsonc", R"({
      "offsets": {
        "GoodOffset": { "linuxsteamrt64": 5 },
        "BadOffset":  { "linuxsteamrt64": "7" }
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets.count("GoodOffset") == 1 && gc.offsets["GoodOffset"] == 5,
          "sibling well-formed offset survives a wrongly-typed one");
    CHECK(gc.offsets.count("BadOffset") == 0, "wrongly-typed offset is left out, not crashed on");
    CHECK(!err.empty() && err.find("BadOffset") != std::string::npos,
          "error names the offending offset key");
}

static void test_bad_signature_type_degrades_only_that_entry() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "mixed.gamedata.jsonc" } ] })");
    // "BadSig" is a bare scalar — wrong type for a signature, which wants an object.
    put(root.path / "core" / "mixed.gamedata.jsonc", R"({
      "signatures": {
        "GoodSig": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct" } },
        "BadSig":  { "linuxsteamrt64": 42 }
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.signatures.count("GoodSig") == 1 && gc.signatures["GoodSig"].pattern == "AA",
          "sibling well-formed signature survives a wrongly-typed one");
    CHECK(gc.signatures.count("BadSig") == 0, "wrongly-typed signature is left out, not crashed on");
    CHECK(!err.empty() && err.find("BadSig") != std::string::npos,
          "error names the offending signature key");
}

static void test_bad_interface_type_degrades_only_that_entry() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "mixed.gamedata.jsonc" } ] })");
    // "BadIface" is a number — wrong type for an interface, which wants a version string.
    put(root.path / "core" / "mixed.gamedata.jsonc", R"({
      "interfaces": {
        "GoodIface": "V001",
        "BadIface":  7
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.interfaces.count("GoodIface") == 1 && gc.interfaces["GoodIface"] == "V001",
          "sibling well-formed interface survives a wrongly-typed one");
    CHECK(gc.interfaces.count("BadIface") == 0,
          "wrongly-typed interface is left out, not crashed on");
    CHECK(!err.empty() && err.find("BadIface") != std::string::npos,
          "error names the offending interface key");
}

static void test_bad_keys_type_degrades_only_that_entry() {
    TempRoot root;
    put(root.path / "cs2" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "mixed.gamedata.jsonc" } ] })");
    // "BadKey" is a number — wrong type for a `keys` entry, which wants a behavioural string.
    put(root.path / "cs2" / "mixed.gamedata.jsonc", R"({
      "keys": {
        "GoodKey": "sprites/laserbeam.vmt",
        "BadKey":  1
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "cs2", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.keys.count("GoodKey") == 1 && gc.keys["GoodKey"] == "sprites/laserbeam.vmt",
          "sibling well-formed key survives a wrongly-typed one");
    CHECK(gc.keys.count("BadKey") == 0, "wrongly-typed key is left out, not crashed on");
    CHECK(!err.empty() && err.find("BadKey") != std::string::npos,
          "error names the offending keys key");
}

static void test_bad_entry_does_not_abort_later_files() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc", R"({
      "files": [
        { "file": "bad-first.gamedata.jsonc" },
        { "file": "good-second.gamedata.jsonc" }
      ]
    })");
    put(root.path / "core" / "bad-first.gamedata.jsonc",
        R"({ "offsets": { "Broken": { "linuxsteamrt64": "not-an-int" } } })");
    put(root.path / "core" / "good-second.gamedata.jsonc",
        R"({ "offsets": { "FromSecondFile": { "linuxsteamrt64": 9 } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets.count("FromSecondFile") == 1 && gc.offsets["FromSecondFile"] == 9,
          "a later file still applies after an earlier file had one bad entry");
    CHECK(!err.empty(), "the earlier bad entry is still reported");
}

// The shim's interface-acquisition gate (s2script_mm.cpp) was rewritten to key off
// filesLoaded.empty() rather than !error.empty(), specifically so a per-entry type error (which
// sets `error` but keeps loading) does not take down every interface/hook/detour with it — only
// a genuinely catastrophic load (missing/unparseable master, or no listed file applied at all)
// should. Pin the distinction here so a future change to filesLoaded's population can't silently
// re-break that gate.
static void test_files_loaded_distinguishes_catastrophic_from_per_entry_error() {
    {
        TempRoot root;
        put(root.path / "core" / "master.gamedata.jsonc",
            R"({ "files": [ { "file": "mixed.gamedata.jsonc" } ] })");
        // A malformed entry sets `error` but the file still loads and is recorded.
        put(root.path / "core" / "mixed.gamedata.jsonc", R"({
          "offsets": {
            "GoodOffset": { "linuxsteamrt64": 5 },
            "BadOffset":  { "linuxsteamrt64": "7" }
          }
        })");

        std::string err;
        GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                       "linuxsteamrt64", err);
        CHECK(!err.empty(), "a per-entry type error is still reported");
        CHECK(!gc.filesLoaded.empty(),
              "filesLoaded is non-empty when a file loaded but had a malformed entry");
    }
    {
        // No master at all -> genuinely catastrophic, nothing was applied.
        TempRoot root;
        fs::create_directories(root.path / "core");
        std::string err;
        GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                       "linuxsteamrt64", err);
        CHECK(!err.empty(), "a missing master is still reported");
        CHECK(gc.filesLoaded.empty(),
              "filesLoaded is empty when the master itself is missing");
    }
}

// The flat (non-platform-keyed) shape is the mistake a server admin hand-editing custom/ makes
// first. Before the fix it was skipped with NO error, NO `overridden` mark and NO other signal —
// the operator saw a clean banner and the stale shipped value still in use.
static void test_non_object_offset_entry_is_a_named_error() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "core" / "game.cs2.jsonc",
        R"({ "offsets": { "Shipped": { "linuxsteamrt64": 7 } } })");
    // Flat shape: the platform level is missing entirely.
    put(root.path / "core" / "custom" / "oops.jsonc",
        R"({ "offsets": { "Shipped": 99 } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets["Shipped"] == 7, "a non-platform-keyed offset does not apply");
    CHECK(gc.overridden.empty(), "and is not falsely reported as an override");
    CHECK(!err.empty() && err.find("Shipped") != std::string::npos,
          "a non-platform-keyed offset is a NAMED error, not a silent skip");
}

static void test_non_object_signature_entry_is_a_named_error() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "mixed.gamedata.jsonc" } ] })");
    put(root.path / "core" / "mixed.gamedata.jsonc", R"({
      "signatures": {
        "GoodSig": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA", "resolve": "direct" } },
        "FlatSig": "55 48 89 E5"
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.signatures.count("GoodSig") == 1, "a sibling well-formed signature still applies");
    CHECK(gc.signatures.count("FlatSig") == 0, "a non-platform-keyed signature does not apply");
    CHECK(!err.empty() && err.find("FlatSig") != std::string::npos,
          "a non-platform-keyed signature is a NAMED error, not a silent skip");
}

// A condition of a type that can never match would deselect a whole file of engine facts,
// indistinguishably from a deliberate non-match.
static void test_bad_condition_type_is_a_named_error() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc", R"({
      "files": [
        { "file": "ok.gamedata.jsonc" },
        { "file": "weird.gamedata.jsonc", "game": 123 }
      ]
    })");
    put(root.path / "core" / "ok.gamedata.jsonc",
        R"({ "offsets": { "Fine": { "linuxsteamrt64": 1 } } })");
    put(root.path / "core" / "weird.gamedata.jsonc",
        R"({ "offsets": { "Lost": { "linuxsteamrt64": 2 } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets.count("Lost") == 0, "a file behind an unmatchable condition is not applied");
    CHECK(gc.offsets.count("Fine") == 1, "sibling files still apply");
    CHECK(!err.empty() && err.find("weird.gamedata.jsonc") != std::string::npos,
          "an unmatchable condition names the file it silently deselected");
}

// A file that parses but contributes nothing is the other half of the same failure: an operator's
// override file with a misspelled section or platform key looks exactly like a working one.
static void test_zero_entry_file_is_recorded() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "core" / "game.cs2.jsonc",
        R"({ "offsets": { "Val": { "linuxsteamrt64": 5 } } })");
    // Right section, WRONG platform key — parses, matches nothing.
    put(root.path / "core" / "custom" / "typo.jsonc",
        R"({ "offsets": { "Val": { "linuxsteamr64": 6 } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets["Val"] == 5, "the mistyped override does not apply");
    CHECK(gc.filesEmpty.size() == 1 && gc.filesEmpty[0] == "custom/typo.jsonc",
          "a file that applied zero entries is named in filesEmpty");
    CHECK(gc.filesLoaded.size() == 2, "it is still counted as loaded");
}

// filesFailed is the catastrophic signal the shim gates interface acquisition on. filesLoaded
// alone cannot see this: common.gamedata.jsonc is unconditional and applies first, so filesLoaded
// is non-empty no matter what fails after it.
static void test_files_failed_names_a_selected_file_that_could_not_apply() {
    {
        TempRoot root;
        put(root.path / "core" / "master.gamedata.jsonc", R"({
          "files": [
            { "file": "common.gamedata.jsonc" },
            { "file": "game.cs2.jsonc" }
          ]
        })");
        put(root.path / "core" / "common.gamedata.jsonc", R"({})");
        put(root.path / "core" / "game.cs2.jsonc", R"({ "offsets": { "Broken": )");   // truncated

        std::string err;
        GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                       "linuxsteamrt64", err);
        CHECK(!gc.filesLoaded.empty(),
              "filesLoaded is non-empty even though a selected file failed (why it is not enough)");
        CHECK(gc.filesFailed.size() == 1 && gc.filesFailed[0] == "game.cs2.jsonc",
              "filesFailed names the selected file that could not be applied");
    }
    {
        // A merely malformed ENTRY must NOT trip it — one bad entry never disables the block.
        TempRoot root;
        put(root.path / "core" / "master.gamedata.jsonc",
            R"({ "files": [ { "file": "mixed.gamedata.jsonc" } ] })");
        put(root.path / "core" / "mixed.gamedata.jsonc", R"({
          "offsets": {
            "GoodOffset": { "linuxsteamrt64": 5 },
            "BadOffset":  { "linuxsteamrt64": "7" }
          }
        })");

        std::string err;
        GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                       "linuxsteamrt64", err);
        CHECK(!err.empty(), "the bad entry is still reported in error");
        CHECK(gc.filesFailed.empty(), "a bad ENTRY does not put the file in filesFailed");
    }
    {
        // A missing master is catastrophic too, and reports through the same signal.
        TempRoot root;
        fs::create_directories(root.path / "core");
        std::string err;
        GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                       "linuxsteamrt64", err);
        CHECK(gc.filesFailed.size() == 1 && gc.filesFailed[0] == "master.gamedata.jsonc",
              "a missing master reports through filesFailed as well");
    }
    {
        // An operator's broken custom/ file is NOT catastrophic: the shipped tree is intact, so it
        // is a named error only — it must never disable interface acquisition.
        TempRoot root;
        put(root.path / "core" / "master.gamedata.jsonc",
            R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
        put(root.path / "core" / "game.cs2.jsonc",
            R"({ "offsets": { "Val": { "linuxsteamrt64": 5 } } })");
        put(root.path / "core" / "custom" / "broken.jsonc", R"({ "offsets": )");   // truncated

        std::string err;
        GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                       "linuxsteamrt64", err);
        CHECK(!err.empty() && err.find("broken.jsonc") != std::string::npos,
              "an unparseable custom/ file is a named error");
        CHECK(gc.filesFailed.empty(),
              "an unparseable custom/ file is not catastrophic — shipped data is intact");
    }
}

static void test_custom_overrides_a_shipped_entry() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "core" / "game.cs2.jsonc", R"({
      "signatures": {
        "Broken": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "AA BB", "resolve": "direct" } }
      }
    })");
    put(root.path / "core" / "custom" / "fix.jsonc", R"({
      "signatures": {
        "Broken": { "linuxsteamrt64": { "module": "libserver.so", "pattern": "CC DD", "resolve": "direct" } }
      }
    })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(err.empty(), "a custom/ override parses without error");
    CHECK(gc.signatures["Broken"].pattern == "CC DD", "custom/ is applied after shipped files");
    CHECK(gc.overridden.count("Broken") == 1, "an overridden entry is recorded as such");
}

static void test_custom_files_apply_in_sorted_order() {
    // No single creation order is a safe fixture: std::filesystem::directory_iterator's order is
    // unspecified and filesystem-dependent, so "created out of sorted order" is not the same thing
    // as "iterated out of sorted order". On this machine /tmp is tmpfs, where the iterator happens
    // to yield REVERSE creation order — meaning creating "20-second" before "10-first" makes the
    // iterator hand back "10-first, 20-second", i.e. sorted order BY ACCIDENT, and an unsorted
    // loader would pass this test. There is no creation order that is safe on every filesystem.
    // So: build the same two-file fixture in BOTH creation orders, each in its own TempRoot, and
    // assert the sorted-last file wins in both. Whichever way a given filesystem's iterator runs,
    // at least one of the two orderings hands an unsorted loader the files in already-sorted order
    // and the other hands it the reverse — so one of the two always catches a missing sort.
    auto run = [](bool createSortedFirstFirst) {
        TempRoot root;
        put(root.path / "core" / "master.gamedata.jsonc",
            R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
        put(root.path / "core" / "game.cs2.jsonc",
            R"({ "offsets": { "Val": { "linuxsteamrt64": 0 } } })");
        if (createSortedFirstFirst) {
            put(root.path / "core" / "custom" / "10-first.jsonc",
                R"({ "offsets": { "Val": { "linuxsteamrt64": 1 } } })");
            put(root.path / "core" / "custom" / "20-second.jsonc",
                R"({ "offsets": { "Val": { "linuxsteamrt64": 2 } } })");
        } else {
            put(root.path / "core" / "custom" / "20-second.jsonc",
                R"({ "offsets": { "Val": { "linuxsteamrt64": 2 } } })");
            put(root.path / "core" / "custom" / "10-first.jsonc",
                R"({ "offsets": { "Val": { "linuxsteamrt64": 1 } } })");
        }

        std::string err;
        GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                       "linuxsteamrt64", err);
        const std::string order = createSortedFirstFirst ? "created 10-first then 20-second"
                                                           : "created 20-second then 10-first";
        CHECK(gc.offsets["Val"] == 2, "later custom/ filename wins (" + order + ")");
        CHECK(gc.filesLoaded.size() == 3, "every applied file is recorded (" + order + ")");
    };
    run(true);
    run(false);
}

static void test_absent_custom_dir_is_not_an_error() {
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "core" / "game.cs2.jsonc",
        R"({ "offsets": { "Val": { "linuxsteamrt64": 5 } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(err.empty(), "no custom/ directory is the normal case, not an error");
    CHECK(gc.overridden.empty(), "nothing is marked overridden without a custom/ file");
}

int main() {
    test_master_selects_by_condition();
    test_array_order_is_apply_order();
    test_condition_accepts_an_array();
    test_override_is_per_named_entry();
    test_other_platform_is_ignored();
    test_keys_section_is_parsed();
    test_merged_json_carries_calls_and_platform_nested_signatures();
    test_merged_json_carries_a_signature_validator();
    test_custom_override_replaces_pattern_and_validator_together();
    test_custom_override_without_validate_inherits_the_shipped_one();
    test_custom_override_can_disarm_a_validator_explicitly();
    test_shipped_replacement_does_not_inherit_a_validator();
    test_merged_json_reflects_a_custom_override();
    test_calls_entry_of_the_wrong_type_degrades_only_that_entry();
    test_merged_json_carries_hooks();
    test_a_hooks_only_owner_still_serialises();
    test_hooks_entry_of_the_wrong_type_degrades_only_that_entry();
    test_custom_override_replaces_a_hook_entry();
    test_an_unconsumed_section_is_reported_by_name();
    test_every_consumed_section_is_silent();
    test_merged_json_is_empty_when_nothing_declares_one();
    test_missing_master_is_a_named_error();
    test_missing_listed_file_is_a_named_error();
    test_owners_do_not_share_a_namespace();
    test_bad_offset_type_degrades_only_that_entry();
    test_bad_signature_type_degrades_only_that_entry();
    test_bad_interface_type_degrades_only_that_entry();
    test_bad_keys_type_degrades_only_that_entry();
    test_bad_entry_does_not_abort_later_files();
    test_files_loaded_distinguishes_catastrophic_from_per_entry_error();
    test_non_object_offset_entry_is_a_named_error();
    test_non_object_signature_entry_is_a_named_error();
    test_bad_condition_type_is_a_named_error();
    test_zero_entry_file_is_recorded();
    test_files_failed_names_a_selected_file_that_could_not_apply();
    test_custom_overrides_a_shipped_entry();
    test_custom_files_apply_in_sorted_order();
    test_absent_custom_dir_is_not_an_error();
    if (g_fail) { std::cerr << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "gamedata_test: all checks passed\n";
    return 0;
}

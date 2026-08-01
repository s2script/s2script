// Unit test for the owner-scoped gamedata loader (A5a Task 1-2).
// Self-contained: builds fixture trees under a temp dir, no repo data, no SDK.
#include "../src/gamedata.h"
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
    TempRoot root;
    put(root.path / "core" / "master.gamedata.jsonc",
        R"({ "files": [ { "file": "game.cs2.jsonc" } ] })");
    put(root.path / "core" / "game.cs2.jsonc",
        R"({ "offsets": { "Val": { "linuxsteamrt64": 0 } } })");
    put(root.path / "core" / "custom" / "10-first.jsonc",
        R"({ "offsets": { "Val": { "linuxsteamrt64": 1 } } })");
    put(root.path / "core" / "custom" / "20-second.jsonc",
        R"({ "offsets": { "Val": { "linuxsteamrt64": 2 } } })");

    std::string err;
    GameConfig gc = LoadGameConfig(root.path.string(), "core", "source2", "csgo",
                                   "linuxsteamrt64", err);
    CHECK(gc.offsets["Val"] == 2, "later custom/ filename wins");
    CHECK(gc.filesLoaded.size() == 3, "every applied file is recorded");
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
    test_missing_master_is_a_named_error();
    test_missing_listed_file_is_a_named_error();
    test_owners_do_not_share_a_namespace();
    test_bad_offset_type_degrades_only_that_entry();
    test_bad_signature_type_degrades_only_that_entry();
    test_bad_entry_does_not_abort_later_files();
    test_custom_overrides_a_shipped_entry();
    test_custom_files_apply_in_sorted_order();
    test_absent_custom_dir_is_not_an_error();
    if (g_fail) { std::cerr << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "gamedata_test: all checks passed\n";
    return 0;
}

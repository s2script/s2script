# A5a — Gamedata Tiering Loader & Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat `gamedata/core.gamedata.jsonc` with SourceMod's owner/applicability tree — owner-scoped namespaces, a `master.gamedata.jsonc` index selecting files by engine/game, and an operator `custom/` override channel — with every one of the 46 existing entries resolving to the identical value it does today.

**Architecture:** `shim/src/gamedata.{h,cpp}` grows a `GameConfig` value type built once per owner at Load, replacing three path-taking free functions that re-parsed the file four times. Selection is a JSON **array** in `master.gamedata.jsonc` (array because plain `nlohmann::json` is `std::map` and sorts object keys alphabetically, so an object master's apply order would be decided by filename spelling). `gamedata/core.gamedata.jsonc` splits into `gamedata/core/{common,engine.source2,game.cs2}.gamedata.jsonc`, and the two gamedata keys named nowhere in source move to the new `gamedata/cs2/` owner.

**Tech Stack:** C++17 (shim, nlohmann/json vendored at `shim/third_party/json.hpp`), CMake, bash + python3 gate scripts, JSONC data files.

**Spec:** `docs/superpowers/specs/2026-08-01-gamedata-tiering-design.md` — this plan implements §8 (A5a) only. §9 (A5b, retiring the CS2 ops) is a separate slice and a separate PR.

## Global Constraints

- **Branch:** `gamedata/tiering-loader`. One slice, one PR, squash-merged. Never commit to `main`.
- **Platform key:** `linuxsteamrt64` is the only platform. Keep the key explicit; do not collapse it.
- **Engine id:** the literal string `"source2"`. **Game id:** the mod directory name, `"csgo"` for CS2.
- **Degrade, never crash:** every failure path disables exactly one owner or one entry with a *named* reason and lets the framework keep running. A missing `master.gamedata.jsonc` for an owner is a named hard error for that owner — never a silent empty namespace.
- **No behaviour change:** this slice must not alter a single resolved address, offset, or interface string. Task 5's equality check is the proof.
- **Comments are data:** the JSONC comments in `gamedata/core.gamedata.jsonc` are treadmill re-resolution recipes. Carry them verbatim to the file each entry lands in. Never summarize or drop one.
- **Gates:** `make ci` must pass before the PR. New gates go in `scripts/ci-native.sh`, never in `.github/workflows/*.yml`.
- **Worktree note:** a fresh worktree fails the licenses gate until `git submodule update --init --recursive` has run (objects are local; this is a checkout, not a download).

---

### Task 1: Owner-scoped `GameConfig` with master-driven file selection

Replaces the three path-taking loaders with one owner-scoped value type. Nothing calls it yet — `s2script_mm.cpp` still uses the old functions, which stay in place until Task 6.

**Files:**
- Modify: `shim/src/gamedata.h` (whole file)
- Modify: `shim/src/gamedata.cpp` (whole file)
- Create: `shim/tests/gamedata_test.cpp`
- Create: `scripts/test-gamedata.sh`
- Modify: `shim/CMakeLists.txt:110-113` (add a selftest target beside `ccommand_selftest`)
- Modify: `scripts/ci-native.sh:19-20` (add the gate after `test-sigscan.sh`)

**Interfaces:**
- Consumes: nothing.
- Produces: `struct SigSpec { std::string module, pattern, resolve; }` (unchanged shape, moved into the new header); `struct GameConfig { std::map<std::string,std::string> interfaces; std::map<std::string,int> offsets; std::map<std::string,SigSpec> signatures; std::map<std::string,std::string> keys; std::set<std::string> overridden; std::vector<std::string> filesLoaded; }`; and `GameConfig LoadGameConfig(const std::string& gamedataRoot, const std::string& owner, const std::string& engine, const std::string& game, const std::string& platform, std::string& error);`

- [ ] **Step 1: Write the failing test**

Create `shim/tests/gamedata_test.cpp`. It writes fixture trees into a temp directory, so it needs no repo data and cannot be broken by a treadmill edit to the real gamedata.

```cpp
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
    if (g_fail) { std::cerr << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "gamedata_test: all checks passed\n";
    return 0;
}
```

- [ ] **Step 2: Add the build + gate wiring**

In `shim/CMakeLists.txt`, immediately after the `ccommand_selftest` block (`:110-113`):

```cmake
# Owner-scoped gamedata loader unit test (not shipped; run by scripts/ci-native.sh).
add_executable(gamedata_selftest src/gamedata.cpp tests/gamedata_test.cpp)
target_include_directories(gamedata_selftest PRIVATE src third_party)
```

Create `scripts/test-gamedata.sh`:

```bash
#!/usr/bin/env bash
# Compile + run the owner-scoped gamedata loader unit test with the host compiler
# (no sniper container, no SDK — gamedata.cpp depends only on nlohmann/json).
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(mktemp -d)/gamedata_test"
g++ -std=c++17 -O2 -Wall -Wextra -I shim/third_party \
    -o "$out" shim/src/gamedata.cpp shim/tests/gamedata_test.cpp
"$out"
```

```bash
chmod +x scripts/test-gamedata.sh
```

In `scripts/ci-native.sh`, after the `test-sigscan.sh` block:

```bash
echo "== test-gamedata.sh =="
bash scripts/test-gamedata.sh
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `bash scripts/test-gamedata.sh`
Expected: FAIL — compile error, `'GameConfig' was not declared in this scope` / no `LoadGameConfig`.

- [ ] **Step 4: Write the header**

Replace `shim/src/gamedata.h` entirely:

```cpp
#pragma once
#include <map>
#include <set>
#include <string>
#include <vector>

// Owner-scoped gamedata (A5a). See docs/superpowers/specs/2026-08-01-gamedata-tiering-design.md.
//
// SourceMod's model, ported: gamedata is partitioned on two ORTHOGONAL axes.
//   * OWNER  — the directory (gamedata/core, gamedata/cs2). Who consumes the fact. Namespaced:
//              two owners may define the same key with different values and neither sees the other.
//   * TARGET — the file within it, selected by master.gamedata.jsonc (common / engine.X / game.Y).
//              What the fact is resolved against.
// core's own facts resolved against CS2's libserver.so live in gamedata/core/game.cs2.jsonc. Both
// axes are true at once; that is the whole point.

// A byte-signature spec: which module to scan, the IDA-style pattern, and the resolve strategy.
struct SigSpec {
    std::string module;
    std::string pattern;
    std::string resolve;
};

// One owner's merged view, for one platform.
struct GameConfig {
    std::map<std::string, std::string> interfaces;
    std::map<std::string, int>         offsets;
    std::map<std::string, SigSpec>     signatures;
    // Per-game behavioural strings (spec §7). Permitted in the game/plugin tiers ONLY;
    // scripts/check-gamedata-owners.sh fails a `keys` section under gamedata/core/.
    std::map<std::string, std::string> keys;

    // Entry names whose final value came from a custom/ override, for the boot banner and the
    // crash fingerprint — an operator's patched signature must never be silently attributed
    // to a shipped one.
    std::set<std::string> overridden;
    // Every file actually applied, in apply order (for the banner and the fingerprint hash).
    std::vector<std::string> filesLoaded;
};

// Build one owner's merged view.
//
//   gamedataRoot  absolute path to the gamedata/ directory
//   owner         subdirectory name ("core", "cs2")
//   engine        engine id, matched against a master entry's "engine" condition ("source2")
//   game          mod directory name, matched against "game" ("csgo")
//   platform      platform key whose nested details are lifted ("linuxsteamrt64")
//
// Apply order: master order first, then <owner>/custom/*.jsonc in sorted filename order. Later
// wins, replacing at the NAMED-ENTRY level — signatures.Respawn is swapped wholesale, never
// deep-merged. That is what lets an operator override be a one-entry file.
//
// `error` is empty on success. A missing master, an unparseable file, or a listed-but-absent file
// sets it with a NAMED reason and returns whatever was merged before the failure: the caller
// degrades that owner, and the framework keeps running.
GameConfig LoadGameConfig(const std::string& gamedataRoot,
                          const std::string& owner,
                          const std::string& engine,
                          const std::string& game,
                          const std::string& platform,
                          std::string& error);

// --- Legacy flat-file loaders (deleted in Task 6, once s2script_mm.cpp is on GameConfig) ---
std::map<std::string, std::string> LoadInterfaceVersions(const std::string& path, std::string& error);
std::map<std::string, int> LoadOffsets(const std::string& path,
                                        const std::string& platform,
                                        std::string& error);
std::map<std::string, SigSpec> LoadSignatures(const std::string& path,
                                              const std::string& platform,
                                              std::string& error);
```

- [ ] **Step 5: Write the implementation**

Append to `shim/src/gamedata.cpp` (keep the three existing functions untouched; they are deleted in Task 6). Add `#include <algorithm>`, `#include <filesystem>` and `#include <vector>` to the top of the file.

```cpp
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
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `bash scripts/test-gamedata.sh`
Expected: PASS — every `ok:` line, ending `gamedata_test: all checks passed`.

- [ ] **Step 7: Commit**

```bash
git add shim/src/gamedata.h shim/src/gamedata.cpp shim/tests/gamedata_test.cpp \
        shim/CMakeLists.txt scripts/test-gamedata.sh scripts/ci-native.sh
git commit -m "gamedata: owner-scoped GameConfig with master-driven file selection"
```

---

### Task 2: `custom/` override channel — tests for the operator hot-fix path

The merge code landed in Task 1; this task proves the operator-facing behaviour that justifies it, which is the piece an operator's post-update hot-fix depends on.

**Files:**
- Modify: `shim/tests/gamedata_test.cpp` (add three tests + three `main()` calls)

**Interfaces:**
- Consumes: `LoadGameConfig`, `GameConfig::overridden`, `GameConfig::filesLoaded` from Task 1.
- Produces: nothing new.

- [ ] **Step 1: Write the failing tests**

Insert before `int main()` in `shim/tests/gamedata_test.cpp`:

```cpp
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
```

And add to `main()`, before the `if (g_fail)` line:

```cpp
    test_custom_overrides_a_shipped_entry();
    test_custom_files_apply_in_sorted_order();
    test_absent_custom_dir_is_not_an_error();
```

- [ ] **Step 2: Run the tests**

Run: `bash scripts/test-gamedata.sh`
Expected: PASS. Task 1's implementation already covers this behaviour; if any check fails, the bug is in Task 1's `MergeFile`/custom-dir loop — fix it there rather than weakening the test.

- [ ] **Step 3: Commit**

```bash
git add shim/tests/gamedata_test.cpp
git commit -m "gamedata: pin the custom/ operator override channel"
```

---

### Task 3: Split the data into the owner tree

Creates the tree beside the flat file. The shim still reads the flat file, so nothing changes at runtime yet — this task is pure data movement, verified by an equality check.

**Files:**
- Create: `gamedata/core/master.gamedata.jsonc`
- Create: `gamedata/core/common.gamedata.jsonc`
- Create: `gamedata/core/engine.source2.jsonc`
- Create: `gamedata/core/game.cs2.jsonc`
- Create: `gamedata/cs2/master.gamedata.jsonc`
- Create: `gamedata/cs2/game.cs2.jsonc`
- Keep (deleted in Task 6): `gamedata/core.gamedata.jsonc`

**Interfaces:**
- Consumes: nothing.
- Produces: the on-disk tree that Task 6's loader wiring reads.

- [ ] **Step 1: Create the two masters**

`gamedata/core/master.gamedata.jsonc`:

```jsonc
// Gamedata master index for the "core" owner — the facts shim C++ / core Rust name in source.
// See docs/superpowers/specs/2026-08-01-gamedata-tiering-design.md.
//
// "files" is an ARRAY and the order is the APPLY order. It is not an object: the shim parses with
// plain nlohmann::json, whose object type is std::map, so object keys come back alphabetically
// sorted and apply order would be silently decided by filename spelling.
//
// Later entries override earlier ones at the NAMED-ENTRY level. <owner>/custom/*.jsonc is applied
// after everything here — that is the operator hot-fix channel, see docs/INSTALL.md.
{
  "files": [
    { "file": "common.gamedata.jsonc" },
    { "file": "engine.source2.jsonc", "engine": "source2" },
    { "file": "game.cs2.jsonc",       "game":   "csgo" }
  ]
}
```

`gamedata/cs2/master.gamedata.jsonc`:

```jsonc
// Gamedata master index for the "cs2" owner — the @s2script/cs2 game package's own facts.
// Nothing in shim/src or core/src may name a key in this namespace; that is enforced by
// scripts/check-gamedata-owners.sh.
{
  "files": [
    { "file": "game.cs2.jsonc", "game": "csgo" }
  ]
}
```

- [ ] **Step 2: Create `common.gamedata.jsonc`**

The tier exists and is empty; no fact has yet been proven game-independent.

```jsonc
// Facts that apply to every Source 2 game on every platform, regardless of which game binary is
// loaded. Empty today: every entry we have is either an engine-module interface string
// (engine.source2.jsonc) or resolved against CS2's libserver.so (game.cs2.jsonc).
//
// Before adding anything here, ask: would this identical value be correct on a Source 2 game that
// is not CS2? If you cannot answer yes with evidence, it belongs in game.cs2.jsonc. Promotion
// later is a free, behaviour-neutral data move; asserting engine-wideness we have not verified is
// not.
{
}
```

- [ ] **Step 3: Split the entries**

Move entries out of `gamedata/core.gamedata.jsonc` **carrying every comment verbatim** — the comments are the treadmill re-resolution recipes and are the most valuable content in the file.

- `gamedata/core/engine.source2.jsonc` — an `interfaces` section with the **5 engine-module** strings: `SchemaSystem`, `EngineCvar`, `EngineToServer`, `NetworkServerService`, `NetworkMessages`.
- `gamedata/core/game.cs2.jsonc` — an `interfaces` section with the **5 server-module** strings: `Source2Server`, `GameResourceService`, `GameEventSystem`, `Source2GameClients`, `Source2GameEntities`; an `offsets` section with **5** entries (all of today's except the two `CCSPlayer_ItemServices_*`); a `signatures` section with **all 29**.
- `gamedata/cs2/game.cs2.jsonc` — an `offsets` section with exactly `CCSPlayer_ItemServices_RemoveWeapons` and `CCSPlayer_ItemServices_DropActivePlayerWeapon`.

Give each new file a header comment naming its owner and its condition, e.g. for `gamedata/core/game.cs2.jsonc`:

```jsonc
// Owner: core (shim C++ / core Rust name every key here).
// Target: CS2 — "game": "csgo". Every signature below scans CS2's libserver.so.
//
// Owner and target are ORTHOGONAL: these are core's OWN facts, resolved against the CS2 game
// binary. Porting to another Source 2 game means a sibling game.<mod>.jsonc with this same key
// set and different bytes — it does not mean moving anything out of the core owner.
```

And for `gamedata/cs2/game.cs2.jsonc`:

```jsonc
// Owner: cs2 — the @s2script/cs2 game package. Nothing in shim/src or core/src may name a key in
// this file (scripts/check-gamedata-owners.sh enforces it).
// Target: CS2 — "game": "csgo".
//
// Today: the two parked CCSPlayer_ItemServices_* vtable indices from the deferred
// dropActiveWeapon work — the only two of the original 46 entries named nowhere in source.
// A5b adds the eight CS2-API signatures here as `calls` descriptors.
```

- [ ] **Step 4: Verify the merged tree equals the flat file**

This is the slice's central correctness claim: no resolved value changes.

Run:

```bash
python3 - <<'PY'
import json, re, pathlib, sys

def load(p):
    s = pathlib.Path(p).read_text()
    s = re.sub(r'//[^\n]*', '', s)
    s = re.sub(r'/\*.*?\*/', '', s, flags=re.S)
    return json.loads(s)

flat = load('gamedata/core.gamedata.jsonc')

merged = {}
for owner in ('core', 'cs2'):
    master = load(f'gamedata/{owner}/master.gamedata.jsonc')
    for entry in master['files']:
        j = load(f"gamedata/{owner}/{entry['file']}")
        for section, body in j.items():
            merged.setdefault(section, {}).update(body)

bad = 0
for section in ('interfaces', 'offsets', 'signatures'):
    a, b = flat.get(section, {}), merged.get(section, {})
    for k in sorted(set(a) | set(b)):
        if k not in b:      print(f'LOST      {section}.{k}');            bad += 1
        elif k not in a:    print(f'INVENTED  {section}.{k}');            bad += 1
        elif a[k] != b[k]:  print(f'CHANGED   {section}.{k}');            bad += 1
    print(f'{section}: {len(a)} flat, {len(b)} merged')

print('MISMATCHES:', bad)
sys.exit(1 if bad else 0)
PY
```

Expected output — exactly this, and exit status 0:

```
interfaces: 10 flat, 10 merged
offsets: 7 flat, 7 merged
signatures: 29 flat, 29 merged
MISMATCHES: 0
```

If any line reports `LOST`/`INVENTED`/`CHANGED`, the split dropped or mangled an entry. Fix the data; do not adjust the check.

- [ ] **Step 5: Commit**

```bash
git add gamedata/core gamedata/cs2
git commit -m "gamedata: split the flat file into the core/ and cs2/ owner tree"
```

---

### Task 4: Owner boundary gate

Makes the ownership rule mechanical. Lands before the shim switchover so a mistake in Task 3's split is caught by CI rather than by a live server.

**Files:**
- Create: `scripts/check-gamedata-owners.sh`
- Modify: `scripts/ci-native.sh` (add after the `test-gamedata.sh` block)

**Interfaces:**
- Consumes: the tree from Task 3.
- Produces: a CI gate; no code interface.

- [ ] **Step 1: Write the gate**

Create `scripts/check-gamedata-owners.sh`:

```bash
#!/usr/bin/env bash
# Enforces the gamedata OWNERSHIP rule (spec 2026-08-01-gamedata-tiering-design.md §4):
#
#   A key belongs to whoever NAMES it in source.
#
# gamedata/core/**  : every key MUST appear as a string literal in shim/src or core/src.
#                     A key nobody names is not core's — it belongs to a game package or a plugin.
# gamedata/cs2/**   : no key may appear in shim/src or core/src. A hit means the game package's
#                     namespace has leaked back into the engine-generic layer, which is exactly
#                     the boundary this tier exists to hold.
#
# Also checks that the master index and the files on disk agree in both directions: a file present
# but unlisted would silently never load, and a file listed but absent is a boot-time hard error.
# And that no `keys` section (per-game behavioural strings, spec §7) appears under gamedata/core/.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'PY'
import json, re, pathlib, sys

def load(p):
    s = pathlib.Path(p).read_text()
    s = re.sub(r'//[^\n]*', '', s)
    s = re.sub(r'/\*.*?\*/', '', s, flags=re.S)
    return json.loads(s)

# Every C++/Rust source byte, concatenated once.
blob = []
for root in ('shim/src', 'core/src'):
    for p in pathlib.Path(root).rglob('*'):
        if p.is_file() and p.suffix in ('.cpp', '.h', '.rs'):
            blob.append(p.read_text(errors='ignore'))
blob = ''.join(blob)

FACT_SECTIONS = ('interfaces', 'offsets', 'signatures')
bad = []

for owner in ('core', 'cs2'):
    owner_dir = pathlib.Path('gamedata') / owner
    if not owner_dir.is_dir():
        bad.append(f'{owner}: owner directory missing')
        continue

    master_path = owner_dir / 'master.gamedata.jsonc'
    if not master_path.exists():
        bad.append(f'{owner}: master.gamedata.jsonc missing')
        continue
    master = load(master_path)
    listed = [e['file'] for e in master.get('files', [])]

    on_disk = sorted(p.name for p in owner_dir.glob('*.jsonc')
                     if p.name != 'master.gamedata.jsonc')
    for f in listed:
        if not (owner_dir / f).exists():
            bad.append(f'{owner}: master lists {f}, which does not exist')
    for f in on_disk:
        if f not in listed:
            bad.append(f'{owner}: {f} exists but is not listed in master — it will never load')

    for f in listed:
        p = owner_dir / f
        if not p.exists():
            continue
        j = load(p)
        if owner == 'core' and 'keys' in j:
            bad.append(f'core/{f}: `keys` (behavioural strings) is not permitted in core gamedata')
        for section in FACT_SECTIONS:
            for key in j.get(section, {}):
                named = f'"{key}"' in blob
                if owner == 'core' and not named:
                    bad.append(f'core/{f}: {section}.{key} is named nowhere in shim/src or '
                               f'core/src — it does not belong to the core owner')
                if owner == 'cs2' and named:
                    bad.append(f'cs2/{f}: {section}.{key} IS named in shim/src or core/src — '
                               f'the game-package namespace has leaked into core')

if bad:
    print('GAMEDATA OWNERSHIP VIOLATIONS:', file=sys.stderr)
    for b in bad:
        print(f'  {b}', file=sys.stderr)
    sys.exit(1)
print('check-gamedata-owners: ownership rule holds for every entry')
PY
```

```bash
chmod +x scripts/check-gamedata-owners.sh
```

- [ ] **Step 2: Run it against the tree**

Run: `bash scripts/check-gamedata-owners.sh`
Expected: PASS — `check-gamedata-owners: ownership rule holds for every entry`.

If it reports `core/game.cs2.jsonc: offsets.CCSPlayer_ItemServices_* is named nowhere`, Task 3 left those two entries in the wrong owner — move them to `gamedata/cs2/game.cs2.jsonc`.

- [ ] **Step 3: Prove the gate actually fails**

A gate that cannot fail is decoration. Verify both directions:

```bash
# Direction 1: a cs2 key that core names.
python3 - <<'PY'
import pathlib
p = pathlib.Path('gamedata/cs2/game.cs2.jsonc')
p.write_text(p.read_text().replace('"offsets": {', '"offsets": {\n    "GameEntitySystem": { "linuxsteamrt64": 80 },', 1))
PY
bash scripts/check-gamedata-owners.sh && echo "GATE FAILED TO FIRE" || echo "gate fired correctly"
git checkout gamedata/cs2/game.cs2.jsonc
```

Expected: `gate fired correctly`, with a `cs2/game.cs2.jsonc: offsets.GameEntitySystem IS named in shim/src or core/src` line on stderr.

- [ ] **Step 4: Wire into CI**

In `scripts/ci-native.sh`, after the `test-gamedata.sh` block:

```bash
echo "== check-gamedata-owners.sh (gamedata ownership boundary) =="
bash scripts/check-gamedata-owners.sh
```

- [ ] **Step 5: Commit**

```bash
git add scripts/check-gamedata-owners.sh scripts/ci-native.sh
git commit -m "gate: enforce the gamedata ownership rule mechanically"
```

---

### Task 5: Mod-directory detection

The one runtime fact the loader needs that the shim does not have today. Isolated into its own task because it is the only piece that can be wrong in a way the unit tests cannot see.

**Files:**
- Modify: `shim/src/s2script_mm.cpp:2380-2402` (rework `GamedataPath` into `GamedataRoot` + add `DetectModDir`)

**Interfaces:**
- Consumes: nothing.
- Produces: `static std::string GamedataRoot()` → absolute path to `addons/s2script/gamedata`; `static std::string DetectModDir()` → the mod directory name (`"csgo"`), or `""` if undetectable. Task 6 consumes both.

- [ ] **Step 1: Replace `GamedataPath` with `GamedataRoot` + `DetectModDir`**

Replace the whole `GamedataPath()` function and its comment block (`s2script_mm.cpp:2380-2402`):

```cpp
// ---------------------------------------------------------------------------
// GamedataRoot / DetectModDir: resolve the gamedata tree and the mod directory relative to the
// plugin .so via dladdr, so both work regardless of the server's working directory.
//
// Expected layout: <game>/csgo/addons/s2script/bin/linuxsteamrt64/s2script.so
//   dirname x1 -> .../bin/linuxsteamrt64
//   dirname x2 -> .../bin
//   dirname x3 -> .../addons/s2script      <- addon root, + /gamedata
//   dirname x4 -> .../addons
//   dirname x5 -> .../csgo                 <- basename = the mod directory ("game" condition)
//
// The mod directory is the `game` axis of the gamedata master index (spec §6). Deriving it from
// the .so path costs no new engine dependency and is correct before any interface is acquired,
// which matters because the gamedata load happens first in Load().
// ---------------------------------------------------------------------------
static std::string AddonRoot() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&AddonRoot), &info) && info.dli_fname) {
        char buf[4096];
        // dirname mutates the buffer in-place; copy each time.
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir;
    }
    return std::string();
}

static std::string GamedataRoot() {
    std::string root = AddonRoot();
    if (!root.empty()) return root + "/gamedata";
    // Fallback: relative to the server's cwd (mirrors the Slice-0 behaviour).
    return "addons/s2script/gamedata";
}

static std::string DetectModDir() {
    std::string root = AddonRoot();
    if (root.empty()) return std::string();
    char buf[4096];
    snprintf(buf, sizeof buf, "%s", root.c_str());
    std::string dir = dirname(buf);                 // addons
    snprintf(buf, sizeof buf, "%s", dir.c_str());
    dir = dirname(buf);                             // the mod directory
    snprintf(buf, sizeof buf, "%s", dir.c_str());
    return std::string(basename(buf));              // "csgo"
}
```

- [ ] **Step 2: Log the detection at Load**

In `S2ScriptPlugin::Load`, immediately after the `s_gdOk = 0; s_gdFail = 0;` line (`s2script_mm.cpp:3611`):

```cpp
    const std::string gdRoot = GamedataRoot();
    const std::string modDir = DetectModDir();
    META_CONPRINTF("[s2script] gamedata root=%s engine=source2 game=%s\n",
                   gdRoot.c_str(), modDir.empty() ? "<undetected>" : modDir.c_str());
```

- [ ] **Step 3: Build**

Run: `cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release -DS2_CORE_LIB_DIR=debug && cmake --build build/shim -j`
Expected: builds clean. (`GamedataPath()` is still referenced by the crash-identity block at `:4683`; leave that call site alone — Task 6 rewrites it. If the build breaks because `GamedataPath` no longer exists, keep a one-line `static std::string GamedataPath() { return GamedataRoot() + "/core.gamedata.jsonc"; }` shim until Task 6 removes it.)

- [ ] **Step 4: Verify detection on the live server**

The unit tests cannot see this; a wrong `game` silently selects no files.

**The binaries must come from the sniper container.** A host `make all` links against this box's
glibc (GLIBC_2.38) and cannot load on the Steam Runtime 3 server (glibc 2.31) — Metamod reports
`meta list` → `<ERROR>` and nothing else, with no explanation. See CLAUDE.md's build section.

This worktree is a *linked* worktree, so `docker/cs2-data` and `docker/metamod` do not exist here
(they are gitignored and live only in the primary). `scripts/gate.sh` is the mechanism: it
reflink-clones the primary's install into `.gate/` and runs this worktree's own instance on port
27016+. Run `gate.sh up` with **no flags** — `--addons dist/addons` would point Metamod at
`dist/addons/metamod`, which holds only the VDF and no `bin/`.

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh     # the ONLY deployable binaries
bash scripts/gate.sh up                                 # this worktree's instance, port 27016
docker logs s2script-cs2-<worktree-name> 2>&1 | grep "gamedata root="
python3 scripts/rcon.py --port 27016 "meta list"        # must NOT say <ERROR>
```

Expected: a line ending `engine=source2 game=csgo`. If it reads `game=<undetected>` or any other name, the dirname chain does not match this deployment's layout — fix `DetectModDir` before continuing, because Task 6 makes the whole tree depend on it.

- [ ] **Step 5: Commit**

```bash
git add shim/src/s2script_mm.cpp
git commit -m "shim: derive the gamedata root and mod directory from the plugin .so path"
```

---

### Task 6: Switch the shim to the tree; retire the flat file

The switchover. Builds one `GameConfig` per owner at Load and points the four existing call sites at it, replacing four separate re-parses of the same file.

**Files:**
- Modify: `shim/src/s2script_mm.cpp` — `:3615` (interfaces), `:3691` (CheckTransmit offset), `:3866` (entity-system offset), `:3896` (signatures), `:4683-4689` (crash fingerprint), `:2784-2787` (banner)
- Modify: `shim/src/gamedata.h`, `shim/src/gamedata.cpp` — delete the three legacy loaders
- Modify: `scripts/package-addon.sh:32-36`
- Modify: `scripts/check-gamedata-sigs.sh:22`
- Delete: `gamedata/core.gamedata.jsonc`

**Interfaces:**
- Consumes: `LoadGameConfig` (Task 1), `GamedataRoot` / `DetectModDir` (Task 5), the tree (Task 3).
- Produces: `static GameConfig s_gdCore;` — the core owner's merged view, built once at Load and read by every later call site.

- [ ] **Step 1: Build both owners once at Load**

Replace the `std::string gdError; auto versions = LoadInterfaceVersions(GamedataPath(), gdError);` block at `s2script_mm.cpp:3614-3615` with:

```cpp
    // --- Gamedata (owner-scoped; built ONCE per Load, spec §6) ---
    std::string gdError;
    s_gdCore = LoadGameConfig(gdRoot, "core", "source2", modDir, "linuxsteamrt64", gdError);
    if (!gdError.empty()) {
        META_CONPRINTF("[s2script] WARN: %s — core gamedata degraded\n", gdError.c_str());
    }
    META_CONPRINTF("[s2script] gamedata core: %zu interfaces, %zu offsets, %zu signatures "
                   "from %zu file(s)\n",
                   s_gdCore.interfaces.size(), s_gdCore.offsets.size(),
                   s_gdCore.signatures.size(), s_gdCore.filesLoaded.size());
    for (const auto& name : s_gdCore.overridden) {
        META_CONPRINTF("[s2script]   gamedata OVERRIDE %s (from custom/) — operator-supplied, "
                       "not the shipped value\n", name.c_str());
    }
    auto& versions = s_gdCore.interfaces;
```

Declare the static beside `s_gdOk`/`s_gdFail` (`s2script_mm.cpp:2741`):

```cpp
static GameConfig s_gdCore;   // the core owner's merged gamedata, rebuilt each Load
```

- [ ] **Step 2: Point the other three call sites at it**

At `:3691`, replace the two lines building `ctiOffsets`:

```cpp
            auto cit = s_gdCore.offsets.find("CheckTransmitInfo_clientEntityIndex");
            s_ctiClientOff = (cit != s_gdCore.offsets.end() && cit->second >= 0) ? cit->second : -1;
            GamedataResult("CheckTransmitInfo_clientEntityIndex", s_ctiClientOff >= 0,
                           "offset key absent from gamedata");
```

At `:3866`, replace the `std::string offsetError; auto offsets = LoadOffsets(...)` pair and its WARN block with:

```cpp
                auto oit = s_gdCore.offsets.find("GameEntitySystem");
```

(leaving the `if (oit != ... )` body that follows unchanged — it already reads `oit->second`).

At `:3896`, replace the `std::string sigErr; auto sigs = LoadSignatures(...)` pair and its WARN block with:

```cpp
            auto& sigs = s_gdCore.signatures;
```

Then delete every remaining `LoadOffsets(` / `LoadSignatures(` / `LoadInterfaceVersions(` call, and remove the now-unused `ctiErr` / `offsetError` / `sigErr` locals. Verify none remain:

```bash
grep -n "LoadOffsets(\|LoadSignatures(\|LoadInterfaceVersions(" shim/src/s2script_mm.cpp
```

Expected: no output.

- [ ] **Step 3: Fingerprint the tree, not the file**

Replace `s2script_mm.cpp:4683-4689` (`std::string gdPath = GamedataPath();` through the `stat` block) with:

```cpp
        // Fingerprint every gamedata file actually applied, in apply order, plus a marker for how
        // many entries an operator override supplied. An incident must never attribute a patched
        // signature to the shipped value.
        std::string gdBytes;
        char gdMtime[32] = "";
        long long newest = 0;
        for (const auto& name : s_gdCore.filesLoaded) {
            const std::string p = gdRoot + "/core/" + name;
            gdBytes += slurp(p);
            struct stat fst{};
            if (stat(p.c_str(), &fst) == 0 && (long long)fst.st_mtime > newest)
                newest = (long long)fst.st_mtime;
        }
        if (newest) snprintf(gdMtime, sizeof gdMtime, "%lld", newest);
        std::string gdFp = gdBytes.empty() ? "" : fnv64hex(gdBytes);
        if (!s_gdCore.overridden.empty())
            gdFp += "+custom" + std::to_string(s_gdCore.overridden.size());
```

Note `gdRoot` is the Task 5 local declared at the top of `Load`; if this block is not in `Load`'s scope, call `GamedataRoot()` here instead. The `+customN` suffix rides inside the existing `fingerprint` string, so the frozen `schema_version:1` crash envelope is unchanged.

- [ ] **Step 4: Report overrides in the banner**

Replace `GamedataBanner()` at `:2784`:

```cpp
static void GamedataBanner() {
    META_CONPRINTF("[s2script] === GAMEDATA VALIDATION: %d ok, %d FAILED%s ===\n", s_gdOk, s_gdFail,
                   s_gdFail ? "  (STALE for this CS2 build — regenerate; see docs/re-strategy.md)" : "");
    if (!s_gdCore.overridden.empty())
        META_CONPRINTF("[s2script] === %zu ENTRY/ENTRIES FROM gamedata/core/custom/ — "
                       "operator-supplied, NOT the shipped values ===\n",
                       s_gdCore.overridden.size());
}
```

- [ ] **Step 5: Delete the legacy loaders and the flat file**

Remove `LoadInterfaceVersions`, `LoadOffsets`, `LoadSignatures` from both `shim/src/gamedata.h` (the "Legacy flat-file loaders" block) and `shim/src/gamedata.cpp` (the three original function bodies), plus the temporary `GamedataPath()` shim from Task 5 Step 3 if it was added.

```bash
git rm gamedata/core.gamedata.jsonc
```

- [ ] **Step 6: Update packaging and the sig gate**

In `scripts/package-addon.sh`, replace the gamedata block at `:32-36`:

```bash
# --- Gamedata (owner tree: core/ + cs2/, each with its master and an optional custom/) ---
if [ -d gamedata ]; then
    cp -r gamedata/. "$DIST/s2script/gamedata/"
else
    echo "ERROR: gamedata/ tree not found" >&2
    exit 1
fi
```

In `scripts/check-gamedata-sigs.sh`, replace the single-path read at `:22`:

```python
paths = sorted(pathlib.Path("gamedata").rglob("*.jsonc"))
sigs = {}
for path in paths:
    raw = re.sub(r'^\s*//.*$', '', path.read_text(), flags=re.M)
    sigs.update(json.loads(raw).get("signatures", {}))
```

- [ ] **Step 7: Run the full native gate**

Run: `make ci-native`
Expected: every gate passes, ending `ci-native: all native gates passed`.

- [ ] **Step 8: Live gate — the byte-identical proof**

Run:

Same build constraint as Task 5 Step 4 — sniper container only, `gate.sh` for the server:

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
bash scripts/gate.sh down && bash scripts/gate.sh up
docker logs s2script-cs2-<worktree-name> 2>&1 \
  | grep -E "gamedata (root=|core:|OK|FAIL)|GAMEDATA VALIDATION"
```

Expected:
- `gamedata root=… engine=source2 game=csgo`
- `gamedata core: 10 interfaces, 5 offsets, 29 signatures from 3 file(s)`
- a `gamedata OK` line for each of the same names as before the change
- `=== GAMEDATA VALIDATION: N ok, 0 FAILED ===` with the **same N** as `main`

Capture the pre-change baseline for comparison. Do it from a **separate checkout of `main`** rather
than stashing in place — a sniper build takes minutes and stashing leaves the tree in a state where
an interrupted run loses work:

```bash
git worktree add /tmp/gd-baseline main
cd /tmp/gd-baseline && docker run --rm -v "$PWD:/repo" -w /repo \
  -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
bash scripts/gate.sh up
docker logs s2script-cs2-gd-baseline 2>&1 \
  | grep -E "gamedata (OK|FAIL)|GAMEDATA VALIDATION" > /tmp/gd-baseline.txt
bash scripts/gate.sh destroy && cd - && git worktree remove /tmp/gd-baseline
```

Any difference in the OK/FAIL set is a loader bug, not a data bug — Task 3 Step 4 already proved the data is equal.

- [ ] **Step 9: Commit**

```bash
git add shim/src/s2script_mm.cpp shim/src/gamedata.h shim/src/gamedata.cpp \
        scripts/package-addon.sh scripts/check-gamedata-sigs.sh
git commit -m "shim: read the owner-scoped gamedata tree; retire the flat file"
```

---

### Task 7: Documentation and the standing-convention amendment

**Files:**
- Modify: `docs/ARCHITECTURE.md` (portability-boundary section)
- Modify: `docs/INSTALL.md` (after-a-CS2-update steps)
- Modify: `CLAUDE.md` (repository-layout `gamedata/` line)
- Modify: `docs/PROGRESS.md` (append the slice entry)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

> **Not in this task:** the `keys` amendment to CLAUDE.md's "Layout is data, semantics are code" bullet. It lands in the **A5b** PR, the one that actually introduces a `keys` section, so the convention and the code that needs it change together (spec §7, §12).

- [ ] **Step 1: `docs/ARCHITECTURE.md`**

Add to the portability-boundary section:

```markdown
### Gamedata ownership

Gamedata is partitioned on two **orthogonal** axes, ported from SourceMod:

- **Owner** — the directory (`gamedata/core/`, `gamedata/cs2/`), plus each plugin's own file inside
  its `.s2sp`. Answers *who consumes this fact*. Namespaced: two owners may define the same key
  with different values and neither sees the other.
- **Target** — the file within it (`common` / `engine.<engine>` / `game.<mod>`), selected by that
  owner's `master.gamedata.jsonc`. Answers *what this fact is resolved against*.

`gamedata/core/game.cs2.jsonc` is therefore core's *own* facts resolved against *CS2's*
`libserver.so`. Both are true at once. Porting to a second Source 2 game adds a sibling
`game.<mod>.jsonc` with the same key set and different bytes; it moves nothing between owners.

The ownership rule is mechanical — **a key belongs to whoever names it in source** — and
`scripts/check-gamedata-owners.sh` enforces it in both directions.

`<owner>/custom/*.jsonc` is applied last and is the operator's channel for hot-fixing a signature
after a CS2 update without waiting for a release. Overridden entries are named in the boot banner
and marked in the crash-report fingerprint.
```

- [ ] **Step 2: `docs/INSTALL.md`**

Add to the after-a-CS2-update section:

```markdown
### Hot-fixing a broken signature

If the boot banner reports `gamedata FAIL <Name> — signature NOT FOUND (moved — regenerate)`, you
do not have to wait for a release. Create `addons/s2script/gamedata/core/custom/fix.jsonc` with
just the entries you are replacing:

```jsonc
{
  "signatures": {
    "SetModel": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 ...",
        "resolve": "direct"
      }
    }
  }
}
```

Files in `custom/` are applied after everything shipped, in sorted filename order, replacing whole
named entries. Never edit the shipped files — an upgrade overwrites them. Overrides are announced
at boot and recorded in any crash report, so a patched signature is never mistaken for a shipped
one.
```

- [ ] **Step 3: `CLAUDE.md`**

Replace the `gamedata/` line in the repository-layout block:

```
gamedata/    Regenerable engine facts, split by OWNER (core/ = what shim+core name in source; cs2/ = the game package's) and by TARGET (common / engine.<engine> / game.<mod>, selected by each owner's master.gamedata.jsonc). custom/ = operator overrides, applied last.
```

- [ ] **Step 4: `docs/PROGRESS.md`**

Append:

```markdown
### A5a — gamedata tiering loader & tree (2026-08-01)

Ported SourceMod's gamedata organization: owner-scoped namespaces (`gamedata/core/`,
`gamedata/cs2/`), a `master.gamedata.jsonc` array index selecting files by `engine`/`game`, and a
`custom/` operator override channel applied last. The flat `gamedata/core.gamedata.jsonc` is gone;
its 46 entries split 44/2 by the mechanical ownership rule (*a key belongs to whoever names it in
source*), the two unnamed ones being the parked `CCSPlayer_ItemServices_*` offsets, which seeded
the new `cs2` owner. Verified byte-identical: the merged tree equals the pre-split file entry for
entry, and the live boot banner reports the same OK/FAIL set as `main`.

Master is an **array**, not an object, because plain `nlohmann::json` is `std::map` and sorts
object keys — an object master's apply order would be decided by filename spelling.

New gates: `scripts/test-gamedata.sh` (loader unit tests) and
`scripts/check-gamedata-owners.sh` (ownership rule, both directions, plus master/disk agreement).

Next: **A5b** retires the 8 CS2-API ops into `gamedata/cs2/` as `calls` descriptors — which needs a
`vtable-member` validator (`Respawn`'s RTTI gate has no equivalent in the `calls` format) and moves
the `Respawn`/`TerminateRound` next-frame drains into `pawn.js`, preserving dedupe and
consume-before-call.
```

- [ ] **Step 5: Full gate + PR**

Run: `CI=1 make ci`
Expected: both suites pass.

```bash
git add docs/ARCHITECTURE.md docs/INSTALL.md CLAUDE.md docs/PROGRESS.md
git commit -m "docs: gamedata ownership model, operator override channel, A5a progress"
git push -u origin gamedata/tiering-loader
```

Write the PR body to a file and use `gh pr create --body-file` — never a heredoc, which mangles
tables and code blocks. The body must open with **Why**: core had one flat gamedata file mixing
its own engine facts with CS2 API signatures, so the game-package boundary existed in doctrine but
not on disk, and an operator whose signature broke after a CS2 update had no path but to wait for a
release. Include the byte-identical evidence from Task 3 Step 4 and Task 6 Step 8.

---

## Self-Review

**Spec coverage (§8 A5a):**

| Spec requirement | Task |
|---|---|
| Owner-scoped `GameConfig`, master parsing, condition matching | 1 |
| `custom/` merge | 1 (code), 2 (tests) |
| moddir detection | 5 |
| Flat file → `core/{common,engine.source2,game.cs2}` | 3 |
| `gamedata/cs2/` wired, carrying the 2 parked offsets (§4.1) | 3 |
| `package-addon.sh` / `check-gamedata-sigs.sh` updated | 6 |
| `check-gamedata-owners.sh` added to `ci-native.sh` | 4 |
| "do not edit" headers on shipped files | 3 (header comments), 7 (INSTALL) |
| Override logged at boot + marked in crash fingerprint | 6 |
| Byte-identical gate over all 46 entries | 3 (data), 6 (live banner) |
| `keys` parsed by the loader, rejected under `core/` | 1 (parse), 4 (gate) |
| Docs: ARCHITECTURE / INSTALL / CLAUDE.md / PROGRESS | 7 |

`package-release.sh` is listed in the spec but needs **no** change — verified: it copies the whole
packaged addon tree that `package-addon.sh` produces (`scripts/package-release.sh:74`) and never
names a gamedata file.

**Deviations from the spec, both deliberate:**
1. The 2 parked `CCSPlayer_ItemServices_*` offsets move in A5a rather than A5b, so the ownership
   gate needs no allowlist and the `cs2` owner is loader-exercised from its first commit. The spec
   was amended to match (§4.1, §8).
2. The `keys` amendment to CLAUDE.md's "Layout is data, semantics are code" bullet is deferred to
   A5b, where a `keys` section actually appears.

**Type consistency:** `GameConfig` fields (`interfaces`, `offsets`, `signatures`, `keys`,
`overridden`, `filesLoaded`) and `LoadGameConfig`'s six-parameter signature are identical in the
header (Task 1 Step 4), the implementation (Step 5), every test (Tasks 1–2), and every call site
(Task 6). `SigSpec` keeps its existing three fields, so `ResolveSigValidated` needs no change.
`AddonRoot` / `GamedataRoot` / `DetectModDir` (Task 5) are the only names Task 6 consumes from
Task 5.

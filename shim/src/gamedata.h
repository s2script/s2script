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

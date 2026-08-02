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
    // The entry's `validate` object as RAW JSON TEXT (empty when it declares none). Kept unparsed
    // for the same reason `calls` is: the validator vocabulary is closed in call_validate.cpp, and a
    // second copy of it here would be the two-implementations-that-disagree bug this loader already
    // hit twice with its JSONC strippers.
    //
    // A validator is authored NEXT TO the pattern it guards, and this field is what carries it to
    // core (which lifts it onto the descriptor in `flatten_decl`). The override channel is why:
    // custom/*.jsonc replaces at the NAMED-ENTRY level, and the entry an operator replaces after a
    // CS2 update is the SIGNATURE — a new pattern. A validator whose offsets lived in another
    // section would then be checked against the shipped instruction layout the new pattern just made
    // meaningless, so a correct hot-fix would be spuriously rejected — or worse, coincidentally
    // pass. Dropping it here instead (which this struct used to do) is worse still: the descriptor
    // would resolve with NO semantic gate at all and nobody would be told.
    //
    // WHICH IS WHY THIS ONE FIELD IS **NOT** REPLACED WHOLESALE BY AN OPERATOR OVERRIDE. It is the
    // single exception to the named-entry replacement rule, and it is a deliberate one: an operator
    // hot-fixing a moved pattern writes `module`/`pattern`/`resolve` and stops, because those are
    // what "the signature" means to them. Taking the omission literally would silently DELETE the
    // semantic gate — and `gamedata/cs2/game.cs2.jsonc` records, by name, a borrowed TerminateRound
    // signature that matches UNIQUELY at the WRONG function, which uniqueness and the .text-range
    // check both wave through. The documented repair procedure would then be memory corruption.
    // So an override that omits `validate` INHERITS the previous entry's (reported in
    // GameConfig::validatorsCarried); an override that means it writes `"validate": {}` explicitly
    // (reported in GameConfig::validatorsDisarmed). Both are loud boot-banner lines.
    std::string validate;
};

// One owner's merged view, for one platform.
struct GameConfig {
    std::map<std::string, std::string> interfaces;
    std::map<std::string, int>         offsets;
    std::map<std::string, SigSpec>     signatures;
    // Per-game behavioural strings (spec §7). Permitted in the game/plugin tiers ONLY;
    // scripts/check-gamedata-owners.sh fails a `keys` section under gamedata/core/.
    std::map<std::string, std::string> keys;

    // `calls` — declared engine-call descriptors, kept as each entry's RAW JSON TEXT rather than a
    // parsed shape. The descriptor grammar (target kinds, validators, arg/return vocabulary) is
    // CORE's, and a second implementation of it here is exactly the two-implementations-that-
    // disagree bug this loader already hit twice with its JSONC strippers. The shim's whole job is
    // the merge; replacement is still at the named-entry level, like every other section.
    std::map<std::string, std::string> calls;

    // The merged view re-serialised as JSON text, for core (spec §9.1b). The tree / master /
    // condition / custom merge stays HERE, in one place; core consumes the result through the
    // gamedata_calls path it already has for a plugin's packed gamedata.json.
    //
    // Carries `signatures` and `calls` — exactly the two sections that path reads. `signatures` is
    // re-nested under the platform key it was lifted from, because core's flatten step looks up
    // `signatures[name][platform]`; an operator's custom/ override therefore reaches a descriptor
    // the same way it reaches the shim's own resolves. Empty when neither section merged.
    std::string mergedJson;

    // Entry names whose final value came from a custom/ override, for the boot banner and the
    // crash fingerprint — an operator's patched signature must never be silently attributed
    // to a shipped one.
    std::set<std::string> overridden;

    // Signature names where a custom/ override supplied a new pattern but NO `validate`, so the
    // PREVIOUS entry's validator was carried forward onto it (see SigSpec::validate). Reported at
    // boot because the carried gate is now checked against a pattern it was not derived for: if the
    // operator's pattern moved the instruction the validator reads, the descriptor will degrade with
    // the validator's reason and the operator has to know that is why.
    std::vector<std::string> validatorsCarried;
    // Signature names where a custom/ override REPLACED a real validator with an explicitly empty
    // one (`"validate": {}` / `null`) — the deliberate "I want no semantic gate" spelling. Reported
    // at boot at the same volume: that descriptor now resolves on uniqueness and a .text-range check
    // alone, which is exactly the state the validator vocabulary exists to prevent.
    std::vector<std::string> validatorsDisarmed;
    // Every file actually applied, in apply order (for the banner and the fingerprint hash).
    std::vector<std::string> filesLoaded;

    // SHIPPED files that were SELECTED by the master (listed, and their engine/game condition
    // matched) but could not be applied at all — unreadable, or unparseable JSON. This is the
    // catastrophic class, distinct from a per-entry type error: our own shipped tree is broken or
    // half-deployed, so the merged view is missing whole descriptor sets while every remaining
    // value still reads as if it loaded fine. The shim gates interface acquisition on this
    // (s2script_mm.cpp) — filesLoaded alone cannot see it, because an unconditional file that
    // applied first leaves filesLoaded non-empty no matter what fails after it.
    //
    // A custom/ file that fails to parse is NOT recorded here: the shipped tree is intact in that
    // case, so the right response is a loud named `error` (which it already gets, and which the
    // boot banner reports), not disabling the whole engine surface over an operator typo.
    std::vector<std::string> filesFailed;

    // Files that applied without failing but contributed ZERO entries, in apply order. For a
    // custom/ file this is the operator's most likely mistake — a wrong section name, a wrong
    // platform key, or the flat (non-platform-keyed) shape — and it otherwise looks exactly like
    // a successful override. Shipped placeholder files (common.gamedata.jsonc is `{}` today)
    // legitimately land here too, so the shim reports the two at different volumes.
    std::vector<std::string> filesEmpty;
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
// wins, replacing at the NAMED-ENTRY level — one `signatures.<key>` is swapped wholesale, never
// deep-merged. That is what lets an operator override be a one-entry file. The single exception is
// a signature's `validate` block under a custom/ override, which is INHERITED when omitted rather
// than deleted (SigSpec::validate; reported in validatorsCarried / validatorsDisarmed).
//
// `error` is empty on success. A missing master, an unparseable file, a listed-but-absent file, or
// a single malformed ENTRY sets it with a NAMED reason and returns whatever was merged before the
// failure: the caller degrades that owner, and the framework keeps running. `error` alone cannot
// tell those apart — see `filesFailed` for the whole-file (catastrophic) signal.
GameConfig LoadGameConfig(const std::string& gamedataRoot,
                          const std::string& owner,
                          const std::string& engine,
                          const std::string& game,
                          const std::string& platform,
                          std::string& error);

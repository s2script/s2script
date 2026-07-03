#pragma once
#include <map>
#include <string>

// Reads interface version strings from the "interfaces" section of a gamedata .jsonc file.
// Returns an empty map (and leaves `error` set) on failure — caller degrades, never crashes.
std::map<std::string, std::string> LoadInterfaceVersions(const std::string& path, std::string& error);

// Reads platform-keyed byte offsets from the "offsets" section of a gamedata .jsonc file.
// `platform` is the platform key (e.g. "linuxsteamrt64").
// Returns a map of offset-name → value for the given platform, or an empty map on failure.
// `error` is left empty on success (including when the "offsets" section is absent); set on
// parse failure.  Absent "offsets" is not an error — the caller degrades gracefully.
std::map<std::string, int> LoadOffsets(const std::string& path,
                                        const std::string& platform,
                                        std::string& error);

// A byte-signature spec: which module to scan, the IDA-style pattern, and the resolve strategy.
struct SigSpec {
    std::string module;
    std::string pattern;
    std::string resolve;
};

// Reads platform-keyed byte signatures from the "signatures" section of a gamedata .jsonc file.
// Returns a map of signature-name → SigSpec for `platform`, or an empty map. `error` is left empty
// on success (including when the "signatures" section is absent); set on parse failure.
std::map<std::string, SigSpec> LoadSignatures(const std::string& path,
                                              const std::string& platform,
                                              std::string& error);

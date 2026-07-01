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

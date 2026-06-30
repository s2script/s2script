#pragma once
#include <map>
#include <string>

// Reads interface version strings from a gamedata .jsonc file.
// Returns an empty map (and leaves `error` set) on failure — caller degrades, never crashes.
std::map<std::string, std::string> LoadInterfaceVersions(const std::string& path, std::string& error);

#pragma once
#include <cstddef>
#include <string>
namespace s2::function_overrides {
inline constexpr std::size_t MaxFiles=64, MaxFileBytes=256*1024, MaxTotalBytes=4*1024*1024, MaxDirectoryEntries=1024;
std::string PluginDirectory(const std::string& id);
// JSON {records:[{relative_path,sha256,content}],error:null|string}. Missing custom is normal.
std::string Snapshot(const std::string& addonRoot,const std::string& id);
}
// Main-thread transient storage; core copies before its next engine operation. Never throws.
const char* s2_plugin_function_overrides(const char* id);

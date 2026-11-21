// The platform seam: every question the shim asks the OS about its own process.
//
// Two implementations, chosen by CMake: platform_posix.cpp (Linux, the shipped build) and
// platform_win.cpp (Windows x64). Call sites keep their own selection rules (for example "largest
// executable segment wins", which keeps Metamod's thin game-module proxy from shadowing the real
// module); the seam only answers "what is loaded, where, and is it readable".
//
// See docs/superpowers/specs/2026-11-18-windows-port-design.md.
#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <string>

// A Source 2 module's file name from its stem: S2_MODULE("server") is "libserver.so" on Linux and
// "server.dll" on Windows. Call sites match it as a substring of the loaded module's path.
#ifdef _WIN32
#define S2_MODULE(stem) stem ".dll"
// steam_api is the one module whose Windows name is not the stem plus ".dll".
#define S2_STEAM_API_MODULE "steam_api64.dll"
// Where tier0's C exports are looked up: Windows has no global symbol namespace.
#define S2_TIER0_EXPORT_SCOPE "tier0.dll"
#define S2_CORE_LIBRARY_FILE "s2script_core.dll"
#else
#define S2_MODULE(stem) "lib" stem ".so"
#define S2_STEAM_API_MODULE "libsteam_api.so"
#define S2_TIER0_EXPORT_SCOPE nullptr   // RTLD_DEFAULT: tier0 is in the global namespace
#define S2_CORE_LIBRARY_FILE "libs2script_core.so"
#endif

namespace s2plat {

// The gamedata platform key, which is also the name of the addon's bin/ subdirectory.
const char* PlatformKey();

// The shim binary's own path, with '/' separators on every platform. "" if it cannot be found.
std::string SelfModulePath();

// The addon root (<mod>/addons/s2script): three directories above SelfModulePath(). "" if the
// shim's own path is unknown.
std::string AddonRoot();

// Everything before the last '/' ("" when there is none). The path is not touched otherwise.
std::string Dirname(const std::string& path);
// Everything after the last '/'.
std::string Basename(const std::string& path);

// One mapped piece of a loaded module: an ELF PT_LOAD segment or a PE section (plus the PE headers,
// as a non-executable piece at the image base).
struct Segment {
    uintptr_t start;
    size_t fileSize;   // bytes backed by the file (ELF p_filesz; PE min(raw size, virtual size))
    size_t memSize;    // bytes mapped (ELF p_memsz; PE virtual size)
    bool exec;
};

struct ModuleView {
    const char* path;      // as the loader reports it; never null (may be "")
    uintptr_t base;        // ELF load bias / PE image base
    const Segment* segs;
    int count;
};

// Visit every loaded module. Return true from `fn` to stop early.
void ForEachModule(const std::function<bool(const ModuleView&)>& fn);

// An exported symbol. `module` is matched as a substring of the loaded module's path; nullptr
// searches the global namespace on Linux (RTLD_DEFAULT) and every loaded module on Windows.
void* FindExport(const char* module, const char* symbol);

// Whether the page containing `p` is mapped and readable. Never faults.
bool IsReadable(const void* p);

// Every executable mapping in the process, [lo, hi).
struct Range { uintptr_t lo, hi; };
void ExecutableRanges(std::function<void(Range)> fn);

size_t PageSize();

// The calling OS thread's id (Linux gettid, Windows GetCurrentThreadId).
long ThreadId();

// Load a library that sits next to the shim binary, by full path, so later imports of it bind to this
// copy. Windows only (core is delay-loaded there, and the loader never searches a plugin's own
// directory); Linux finds core through the shim's $ORIGIN rpath and this returns true untouched.
bool PreloadSibling(const char* fileName, std::string& error);

// mkdir for one directory level; true if it exists afterwards.
bool MakeDir(const std::string& path);

}  // namespace s2plat

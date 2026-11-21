// Windows x64 implementation of the platform seam (platform.h).
//
// Module views come from the PE headers of each loaded image, read in place: the loader maps the
// headers at the image base, so no file is opened. A section is one Segment; the headers are one
// more, non-executable, at the base, so the lowest segment start is the image base exactly as an
// ELF module's lowest PT_LOAD is its first mapped byte.
#include "platform.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <psapi.h>

#include <cstring>
#include <vector>

namespace s2plat {

namespace {

std::string Narrow(const wchar_t* w, int len = -1) {
    int n = WideCharToMultiByte(CP_UTF8, 0, w, len, nullptr, 0, nullptr, nullptr);
    if (n <= 0) return std::string();
    std::string out(static_cast<size_t>(n), '\0');
    WideCharToMultiByte(CP_UTF8, 0, w, len, out.data(), n, nullptr, nullptr);
    if (len < 0 && !out.empty() && out.back() == '\0') out.pop_back();
    return out;
}

std::string Slashes(std::string s) {
    for (char& c : s) if (c == '\\') c = '/';
    return s;
}

std::string ModulePath(HMODULE h) {
    std::vector<wchar_t> buf(32768);
    DWORD n = GetModuleFileNameW(h, buf.data(), static_cast<DWORD>(buf.size()));
    if (n == 0 || n >= buf.size()) return std::string();
    return Slashes(Narrow(buf.data(), static_cast<int>(n)));
}

// The shim's file I/O (fopen, stat, std::ifstream) takes narrow paths, which Windows reads in the
// ANSI code page, not UTF-8. For a path with non-ASCII characters, use its 8.3 short form, which is
// ASCII, when the volume has short names. Otherwise the UTF-8 path is returned and such a file
// open fails with its usual error.
std::string FileIoPath(HMODULE h) {
    std::vector<wchar_t> buf(32768);
    DWORD n = GetModuleFileNameW(h, buf.data(), static_cast<DWORD>(buf.size()));
    if (n == 0 || n >= buf.size()) return std::string();
    bool ascii = true;
    for (DWORD i = 0; i < n; i++) if (buf[i] > 0x7f) { ascii = false; break; }
    if (!ascii) {
        std::vector<wchar_t> shortBuf(32768);
        DWORD m = GetShortPathNameW(buf.data(), shortBuf.data(), static_cast<DWORD>(shortBuf.size()));
        bool shortAscii = m > 0 && m < shortBuf.size();
        for (DWORD i = 0; shortAscii && i < m; i++) if (shortBuf[i] > 0x7f) shortAscii = false;
        if (shortAscii) return Slashes(Narrow(shortBuf.data(), static_cast<int>(m)));
    }
    return Slashes(Narrow(buf.data(), static_cast<int>(n)));
}

std::vector<HMODULE> LoadedModules() {
    std::vector<HMODULE> mods(512);
    for (;;) {
        DWORD needed = 0;
        if (!EnumProcessModules(GetCurrentProcess(), mods.data(),
                                static_cast<DWORD>(mods.size() * sizeof(HMODULE)), &needed))
            return {};
        const size_t count = needed / sizeof(HMODULE);
        if (count <= mods.size()) { mods.resize(count); return mods; }
        mods.resize(count + 64);   // modules loaded between the two calls: ask again
    }
}

// The image's sections as Segments. False if the headers do not look like a PE32+ image.
bool Sections(HMODULE h, std::vector<Segment>& out) {
    const auto base = reinterpret_cast<uintptr_t>(h);
    const auto* dos = reinterpret_cast<const IMAGE_DOS_HEADER*>(base);
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return false;
    const auto* nt = reinterpret_cast<const IMAGE_NT_HEADERS64*>(base + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE ||
        nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC) return false;
    out.push_back({base, nt->OptionalHeader.SizeOfHeaders, nt->OptionalHeader.SizeOfHeaders, false});
    const IMAGE_SECTION_HEADER* sec = IMAGE_FIRST_SECTION(nt);
    for (WORD i = 0; i < nt->FileHeader.NumberOfSections; i++, sec++) {
        const size_t vsize = sec->Misc.VirtualSize ? sec->Misc.VirtualSize : sec->SizeOfRawData;
        const size_t fsize = sec->SizeOfRawData < vsize ? sec->SizeOfRawData : vsize;
        out.push_back({base + sec->VirtualAddress, fsize, vsize,
                       (sec->Characteristics & IMAGE_SCN_MEM_EXECUTE) != 0});
    }
    return true;
}

}  // namespace

const char* PlatformKey() { return "win64"; }

std::string SelfModulePath() {
    HMODULE self = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(&SelfModulePath), &self))
        return std::string();
    return FileIoPath(self);
}

std::string Dirname(const std::string& path) {
    const size_t slash = path.find_last_of('/');
    return slash == std::string::npos ? std::string() : path.substr(0, slash);
}

std::string Basename(const std::string& path) {
    const size_t slash = path.find_last_of('/');
    return slash == std::string::npos ? path : path.substr(slash + 1);
}

std::string AddonRoot() {
    std::string self = SelfModulePath();
    if (self.empty()) return self;
    // <root>/bin/win64/s2script.dll: up three.
    return Dirname(Dirname(Dirname(self)));
}

void ForEachModule(const std::function<bool(const ModuleView&)>& fn) {
    for (HMODULE h : LoadedModules()) {
        std::vector<Segment> segs;
        if (!Sections(h, segs)) continue;
        const std::string path = ModulePath(h);
        ModuleView view{path.c_str(), reinterpret_cast<uintptr_t>(h), segs.data(),
                        static_cast<int>(segs.size())};
        if (fn(view)) return;
    }
}

void* FindExport(const char* module, const char* symbol) {
    for (HMODULE h : LoadedModules()) {
        if (module) {
            const std::string path = ModulePath(h);
            if (!std::strstr(path.c_str(), module)) continue;
        }
        if (FARPROC p = GetProcAddress(h, symbol)) return reinterpret_cast<void*>(p);
    }
    return nullptr;
}

bool IsReadable(const void* p) {
    MEMORY_BASIC_INFORMATION mbi;
    if (VirtualQuery(p, &mbi, sizeof mbi) != sizeof mbi) return false;
    if (mbi.State != MEM_COMMIT) return false;
    if (mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD)) return false;
    const DWORD readable = PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READ |
                           PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    return (mbi.Protect & readable) != 0;
}

void ExecutableRanges(std::function<void(Range)> fn) {
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    auto at = reinterpret_cast<uintptr_t>(si.lpMinimumApplicationAddress);
    const auto end = reinterpret_cast<uintptr_t>(si.lpMaximumApplicationAddress);
    const DWORD exec = PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    MEMORY_BASIC_INFORMATION mbi;
    while (at < end && VirtualQuery(reinterpret_cast<void*>(at), &mbi, sizeof mbi) == sizeof mbi) {
        const auto lo = reinterpret_cast<uintptr_t>(mbi.BaseAddress);
        const auto hi = lo + mbi.RegionSize;
        if (mbi.State == MEM_COMMIT && (mbi.Protect & exec) && !(mbi.Protect & PAGE_GUARD))
            fn({lo, hi});
        if (hi <= at) break;   // no progress: stop rather than spin
        at = hi;
    }
}

size_t PageSize() {
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    return si.dwPageSize ? si.dwPageSize : 4096;
}

bool PreloadSibling(const char* fileName, std::string& error) {
    const std::string dir = Dirname(SelfModulePath());
    if (dir.empty()) { error = "cannot find the shim's own directory"; return false; }
    const std::string path = dir + "/" + fileName;
    int n = MultiByteToWideChar(CP_UTF8, 0, path.c_str(), -1, nullptr, 0);
    std::wstring w(n > 0 ? static_cast<size_t>(n) : 0, L'\0');
    if (n > 0) MultiByteToWideChar(CP_UTF8, 0, path.c_str(), -1, w.data(), n);
    for (wchar_t& c : w) if (c == L'/') c = L'\\';
    // LOAD_WITH_ALTERED_SEARCH_PATH: the library's own dependencies resolve from its directory too.
    if (LoadLibraryExW(w.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH)) return true;
    error = path + ": LoadLibraryEx failed, error " + std::to_string(GetLastError());
    return false;
}

long ThreadId() { return static_cast<long>(GetCurrentThreadId()); }

bool MakeDir(const std::string& path) {
    std::wstring w;
    int n = MultiByteToWideChar(CP_UTF8, 0, path.c_str(), -1, nullptr, 0);
    if (n <= 0) return false;
    w.resize(static_cast<size_t>(n));
    MultiByteToWideChar(CP_UTF8, 0, path.c_str(), -1, w.data(), n);
    if (CreateDirectoryW(w.c_str(), nullptr)) return true;
    return GetLastError() == ERROR_ALREADY_EXISTS;
}

}  // namespace s2plat

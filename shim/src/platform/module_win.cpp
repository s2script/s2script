#include "module.h"

#include "pe_image.h"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <psapi.h>

#include <algorithm>
#include <cwctype>
#include <utility>
#include <vector>

namespace s2platform {
namespace {

std::wstring ModulePath(HMODULE module) {
    std::vector<wchar_t> buffer(512);
    for (;;) {
        const DWORD length = GetModuleFileNameW(module, buffer.data(),
                                                static_cast<DWORD>(buffer.size()));
        if (length == 0) return {};
        if (length < buffer.size()) return std::wstring(buffer.data(), length);
        buffer.resize(buffer.size() * 2);
    }
}

std::string Utf8(const std::wstring& value) {
    if (value.empty()) return {};
    const int size = WideCharToMultiByte(CP_UTF8, 0, value.data(),
                                         static_cast<int>(value.size()),
                                         nullptr, 0, nullptr, nullptr);
    if (size <= 0) return {};
    std::string result(static_cast<size_t>(size), '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()),
                        result.data(), size, nullptr, nullptr);
    return result;
}

std::wstring Wide(const char* value) {
    if (!value || !value[0]) return {};
    const int size = MultiByteToWideChar(CP_UTF8, 0, value, -1, nullptr, 0);
    if (size <= 1) return {};
    std::wstring result(static_cast<size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, value, -1, result.data(), size);
    result.pop_back();
    return result;
}

std::vector<HMODULE> Modules() {
    std::vector<HMODULE> modules(256);
    for (;;) {
        DWORD needed = 0;
        if (!EnumProcessModulesEx(GetCurrentProcess(), modules.data(),
                                  static_cast<DWORD>(modules.size() * sizeof(HMODULE)),
                                  &needed, LIST_MODULES_ALL)) {
            return {};
        }
        if (needed <= modules.size() * sizeof(HMODULE)) {
            modules.resize(needed / sizeof(HMODULE));
            return modules;
        }
        modules.resize(needed / sizeof(HMODULE));
    }
}

bool ParseModule(HMODULE module, ModuleView* out) {
    MODULEINFO info{};
    if (!GetModuleInformation(GetCurrentProcess(), module, &info, sizeof(info))) return false;
    auto* base = static_cast<const uint8_t*>(info.lpBaseOfDll);
    if (!ParsePeImage(base, static_cast<size_t>(info.SizeOfImage), out)) return false;
    out->path = Utf8(ModulePath(module));
    return true;
}

}  // namespace

ModuleView FindModule(const char* name) {
    std::wstring wanted = Wide(name);
    if (wanted.empty()) return {};
    std::transform(wanted.begin(), wanted.end(), wanted.begin(),
                   [](wchar_t c) { return static_cast<wchar_t>(std::towlower(c)); });

    ModuleView best;
    for (HMODULE module : Modules()) {
        std::wstring path = ModulePath(module);
        std::transform(path.begin(), path.end(), path.begin(),
                       [](wchar_t c) { return static_cast<wchar_t>(std::towlower(c)); });
        if (path.find(wanted) == std::wstring::npos) continue;
        ModuleView candidate;
        if (ParseModule(module, &candidate) && candidate.textSize > best.textSize)
            best = std::move(candidate);
    }
    return best;
}

bool ModuleViewForAddress(const void* address, ModuleView* out) {
    if (!address || !out) return false;
    for (HMODULE module : Modules()) {
        MODULEINFO info{};
        if (!GetModuleInformation(GetCurrentProcess(), module, &info, sizeof(info))) continue;
        auto* base = static_cast<const uint8_t*>(info.lpBaseOfDll);
        ModuleView candidate;
        if (PeModuleViewForAddress(base, static_cast<size_t>(info.SizeOfImage),
                                   address, &candidate)) {
            candidate.path = Utf8(ModulePath(module));
            *out = std::move(candidate);
            return true;
        }
    }
    return false;
}

bool IsExecutableAddress(const void* address) {
    ModuleView ignored;
    return ModuleViewForAddress(address, &ignored);
}

bool IsReadableAddress(const void* address, size_t size) {
    if (!address || !size) return false;
    uintptr_t cursor = reinterpret_cast<uintptr_t>(address);
    const uintptr_t end = cursor + size;
    if (end < cursor) return false;
    while (cursor < end) {
        MEMORY_BASIC_INFORMATION info{};
        if (!VirtualQuery(reinterpret_cast<const void*>(cursor), &info, sizeof(info)) ||
            info.State != MEM_COMMIT || (info.Protect & (PAGE_NOACCESS | PAGE_GUARD))) {
            return false;
        }
        const uintptr_t regionEnd = reinterpret_cast<uintptr_t>(info.BaseAddress) + info.RegionSize;
        if (regionEnd <= cursor) return false;
        cursor = regionEnd;
    }
    return true;
}

}  // namespace s2platform

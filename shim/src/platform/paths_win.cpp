#include "paths.h"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <vector>

namespace s2platform {
namespace {

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

}  // namespace

std::string AddonRoot(const void* anchor) {
    if (!anchor) return {};
    HMODULE module = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            reinterpret_cast<LPCWSTR>(anchor), &module)) {
        return {};
    }

    std::vector<wchar_t> buffer(512);
    for (;;) {
        const DWORD length = GetModuleFileNameW(module, buffer.data(),
                                                static_cast<DWORD>(buffer.size()));
        if (length == 0) return {};
        if (length < buffer.size())
            return AddonRootFromModulePath(Utf8(std::wstring(buffer.data(), length)));
        buffer.resize(buffer.size() * 2);
    }
}

}  // namespace s2platform

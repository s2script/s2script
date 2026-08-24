// Windows crash-handler selftest: launch a child that installs a prior top-level exception
// filter, arms Breakpad, and raises an access violation. The parent verifies that the crash was
// not swallowed, the prior filter ran, and one byte-exact <uuid>.dmp.s2meta accompanies its dump.
#include "crash_handler.h"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <stdint.h>
#include <stdio.h>

#include <cstring>
#include <string>
#include <vector>

static uint8_t g_breadcrumb[128];
static wchar_t g_chainMarker[32768];

static void FillBreadcrumb() {
    for (size_t i = 0; i < sizeof(g_breadcrumb); ++i) {
        g_breadcrumb[i] = static_cast<uint8_t>(i * 7 + 1);
    }
}

static bool EndsWith(const std::wstring& value, const wchar_t* suffix) {
    const size_t suffixLength = wcslen(suffix);
    return value.size() >= suffixLength &&
           value.compare(value.size() - suffixLength, suffixLength, suffix) == 0;
}

static std::string WideToUtf8(const wchar_t* value) {
    if (!value || !value[0]) return {};
    const int size = WideCharToMultiByte(
        CP_UTF8, WC_ERR_INVALID_CHARS, value, -1, nullptr, 0, nullptr, nullptr);
    if (size <= 1) return {};
    std::string utf8(static_cast<size_t>(size), '\0');
    if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value, -1, utf8.data(), size,
                            nullptr, nullptr) != size) {
        return {};
    }
    utf8.resize(static_cast<size_t>(size - 1));
    return utf8;
}

static LONG WINAPI PriorExceptionFilter(EXCEPTION_POINTERS*) {
    HANDLE marker = CreateFileW(g_chainMarker, GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
                                FILE_ATTRIBUTE_NORMAL, nullptr);
    if (marker != INVALID_HANDLE_VALUE) CloseHandle(marker);
    return EXCEPTION_CONTINUE_SEARCH;
}

static int RunChild(const wchar_t* spoolDir) {
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    FillBreadcrumb();

    const std::wstring marker = std::wstring(spoolDir) + L"\\prior-handler-ran";
    if (marker.size() >= sizeof(g_chainMarker) / sizeof(g_chainMarker[0])) return 10;
    memcpy(g_chainMarker, marker.c_str(), (marker.size() + 1) * sizeof(wchar_t));
    SetUnhandledExceptionFilter(PriorExceptionFilter);

    const std::string spoolUtf8 = WideToUtf8(spoolDir);
    if (spoolUtf8.empty() ||
        !S2CrashArm(spoolUtf8.c_str(), g_breadcrumb, sizeof(g_breadcrumb))) {
        return 11;
    }
    if (S2CrashArm(spoolUtf8.c_str(), g_breadcrumb, sizeof(g_breadcrumb))) return 12;

    RaiseException(EXCEPTION_ACCESS_VIOLATION, EXCEPTION_NONCONTINUABLE, 0, nullptr);
    return 0;
}

static void RemoveTree(const std::wstring& directory) {
    WIN32_FIND_DATAW entry = {};
    HANDLE search = FindFirstFileW((directory + L"\\*").c_str(), &entry);
    if (search != INVALID_HANDLE_VALUE) {
        do {
            if (wcscmp(entry.cFileName, L".") == 0 || wcscmp(entry.cFileName, L"..") == 0)
                continue;
            const std::wstring path = directory + L"\\" + entry.cFileName;
            if (entry.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
                RemoveTree(path);
            } else {
                SetFileAttributesW(path.c_str(), FILE_ATTRIBUTE_NORMAL);
                DeleteFileW(path.c_str());
            }
        } while (FindNextFileW(search, &entry));
        FindClose(search);
    }
    RemoveDirectoryW(directory.c_str());
}

struct ScopedSpoolDirectory {
    std::wstring path;
    ~ScopedSpoolDirectory() {
        if (!path.empty()) RemoveTree(path);
    }
};

static bool MakeSpoolDir(std::wstring* result) {
    wchar_t tempDir[MAX_PATH];
    const DWORD tempLength = GetTempPathW(MAX_PATH, tempDir);
    if (!tempLength || tempLength >= MAX_PATH) return false;

    for (unsigned int attempt = 0; attempt < 32; ++attempt) {
        wchar_t uniquePath[MAX_PATH];
        const int length = swprintf_s(
            uniquePath, MAX_PATH, L"%ss2-crash-%lu-%llu-%u", tempDir,
            GetCurrentProcessId(), static_cast<unsigned long long>(GetTickCount64()), attempt);
        if (length <= 0 || length >= MAX_PATH) return false;
        if (CreateDirectoryW(uniquePath, nullptr)) {
            *result = uniquePath;
            return true;
        }
        if (GetLastError() != ERROR_ALREADY_EXISTS) return false;
    }
    return false;
}

static std::wstring ExecutablePath() {
    std::vector<wchar_t> path(32768);
    const DWORD length =
        GetModuleFileNameW(nullptr, path.data(), static_cast<DWORD>(path.size()));
    if (!length || length >= path.size()) return {};
    return std::wstring(path.data(), length);
}

static bool ReadExactBreadcrumb(const std::wstring& path) {
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
                              FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;

    LARGE_INTEGER size = {};
    uint8_t bytes[sizeof(g_breadcrumb)] = {};
    DWORD read = 0;
    const bool ok = GetFileSizeEx(file, &size) &&
                    size.QuadPart == static_cast<LONGLONG>(sizeof(bytes)) &&
                    ReadFile(file, bytes, sizeof(bytes), &read, nullptr) &&
                    read == sizeof(bytes);
    CloseHandle(file);
    return ok && memcmp(bytes, g_breadcrumb, sizeof(bytes)) == 0;
}

static bool DirectoryIsEmpty(const std::wstring& directory) {
    WIN32_FIND_DATAW entry = {};
    HANDLE search = FindFirstFileW((directory + L"\\*").c_str(), &entry);
    if (search == INVALID_HANDLE_VALUE) return false;
    bool empty = true;
    do {
        if (wcscmp(entry.cFileName, L".") != 0 && wcscmp(entry.cFileName, L"..") != 0) {
            empty = false;
            break;
        }
    } while (FindNextFileW(search, &entry));
    FindClose(search);
    return empty;
}

static bool CheckDisarmRestoresPriorFilter(const std::wstring& spoolDir) {
    const std::string spoolUtf8 = WideToUtf8(spoolDir.c_str());
    if (spoolUtf8.empty()) return false;

    LPTOP_LEVEL_EXCEPTION_FILTER original =
        SetUnhandledExceptionFilter(PriorExceptionFilter);
    if (!S2CrashArm(spoolUtf8.c_str(), g_breadcrumb, sizeof(g_breadcrumb))) {
        SetUnhandledExceptionFilter(original);
        return false;
    }
    const bool secondArmRejected =
        !S2CrashArm(spoolUtf8.c_str(), g_breadcrumb, sizeof(g_breadcrumb));
    S2CrashDisarm();
    LPTOP_LEVEL_EXCEPTION_FILTER restored =
        SetUnhandledExceptionFilter(original);
    return secondArmRejected && restored == PriorExceptionFilter && DirectoryIsEmpty(spoolDir);
}

int wmain(int argc, wchar_t** argv) {
    if (argc == 3 && wcscmp(argv[1], L"--child") == 0) return RunChild(argv[2]);

    FillBreadcrumb();
    ScopedSpoolDirectory spool;
    const std::wstring executable = ExecutablePath();
    if (!MakeSpoolDir(&spool.path) || executable.empty()) {
        fwprintf(stderr, L"FAIL: could not create the Windows crash selftest environment\n");
        return 1;
    }
    if (!CheckDisarmRestoresPriorFilter(spool.path)) {
        fwprintf(stderr, L"FAIL: disarm did not restore the prior filter or preflight leaked\n");
        return 1;
    }

    std::wstring commandLine =
        L"\"" + executable + L"\" --child \"" + spool.path + L"\"";
    std::vector<wchar_t> mutableCommand(commandLine.begin(), commandLine.end());
    mutableCommand.push_back(L'\0');

    STARTUPINFOW startup = {};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION process = {};
    if (!CreateProcessW(executable.c_str(), mutableCommand.data(), nullptr, nullptr, FALSE, 0,
                        nullptr, nullptr, &startup, &process)) {
        fwprintf(stderr, L"FAIL: CreateProcessW failed (%lu)\n", GetLastError());
        return 1;
    }
    CloseHandle(process.hThread);

    const DWORD wait = WaitForSingleObject(process.hProcess, 60000);
    DWORD exitCode = 0;
    if (wait != WAIT_OBJECT_0) {
        TerminateProcess(process.hProcess, 2);
        WaitForSingleObject(process.hProcess, 10000);
        CloseHandle(process.hProcess);
        fwprintf(stderr, L"FAIL: crash child did not exit\n");
        return 1;
    }
    if (!GetExitCodeProcess(process.hProcess, &exitCode)) {
        CloseHandle(process.hProcess);
        fwprintf(stderr, L"FAIL: could not read crash child exit code\n");
        return 1;
    }
    CloseHandle(process.hProcess);
    if (exitCode != static_cast<DWORD>(EXCEPTION_ACCESS_VIOLATION)) {
        fwprintf(stderr, L"FAIL: child exit was 0x%08lx, expected access violation 0x%08lx\n",
                 exitCode, static_cast<DWORD>(EXCEPTION_ACCESS_VIOLATION));
        return 1;
    }

    int dumpCount = 0;
    int metaCount = 0;
    std::wstring dumpName;
    std::wstring metaName;
    WIN32_FIND_DATAW entry = {};
    HANDLE search = FindFirstFileW((spool.path + L"\\*").c_str(), &entry);
    if (search != INVALID_HANDLE_VALUE) {
        do {
            if (entry.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) continue;
            const std::wstring name(entry.cFileName);
            if (EndsWith(name, L".dmp.s2meta")) {
                ++metaCount;
                metaName = name;
            } else if (EndsWith(name, L".dmp")) {
                ++dumpCount;
                dumpName = name;
            }
        } while (FindNextFileW(search, &entry));
        FindClose(search);
    }

    if (dumpCount != 1 || metaCount != 1 || metaName != dumpName + L".s2meta") {
        fwprintf(stderr, L"FAIL: expected one matching .dmp/.dmp.s2meta pair, got %d/%d\n",
                 dumpCount, metaCount);
        return 1;
    }
    if (GetFileAttributesW((spool.path + L"\\prior-handler-ran").c_str()) ==
        INVALID_FILE_ATTRIBUTES) {
        fwprintf(stderr, L"FAIL: prior unhandled-exception filter was not chained\n");
        return 1;
    }
    if (!ReadExactBreadcrumb(spool.path + L"\\" + metaName)) {
        fwprintf(stderr, L"FAIL: .s2meta breadcrumb content mismatch\n");
        return 1;
    }

    wprintf(L"OK: disarm restored; access violation chained; matching byte-exact crash pair\n");
    return 0;
}

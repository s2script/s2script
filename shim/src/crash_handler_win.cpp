// Vendored Breakpad Windows in-process capture + .s2meta sidecar writer.
//
// Breakpad invokes DumpCallback on its pre-created handler thread after MiniDumpWriteDump. Keep
// the callback bounded and allocation-free: only fixed-buffer copies and Win32 file APIs are used.
// Returning false is deliberate; Breakpad then invokes the previously installed top-level filter
// (or continues the system search), so capture never swallows the original crash.
#include "crash_handler.h"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include "client/windows/handler/exception_handler.h"

#include <string>

static google_breakpad::ExceptionHandler* s_handler = nullptr;
static const uint8_t* s_breadcrumb = nullptr;
static uint32_t s_breadcrumbSize = 0;

static bool BuildArtifactPath(const wchar_t* dumpPath,
                              const wchar_t* minidumpId,
                              const wchar_t* suffix,
                              wchar_t* output,
                              size_t capacity) {
    if (!dumpPath || !minidumpId || !suffix || !output || !capacity) return false;

    size_t length = 0;
    while (dumpPath[length] && length + 1 < capacity) {
        output[length] = dumpPath[length];
        ++length;
    }
    if (dumpPath[length]) return false;
    if (length && output[length - 1] != L'\\' && output[length - 1] != L'/') {
        if (length + 1 >= capacity) return false;
        output[length++] = L'\\';
    }

    size_t idLength = 0;
    while (minidumpId[idLength] && length + 1 < capacity) {
        output[length++] = minidumpId[idLength++];
    }
    if (minidumpId[idLength]) return false;

    size_t suffixLength = 0;
    while (suffix[suffixLength] && length + 1 < capacity) {
        output[length++] = suffix[suffixLength++];
    }
    if (suffix[suffixLength]) return false;
    output[length] = L'\0';
    return true;
}

struct PreflightState {
    wchar_t dumpPath[MAX_PATH];
    bool pathValid;
    bool deleted;
};

struct CallbackState {
    volatile LONG preflight;
    PreflightState result;
};

static CallbackState s_callbackState = {};

static bool PreflightCallback(const wchar_t* dumpPath,
                              const wchar_t* minidumpId,
                              PreflightState* state,
                              bool succeeded) {
    state->pathValid = BuildArtifactPath(
        dumpPath, minidumpId, L".dmp", state->dumpPath,
        sizeof(state->dumpPath) / sizeof(state->dumpPath[0]));
    state->deleted = succeeded && state->pathValid && DeleteFileW(state->dumpPath);
    return succeeded;
}

static bool DumpCallback(const wchar_t* dumpPath,
                         const wchar_t* minidumpId,
                         void* context,
                         EXCEPTION_POINTERS* exception,
                         MDRawAssertionInfo* /*assertion*/,
                         bool succeeded) {
    CallbackState* state = static_cast<CallbackState*>(context);
    if (state && InterlockedCompareExchange(&state->preflight, 0, 0) != 0 &&
        exception && exception->ExceptionRecord &&
        exception->ExceptionRecord->ExceptionCode == STATUS_NONCONTINUABLE_EXCEPTION) {
        return PreflightCallback(dumpPath, minidumpId, &state->result, succeeded);
    }

    if (succeeded && dumpPath && minidumpId && s_breadcrumb && s_breadcrumbSize) {
        wchar_t metaPath[MAX_PATH + 8];
        if (BuildArtifactPath(dumpPath, minidumpId, L".dmp.s2meta", metaPath,
                              sizeof(metaPath) / sizeof(metaPath[0]))) {
            HANDLE file = CreateFileW(metaPath, GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
                                      FILE_ATTRIBUTE_NORMAL, nullptr);
            if (file != INVALID_HANDLE_VALUE) {
                uint32_t offset = 0;
                bool complete = true;
                while (offset < s_breadcrumbSize) {
                    DWORD written = 0;
                    if (!WriteFile(file, s_breadcrumb + offset,
                                   s_breadcrumbSize - offset, &written, nullptr) ||
                        written == 0) {
                        complete = false;
                        break;
                    }
                    offset += written;
                }
                CloseHandle(file);
                if (!complete) DeleteFileW(metaPath);
            }
        }
    }

    return false;
}

bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize) {
    if (s_handler || !spoolDir || !spoolDir[0]) return false;

    const int wideLength = MultiByteToWideChar(
        CP_UTF8, MB_ERR_INVALID_CHARS, spoolDir, -1, nullptr, 0);
    if (wideLength <= 1) return false;
    std::wstring wideSpool(static_cast<size_t>(wideLength), L'\0');
    if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, spoolDir, -1,
                            wideSpool.data(), wideLength) != wideLength) {
        return false;
    }
    wideSpool.resize(static_cast<size_t>(wideLength - 1));

    // This pinned Breakpad revision constructs the dump filename in a MAX_PATH wchar_t buffer.
    // Reject a path it would truncate, leaving enough room for "\\<uuid>.dmp\0".
    constexpr size_t kBreakpadNameLength = 1 + 36 + 4 + 1;
    if (wideSpool.size() + kBreakpadNameLength > MAX_PATH) return false;

    s_breadcrumb = breadcrumb;
    s_breadcrumbSize = breadcrumbSize;
    InterlockedExchange(&s_callbackState.preflight, 0);
    s_callbackState.result = {};
    s_handler = new google_breakpad::ExceptionHandler(
        wideSpool, /*filter=*/nullptr, DumpCallback, &s_callbackState,
        google_breakpad::ExceptionHandler::HANDLER_EXCEPTION);

    InterlockedExchange(&s_callbackState.preflight, 1);
    const bool operational = s_handler->WriteMinidump();
    InterlockedExchange(&s_callbackState.preflight, 0);
    if (s_callbackState.result.pathValid && !s_callbackState.result.deleted) {
        s_callbackState.result.deleted = DeleteFileW(s_callbackState.result.dumpPath);
    }
    if (!operational || !s_callbackState.result.pathValid ||
        !s_callbackState.result.deleted) {
        S2CrashDisarm();
        return false;
    }
    return true;
}

void S2CrashDisarm(void) {
    delete s_handler;
    s_handler = nullptr;
    s_breadcrumb = nullptr;
    s_breadcrumbSize = 0;
    InterlockedExchange(&s_callbackState.preflight, 0);
    s_callbackState.result = {};
}

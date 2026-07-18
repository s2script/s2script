// Breakpad arming + the .s2meta sidecar writer (crash-reporter spec §6.2).
//
// GOLDEN RULE (spec §9): nothing reachable from DumpCallback may allocate, lock, format, or
// call non-async-signal-safe libc. The callback uses only open/write/close (POSIX AS-safe) and
// byte copies into fixed buffers, then returns false so previously installed handlers run —
// the process still dies / core-dumps exactly as it would have without us.
#include "crash_handler.h"
// <cstddef> before the Breakpad header: exception_handler.h transitively pulls in
// common/memory_allocator.h, which uses std::max_align_t but only includes <stddef.h>. Newer host
// libstdc++ provides std::max_align_t transitively; the Steam-Runtime-3 sniper toolchain (bullseye
// gcc 10) does not — so we include <cstddef> ourselves to keep the vendored submodule unpatched.
#include <cstddef>
#include "client/linux/handler/exception_handler.h"
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

static google_breakpad::ExceptionHandler* s_handler = nullptr;
static const uint8_t* s_breadcrumb = nullptr;
static uint32_t s_breadcrumbSize = 0;

static bool DumpCallback(const google_breakpad::MinidumpDescriptor& descriptor,
                         void* /*context*/, bool /*succeeded*/) {
    // ASYNC-SIGNAL-SAFE ONLY from here down.
    if (s_breadcrumb && s_breadcrumbSize) {
        char meta[512];
        const char* p = descriptor.path(); // "<spool>/<uuid>.dmp" (fixed buffer inside Breakpad)
        size_t n = 0;
        while (p[n] != '\0' && n < sizeof(meta) - 8) { meta[n] = p[n]; n++; }
        memcpy(meta + n, ".s2meta", 8); // 7 chars + NUL
        int fd = open(meta, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd >= 0) {
            uint32_t off = 0;
            while (off < s_breadcrumbSize) {
                ssize_t w = write(fd, s_breadcrumb + off, s_breadcrumbSize - off);
                if (w <= 0) break;
                off += (uint32_t)w;
            }
            close(fd);
        }
    }
    // false => Breakpad restores + re-raises to any previously installed handler: never swallow
    // the crash, never suppress core dumps (spec §6.2 chaining requirement).
    return false;
}

bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize) {
    if (s_handler || !spoolDir || !spoolDir[0]) return false;
    s_breadcrumb = breadcrumb;
    s_breadcrumbSize = breadcrumbSize;
    google_breakpad::MinidumpDescriptor descriptor(spoolDir);
    // install_handler=true: Breakpad installs SIGSEGV/SIGABRT/SIGBUS/SIGFPE/SIGILL/SIGTRAP
    // handlers on its own dedicated sigaltstack (stack-overflow faults stay catchable) and
    // SAVES the previous handlers for restore-and-re-raise. server_fd=-1: in-process dumping
    // (the Accelerator model; out-of-process is a documented future hardening).
    s_handler = new google_breakpad::ExceptionHandler(
        descriptor, /*filter=*/nullptr, DumpCallback, /*context=*/nullptr,
        /*install_handler=*/true, /*server_fd=*/-1);
    return true;
}

void S2CrashDisarm(void) {
    delete s_handler; // ~ExceptionHandler restores the previous signal handlers
    s_handler = nullptr;
    s_breadcrumb = nullptr;
    s_breadcrumbSize = 0;
}

// TEMPORARY WINDOWS NO-OP.
//
// The next crash-reporter task replaces this with the Breakpad Windows client. Returning false is
// intentional and observable: Load logs that the handler is NOT armed. Never silently claim crash
// capture while no exception handler exists.
#include "crash_handler.h"

bool S2CrashArm(const char*, const uint8_t*, uint32_t) {
    return false;
}

void S2CrashDisarm(void) {}

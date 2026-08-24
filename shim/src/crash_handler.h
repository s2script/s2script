#ifndef S2_CRASH_HANDLER_H
#define S2_CRASH_HANDLER_H
#include <stdint.h>
// Arm the platform's vendored Breakpad handler. Native faults write a minidump plus byte-exact
// <dump>.s2meta breadcrumb sidecar. Idempotent; false always means NOT armed.
bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize);
void S2CrashDisarm(void);
#endif

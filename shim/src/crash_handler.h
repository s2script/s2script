#ifndef S2_CRASH_HANDLER_H
#define S2_CRASH_HANDLER_H
#include <stdint.h>
// Arm the native crash handler: Linux Breakpad writes minidumps + <dump>.s2meta breadcrumb bytes.
// The temporary Windows implementation is an explicit no-op and always returns false until its
// dedicated crash-reporter task lands. Idempotent; false always means NOT armed.
bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize);
void S2CrashDisarm(void);
#endif

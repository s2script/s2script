#ifndef S2_CRASH_HANDLER_H
#define S2_CRASH_HANDLER_H
#include <stdint.h>
// Arm the Breakpad ExceptionHandler: minidumps into spoolDir; on fault also write the raw
// breadcrumb bytes as <dump>.s2meta. Idempotent (second call is a no-op returning false).
// Returns false on empty dir (fail-off).
bool S2CrashArm(const char* spoolDir, const uint8_t* breadcrumb, uint32_t breadcrumbSize);
void S2CrashDisarm(void);
#endif

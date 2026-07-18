// Standalone crash-handler selftest: fork a child that arms the handler and SIGSEGVs; assert
// (1) the child died by SIGSEGV (chaining preserved — the crash was NOT swallowed),
// (2) exactly one .dmp and one .dmp.s2meta appeared in the spool dir,
// (3) the .s2meta content is byte-identical to the breadcrumb buffer.
#include "crash_handler.h"
#include <dirent.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static uint8_t g_breadcrumb[128];

int main(int argc, char** argv) {
    const char* dir = argc > 1 ? argv[1] : "/tmp/s2-crash-selftest";
    mkdir(dir, 0755);
    // Clean previous artifacts.
    if (DIR* d = opendir(dir)) {
        while (dirent* e = readdir(d)) {
            if (e->d_name[0] == '.') continue;
            char p[1024];
            snprintf(p, sizeof p, "%s/%s", dir, e->d_name);
            unlink(p);
        }
        closedir(d);
    }
    for (size_t i = 0; i < sizeof g_breadcrumb; i++) g_breadcrumb[i] = (uint8_t)(i * 7 + 1);

    pid_t pid = fork();
    if (pid == 0) {
        if (!S2CrashArm(dir, g_breadcrumb, sizeof g_breadcrumb)) _exit(3);
        volatile int* p = nullptr;
        *p = 42; // SIGSEGV
        _exit(0); // unreachable
    }
    int status = 0;
    waitpid(pid, &status, 0);
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGSEGV) {
        fprintf(stderr, "FAIL: child did not die by SIGSEGV (chaining broken?) status=%d\n", status);
        return 1;
    }
    int dmp = 0, meta = 0;
    char metaPath[1024] = {0};
    if (DIR* d = opendir(dir)) {
        while (dirent* e = readdir(d)) {
            size_t n = strlen(e->d_name);
            if (n > 4 && strcmp(e->d_name + n - 4, ".dmp") == 0) dmp++;
            if (n > 11 && strcmp(e->d_name + n - 11, ".dmp.s2meta") == 0) {
                meta++;
                snprintf(metaPath, sizeof metaPath, "%s/%s", dir, e->d_name);
            }
        }
        closedir(d);
    }
    if (dmp != 1 || meta != 1) {
        fprintf(stderr, "FAIL: expected 1 .dmp + 1 .s2meta, got dmp=%d meta=%d\n", dmp, meta);
        return 1;
    }
    FILE* f = fopen(metaPath, "rb");
    uint8_t back[sizeof g_breadcrumb];
    size_t rd = f ? fread(back, 1, sizeof back, f) : 0;
    if (f) fclose(f);
    if (rd != sizeof g_breadcrumb || memcmp(back, g_breadcrumb, sizeof g_breadcrumb) != 0) {
        fprintf(stderr, "FAIL: .s2meta content mismatch (read %zu bytes)\n", rd);
        return 1;
    }
    printf("OK: SIGSEGV chained, minidump + byte-exact .s2meta written to %s\n", dir);
    return 0;
}

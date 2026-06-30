#ifndef S2SCRIPT_CORE_H
#define S2SCRIPT_CORE_H
#ifdef __cplusplus
extern "C" {
#endif

typedef void (*s2_log_fn)(int level, const char* utf8_msg);
typedef void (*s2_hook_request_fn)(const char* descriptor, int enable); /* core -> shim: install(1)/remove(0) */

int  s2script_core_init(s2_log_fn logger, s2_hook_request_fn request_hook);
int  s2script_core_eval(const char* utf8_js);
int  s2script_core_dispatch_game_frame(int phase, int simulating, int first, int last); /* phase 0=Pre,1=Post; returns collapsed HookResult */
void s2script_core_shutdown(void);

#ifdef __cplusplus
}
#endif
#endif /* S2SCRIPT_CORE_H */

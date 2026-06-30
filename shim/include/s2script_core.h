#ifndef S2SCRIPT_CORE_H
#define S2SCRIPT_CORE_H
#ifdef __cplusplus
extern "C" {
#endif

/* level: 0=info (reserved for warn/error in later slices) */
typedef void (*s2_log_fn)(int level, const char* utf8_msg);

/* Returns 0 on success, negative on error. */
int  s2script_core_init(s2_log_fn logger);
int  s2script_core_eval(const char* utf8_js);
void s2script_core_shutdown(void);

#ifdef __cplusplus
}
#endif
#endif /* S2SCRIPT_CORE_H */

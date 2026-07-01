#ifndef S2SCRIPT_CORE_H
#define S2SCRIPT_CORE_H
#ifdef __cplusplus
extern "C" {
#endif

typedef void (*s2_log_fn)(int level, const char* utf8_msg);
typedef void (*s2_hook_request_fn)(const char* descriptor, int enable); /* core -> shim: install(1)/remove(0) */

/* Engine-operation function pointers the shim implements and the core calls.
 * Every Slice-3 engine touchpoint is a C++ call (SchemaSystem virtuals, entity
 * system, ...) that lives shim-side; the core only ever sees these opaque C-ABI
 * pointers, never a raw C++ vtable.  All fields may be null -> the matching native
 * degrades to a safe miss.  Task 3 wires schema_offset; Tasks 4-5 fill the rest. */
typedef int   (*s2_schema_offset_fn)(const char* cls, const char* field);
typedef void* (*s2_ent_by_index_fn)(int idx);
typedef void* (*s2_deref_handle_fn)(unsigned int handle);
typedef void  (*s2_ent_state_changed_fn)(void* ent, int offset);
typedef void  (*s2_concommand_register_fn)(const char* name);

typedef struct {
    s2_schema_offset_fn       schema_offset;
    s2_ent_by_index_fn        ent_by_index;
    s2_deref_handle_fn        deref_handle;
    s2_ent_state_changed_fn   ent_state_changed;
    s2_concommand_register_fn concommand_register;
} S2EngineOps;

/* ops may be null -> all engine natives degrade.  The core copies the struct by
 * value at init; the caller's storage need not outlive the call. */
int  s2script_core_init(s2_log_fn logger, s2_hook_request_fn request_hook, const S2EngineOps* ops);
int  s2script_core_eval(const char* utf8_js);
int  s2script_core_dispatch_game_frame(int phase, int simulating, int first, int last); /* phase 0=Pre,1=Post; returns collapsed HookResult */
void s2script_core_shutdown(void);
/* Shim -> core: called by the ConCommand trampoline when a registered command fires.
 * name = Arg(0) (command name), slot = CPlayerSlot::Get() (-1 for server console),
 * args = ArgS() (everything after the command name). */
void s2script_core_dispatch_concommand(const char* name, int slot, const char* args);

#ifdef __cplusplus
}
#endif
#endif /* S2SCRIPT_CORE_H */

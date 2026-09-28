# S3 prerequisite and baseline receipt (2026-09-24)

Starting HEAD `57356a64353724093acce9ab1278a22938950d40` (`docs: assign compatibility artifact removal to loader cutover`), on `codex/game-package-runtime`; working tree was clean before this task. Root main baseline was `62678e782b5b69f60462381de7d61b9d961cc593`.

Exact integrated prerequisites:

- PR A stock KHook: `3525ce9e770e44208e939817716d261776d9ae33`.
- S1 checked binding resolution: `9b876f755200261def72464c53d02b08757a442f`.
- S2 function foundations: `08a369c94e16b9580794cbfdbdba8aa59d5ab463`.

Official, unmodified Metamod:Source pin: `fa6f80e4662e5b96cc2e97722d812f374581dfd8`, release 1469. S2's intended Linux x86_64 scalar ABI is receiver plus ordered `void/u8/i32/u32/i64/u64/f32/f64/ptr` atoms; `void` is return-only, `u8` is limited to proven bool projection. Varargs, aggregates, and unusual ABIs are outside the agreed matrix. The merged S2 commit supplies foundations, not the completed `Engine.function` runtime, all codecs/adapters, or full activation. Those remain prerequisites for later S3 tasks.

Inherited main checks: JS job 35970012299 and native job 35970012283 passed, including 940 Rust passed/5 ignored and release/sniper verification. These are inherited baseline records, not tests of this change. This worktree's `bash scripts/test-khook-shutdown.sh` passed after initializing the pinned public Metamod source; `npm test -w @s2script/sdk` passed 778/778 after installing worktree dependencies. Logs: `.superpowers/sdd/2026-09-22-game-package-boundary/evidence/s3-khook-shutdown.log` and `s3-sdk-affected.log`.

Baseline CS2 callback observations are limited to inherited fixture/native checks. Real-client and peer callbacks, map/reload behavior, and integrated S2 live acceptance remain pending. The previously observed whole-process shutdown-only SIGSEGV/139 remains a diagnostic, non-blocking observation; this task did not run a server quit test.

## Hardcoded selection sites (exact pre-change inventory)

```text
scripts/test-boundary-nameleak.sh:30:echo 'const _: &str = include_str!("../../games/cs2/js/pawn.js");' > "$tmp2"
shim/src/s2script_mm.cpp:327:// S2EngineOps table; `cls`/`field` are opaque strings the JS @s2script/cs2 layer
shim/src/s2script_mm.cpp:2791:// Cs2JsPath: resolve pawn.js relative to the plugin .so via dladdr (mirrors
shim/src/s2script_mm.cpp:2797://   + /js/pawn.js
shim/src/s2script_mm.cpp:2799:static std::string Cs2JsPath() {
shim/src/s2script_mm.cpp:2801:    if (dladdr(reinterpret_cast<void*>(&Cs2JsPath), &info) && info.dli_fname) {
shim/src/s2script_mm.cpp:2809:        return dir + "/js/pawn.js";
shim/src/s2script_mm.cpp:2812:    return "addons/s2script/js/pawn.js";
shim/src/s2script_mm.cpp:2817:// (mirrors Cs2JsPath / GamedataRoot).  Expected layout:
shim/src/s2script_mm.cpp:5047:            std::string js = slurp(Cs2JsPath());   // the deployed pawn.js concat carries the
shim/src/s2script_mm.cpp:5070:    // Register the @s2script/cs2 package (pawn.js) with the core so each plugin context
shim/src/s2script_mm.cpp:5072:    // Degrade-never-crash: a missing or unreadable pawn.js logs a WARN and continues;
shim/src/s2script_mm.cpp:5073:    // require("@s2script/cs2") will return null in plugin contexts until it is registered.
shim/src/s2script_mm.cpp:5075:        std::string cs2JsPath = Cs2JsPath();
shim/src/s2script_mm.cpp:5086:                    s2script_core_register_package("@s2script/cs2", js.c_str());
shim/src/s2script_mm.cpp:5087:                    META_CONPRINTF("[s2script] @s2script/cs2 registered (%ld bytes from %s)\n",
shim/src/s2script_mm.cpp:5091:                                   " — @s2script/cs2 not registered\n",
shim/src/s2script_mm.cpp:5096:                META_CONPRINTF("[s2script] WARN: %s is empty — @s2script/cs2 not registered\n",
shim/src/s2script_mm.cpp:5100:            META_CONPRINTF("[s2script] WARN: could not open %s — @s2script/cs2 not registered\n",
shim/src/s2script_mm.cpp:5113:    // vanishing silently. Independent of the pawn.js block: JS missing must not take the
shim/src/s2script_mm.cpp:5117:        s2script_core_register_package_gamedata("@s2script/cs2", gdJson.c_str());
shim/src/s2script_mm.cpp:5122:        META_CONPRINTF("[s2script] @s2script/cs2 gamedata registered (%zu declared call(s), "
core/src/loader.rs:1998:        let spoofed = crate::gamedata_calls::reserved_owner_id("@s2script/cs2");
core/src/v8host.rs:255:    /// Runtime package registry: maps package name (e.g. `"@s2script/cs2"`) to JS source.
core/src/v8host.rs:256:    /// Populated by the shim at load time via `s2script_core_register_package` (C-ABI, see ffi.rs).
core/src/v8host.rs:622:/// Register a game-package JS source string under `name` (e.g. `"@s2script/cs2"`).
core/src/v8host.rs:624:/// Called by the shim at load time (via the C-ABI `s2script_core_register_package`) to provide
core/src/v8host.rs:752:// funnels call. Same ordering contract as games/cs2/js (activity.js before pawn.js).
core/src/v8host.rs:756:// @s2script/cs2 is NOT embedded here. It is provided externally at runtime by the shim via
core/src/v8host.rs:757:// `register_injected_package("@s2script/cs2", <js>)` (see `ffi.rs`).  Core contains zero cs2 JS.
core/src/v8host.rs:758:// If the package is not registered, `require("@s2script/cs2")` returns null (graceful degrade).
core/src/v8host.rs:1174:        // `undefined` — which would slip past pawn.js's `HEALTH < 0` guard and be used as an offset.
core/src/v8host.rs:1413:/// module list hardcoded; `@s2script/cs2` maps to `__s2pkg_cs2` via the plain `@s2script/` strip.
core/src/v8host.rs:1428:        // hardcoded; @s2script/cs2 → __s2pkg_cs2 subsumed). Non-@s2script specifiers → null (the JS
core/src/v8host.rs:1437:        // module list hardcoded; `@s2script/cs2` keeps riding the plain `@s2script/` strip.
core/src/v8host.rs:3552:/// (`pawn.js`) runs in the raw context scope of EVERY plugin context, so these natives are reachable
core/src/v8host.rs:4076:/// A game package's hooks are registered under the RESERVED owner id (`game-package:@s2script/cs2`),
scripts/check-changeset-ignore.sh:12:# main, which auto-publishes @s2script/sdk and @s2script/cs2) blow up with "The package or glob
scripts/check-changeset-ignore.sh:19:# @s2script/cs2, which must keep being released via changesets. So the list stays an explicit
scripts/check-core-names.sh:17:# This closes the gap the Slice-4 regression exploited (core include_str!-ing games/cs2/js/pawn.js).
core/src/ffi.rs:768:/// is provided to core via `s2script_core_register_package` instead (see below).
core/src/ffi.rs:785:/// The shim calls this at load time with ("@s2script/cs2", <packaged pawn.js>), so each plugin
core/src/ffi.rs:786:/// context receives the @s2script/cs2 package via the runtime registry.
core/src/ffi.rs:788:pub extern "C" fn s2script_core_register_package(name: *const c_char, js: *const c_char) {
core/src/ffi.rs:807:/// `s2script_core_register_package`, and called with the same `name`.
core/src/ffi.rs:825:pub extern "C" fn s2script_core_register_package_gamedata(
core/src/gamedata_calls.rs:45:/// The reserved owner id for a game package (`@s2script/cs2` → `game-package:@s2script/cs2`).
core/src/gamedata_calls.rs:520:    /// `s2script_core_register_package_gamedata`. `None` on a host with no game package (every unit
core/src/gamedata_calls.rs:551:/// `s2script_core_register_package_gamedata`, with `package` the same injected-package name the
core/src/gamedata_calls.rs:552:/// shim passes to `s2script_core_register_package` (e.g. `@s2script/cs2`) — core never names a game.
core/src/gamedata_calls.rs:913:        assert!(!is_reserved_owner("@s2script/cs2"));
core/src/gamedata_calls.rs:916:        assert!(is_reserved_owner(&reserved_owner_id("@s2script/cs2")));
scripts/check-core-js-lint.sh:19:# games/cs2/js/ TOO. pawn.js runs in the RAW context scope, not the plugin CJS wrapper, so it reaches
scripts/check-core-js-lint.sh:21:# it just as silently. This gate was cited during review as covering pawn.js when it did not; the
scripts/package-addon.sh:45:# --- CS2 JS package (schema.generated.js + nav.generated.js + pawn.js — CS2 names live here, never in core) ---
scripts/package-addon.sh:47:if [ -f games/cs2/js/pawn.js ]; then
scripts/package-addon.sh:49:    # nav.generated.js MUST precede activity.js (which precedes pawn.js, the final IIFE).
scripts/package-addon.sh:50:    # activity.js sets globalThis.__s2_activity before pawn.js reads it.
scripts/package-addon.sh:51:    # csitem.generated.js sets globalThis.__s2pkg_cs2.CsItem; pawn.js's IIFE MERGES into
scripts/package-addon.sh:53:    # weapon.js MUST run after schema.generated.js (needs __s2pkg_cs2_schema) and before pawn.js
scripts/package-addon.sh:56:    cat games/cs2/js/schema.generated.js games/cs2/js/nav.generated.js games/cs2/js/activity.js games/cs2/js/csitem.generated.js games/cs2/js/weapon.js games/cs2/js/pawn.js games/cs2/js/camera.js games/cs2/js/ui.js games/cs2/js/components.js games/cs2/js/hudinput.js games/cs2/js/menuhud.js games/cs2/js/voterail.js > "$DIST/s2script/js/pawn.js"
core/src/v8host/lifecycle.rs:127:            // @s2script/cs2: provided externally at runtime via register_injected_package
core/src/v8host/lifecycle.rs:128:            // (the shim calls s2script_core_register_package at load — see ffi.rs).
core/src/v8host/lifecycle.rs:129:            // If not registered, __s2pkg_cs2 stays undefined and require("@s2script/cs2") → null.
core/src/v8host/lifecycle.rs:130:            let cs2_src = INJECTED_PACKAGES.with(|p| p.borrow().get("@s2script/cs2").cloned());
core/src/v8host/lifecycle.rs:132:                run_prelude(scope, "@s2script/cs2", &src);
core/js/prelude.js:1526:  //     game layer (games/cs2/js/pawn.js `Sounds`), never here.
core/src/v8host/tests.rs:2014:    /// A5b: the GAME PACKAGE's descriptor path, end to end through the natives pawn.js uses.
core/src/v8host/tests.rs:4829:            "@s2script/cs2",
core/src/v8host/tests.rs:4840:            const cs2 = require("@s2script/cs2");
core/src/v8host/tests.rs:4858:            "@s2script/cs2",
core/src/v8host/tests.rs:4883:        register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
core/src/v8host/tests.rs:4895:            "@s2script/cs2",
core/src/v8host/function_adapter.rs:3054:                        "@s2script/cs2",
core/src/v8host/function_adapter.rs:3170:        register_injected_package("@s2script/cs2", "");
shim/include/s2script_core.h:238: * s2script_core_register_package instead).  Safe to call; does nothing. */
shim/include/s2script_core.h:243:void s2script_core_register_package(const char* name, const char* js);
shim/include/s2script_core.h:246: * `name` is the SAME string passed to s2script_core_register_package. Core registers the `calls`
shim/include/s2script_core.h:251:void s2script_core_register_package_gamedata(const char* name, const char* gamedata_json);
```

## Capability-specific branches (exact pre-change inventory)

```text
core/src/admin.rs:272:        assert_eq!(eval_in_context_string("p", "String(__s2_damage_read_float(68))"), "0");
core/src/admin.rs:273:        assert_eq!(eval_in_context_string("p", "String(__s2_damage_read_int(60))"), "0");
core/src/admin.rs:275:        assert_eq!(eval_in_context_string("p", "String(__s2_damage_write_float(68, 5))"), "undefined");
core/src/engine_functions/mod.rs:71:            ("/functions/0/policy/id", json!("legacy.acquire.v1")),
shim/src/s2script_mm.cpp:832:    s2script_core_dispatch_damage();
shim/src/s2script_mm.cpp:834:static void S2NamedDamagePostOp() { s2script_core_dispatch_damage_post(); }
shim/src/s2script_mm.cpp:2535:static float s2_damage_read_float(int offset) {
shim/src/s2script_mm.cpp:2540:static int s2_damage_read_int(int offset) {
shim/src/s2script_mm.cpp:2545:static void s2_damage_write_float(int offset, float value) {
core/src/v8host.rs:2676:/// `__s2_damage_read_float(offset) -> f32` — read a float from the current CTakeDamageInfo. 0 if no op.
core/src/v8host.rs:2677:fn s2_damage_read_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
core/src/v8host.rs:2684:        let Some(func) = ops.damage_read_float else { return };
core/src/v8host.rs:2689:/// `__s2_damage_read_int(offset) -> i32` — read an int (e.g. a handle or m_bitsDamageType). 0 if no op.
core/src/v8host.rs:2690:fn s2_damage_read_int(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
core/src/v8host.rs:2697:        let Some(func) = ops.damage_read_int else { return };
core/src/v8host.rs:2702:/// `__s2_damage_write_float(offset, value)` — write m_flDamage etc. during a pre-hook (modify/block).
core/src/v8host.rs:2704:fn s2_damage_write_float(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
core/src/v8host.rs:2714:        let Some(func) = ops.damage_write_float else { return };
core/src/v8host.rs:3919:/// `damage_write_float` engine op with `0.0`. (CTakeDamageInfo is a Source 2 engine type, not a
core/src/v8host.rs:3932:    if let Some(func) = ENGINE_OPS.with(|o| o.get()).and_then(|o| o.damage_write_float) {
core/src/v8host.rs:3937:pub(crate) fn dispatch_damage() {
core/src/v8host.rs:3940:        dispatch_damage_kind(
core/src/v8host.rs:3942:            "dispatch_damage",
core/src/v8host.rs:3952:pub(crate) fn dispatch_damage_post() {
core/src/v8host.rs:3954:        dispatch_damage_kind(
core/src/v8host.rs:3956:            "dispatch_damage_post",
core/src/v8host.rs:3964:fn dispatch_damage_kind(
core/src/v8host.rs:3997:/// possibly-modified fields back after). Mirrors `dispatch_damage`'s snapshot + `try_borrow_mut`
core/src/v8host.rs:4002:/// (mirrors `dispatch_output`/`dispatch_game_event_pre`, NOT `dispatch_damage` which is void) — the
core/src/v8host.rs:4534:    let is_acquire = plan.shape == 3; // this_i64_i32_i64 — see gamedata_hooks::SHAPES
core/src/v8host.rs:4535:    let prev_acq = if is_acquire {
core/src/v8host.rs:4546:    let prev_after = if is_acquire {
core/src/v8host.rs:4556:    if is_acquire {
core/src/v8host.rs:4729:/// call when the result is >= Handled. Mirrors `dispatch_game_event_pre` / `dispatch_damage` (the
shim/src/s2script_engine_ops_fill.generated.inc:45:    ops.damage_read_float = &s2_damage_read_float;
shim/src/s2script_engine_ops_fill.generated.inc:46:    ops.damage_read_int = &s2_damage_read_int;
shim/src/s2script_engine_ops_fill.generated.inc:47:    ops.damage_write_float = &s2_damage_write_float;
core/src/ffi.rs:366:pub extern "C" fn s2script_core_dispatch_damage() {
core/src/ffi.rs:367:    let _ = catch_unwind(|| v8host::dispatch_damage());
core/src/ffi.rs:372:pub extern "C" fn s2script_core_dispatch_damage_post() {
core/src/ffi.rs:373:    let _ = catch_unwind(|| v8host::dispatch_damage_post());
core/src/dispatch.rs:159:/// The distinction matters: `dispatch_damage` truncated at `Handled` too, but as a BUG (one
core/engine-ops.jsonc:539:      "name": "damage_read_float",
core/engine-ops.jsonc:545:      "shim": "s2_damage_read_float"
core/engine-ops.jsonc:548:      "name": "damage_read_int",
core/engine-ops.jsonc:554:      "shim": "s2_damage_read_int"
core/engine-ops.jsonc:557:      "name": "damage_write_float",
core/engine-ops.jsonc:563:      "shim": "s2_damage_write_float"
core/src/sdkhooks.rs:900:        create_plugin_context, dispatch_damage, dispatch_damage_post, eval_in_context, init, load_plugin_js, plugin_phase,
core/src/sdkhooks.rs:920:    extern "C" fn rec_damage_write_float(offset: c_int, value: f32) {
core/src/sdkhooks.rs:1008:        dispatch_damage();
core/src/sdkhooks.rs:1059:        dispatch_damage();
core/src/sdkhooks.rs:1066:        dispatch_damage();
core/src/sdkhooks.rs:1083:        dispatch_damage();
core/src/sdkhooks.rs:1094:            damage_write_float: Some(rec_damage_write_float),
core/src/sdkhooks.rs:1098:        eval_in_context_string("p", &hook_js(5, id, "__s2_damage_write_float(777, 9);"));
core/src/sdkhooks.rs:1101:        dispatch_damage();
core/src/sdkhooks.rs:1119:        dispatch_damage();
core/src/sdkhooks.rs:1133:        dispatch_damage();
core/src/sdkhooks.rs:1148:        dispatch_damage();
core/src/sdkhooks.rs:1161:            damage_write_float: Some(rec_damage_write_float),
core/src/sdkhooks.rs:1167:        dispatch_damage();
core/src/sdkhooks.rs:1190:        dispatch_damage();
core/src/sdkhooks.rs:1204:        dispatch_damage();
core/src/sdkhooks.rs:1225:        dispatch_damage();
core/src/sdkhooks.rs:1250:        dispatch_damage();
core/src/sdkhooks.rs:1871:        dispatch_damage();
core/src/sdkhooks.rs:1873:        dispatch_damage_post();
core/src/sdkhooks.rs:1885:            damage_write_float: Some(rec_damage_write_float),
core/src/sdkhooks.rs:1899:        dispatch_damage_post();
core/src/sdkhooks.rs:1911:            damage_write_float: Some(rec_damage_write_float),
core/src/sdkhooks.rs:1927:        dispatch_damage();
core/src/sdkhooks.rs:1930:        dispatch_damage_post();
core/src/sdkhooks.rs:1957:        dispatch_damage_post();
core/js/prelude.js:824:    var h = __s2_damage_read_int(o) >>> 0;
core/js/prelude.js:832:      get: function () { var o = __s2_schema_offset("CTakeDamageInfo", "m_flDamage"); return o < 0 ? 0 : __s2_damage_read_float(o); },
core/js/prelude.js:833:      set: function (v) { var o = __s2_schema_offset("CTakeDamageInfo", "m_flDamage"); if (o >= 0) __s2_damage_write_float(o, +v); },
core/js/prelude.js:837:      get: function () { var o = __s2_schema_offset("CTakeDamageInfo", "m_bitsDamageType"); return o < 0 ? 0 : __s2_damage_read_int(o); },
core/src/engine_ops.generated.rs:243:    pub damage_read_float: Option<DamageReadFloatFn>,
core/src/engine_ops.generated.rs:244:    pub damage_read_int: Option<DamageReadIntFn>,
core/src/engine_ops.generated.rs:245:    pub damage_write_float: Option<DamageWriteFloatFn>,
core/src/v8host/natives.rs:144:        "__s2_damage_read_float",
core/src/v8host/natives.rs:145:        s2_damage_read_float,
core/src/v8host/natives.rs:150:        "__s2_damage_read_int",
core/src/v8host/natives.rs:151:        s2_damage_read_int,
core/src/v8host/natives.rs:156:        "__s2_damage_write_float",
core/src/v8host/natives.rs:157:        s2_damage_write_float,
core/src/v8host/tests.rs:1858:        dispatch_damage();
shim/include/s2script_core.h:161:void s2script_core_dispatch_damage(void);
shim/include/s2script_core.h:163:void s2script_core_dispatch_damage_post(void);
shim/include/s2script_engine_ops.generated.h:78:typedef float (*s2_damage_read_float_fn)(int offset);
shim/include/s2script_engine_ops.generated.h:79:typedef int (*s2_damage_read_int_fn)(int offset);
shim/include/s2script_engine_ops.generated.h:80:typedef void (*s2_damage_write_float_fn)(int offset, float value);
shim/include/s2script_engine_ops.generated.h:226:    s2_damage_read_float_fn damage_read_float;
shim/include/s2script_engine_ops.generated.h:227:    s2_damage_read_int_fn damage_read_int;
shim/include/s2script_engine_ops.generated.h:228:    s2_damage_write_float_fn damage_write_float;
```

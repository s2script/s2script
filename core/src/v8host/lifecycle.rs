use super::*;

/// `__s2_load_settled(hooks?)` — the plugin factory settled OK (design spec §5). Stores the returned
/// `PluginHooks` object (if any) on the `PluginInstance` (via `exports`) and marks the load `Settled`;
/// `finalize_loading_plugins` then arms + reconciles + moves it to `Active`. A second call for the same
/// id (state no longer `InFlight`) is ignored.
pub(super) fn s2_load_settled(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        if args.length() >= 1 {
            if let Ok(obj) = v8::Local::<v8::Object>::try_from(args.get(0)) {
                let g = v8::Global::new(scope.as_ref(), obj);
                PLUGINS.with(|p| {
                    if let Some(pi) = p.borrow_mut().get_mut(&id) {
                        pi.exports = Some(g);
                    }
                });
            }
        }
        LOADING.with(|l| {
            if let Some(e) = l.borrow_mut().get_mut(&id) {
                if matches!(e.state, SettleState::InFlight) {
                    e.state = SettleState::Settled;
                }
            }
        });
    }));
}

/// `__s2_load_failed(message)` — the plugin factory threw or its promise rejected (design spec §5).
/// Marks the load `Failed(message)`; `finalize_loading_plugins` then tears it down (never runs it).
pub(super) fn s2_load_failed(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        let msg = if args.length() >= 1 {
            args.get(0).to_rust_string_lossy(scope)
        } else {
            "factory failed".into()
        };
        LOADING.with(|l| {
            if let Some(e) = l.borrow_mut().get_mut(&id) {
                if matches!(e.state, SettleState::InFlight) {
                    e.state = SettleState::Failed(msg);
                }
            }
        });
    }));
}

/// `__s2_handoff_take() -> unknown` — consume this plugin's reload-handoff blob (if a prior unload
/// captured one via `state()`) and revive it in THIS (new) context via `iface_from_json` (JSON.parse +
/// the EntityRef reviver). Consume-once. Backs `ctx.previous` (the 5E.3 mechanics moved off
/// `onLoad(prev)`). No blob → `undefined`.
pub(super) fn s2_handoff_take(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(id) = current_plugin(scope) else { return };
        let Some(blob) = PENDING_HANDOFF.with(|h| h.borrow_mut().remove(&id)) else { return };
        if let Some(v) = iface_from_json(scope, &blob) {
            rv.set(v);
        }
    }));
}

/// Create a fresh per-plugin `v8::Context` on the shared isolate (borrowed from `HOST`): register
/// the plugin in `REGISTRY` (FIRST — preludes subscribe at eval time and must stamp the real
/// generation), stamp the context with the plugin id via `set_slot::<PluginId>`, install the FULL
/// per-context API (all natives + `__s2require`) and evaluate the injected engine-generic prelude
/// + any registered game preludes, store its `PluginInstance` in `PLUGINS`, and return the
/// generation.
///
/// Panics only if called before `init` (no isolate yet) — an internal invariant, not an FFI path.
pub(crate) fn create_plugin_context(id: &str) -> u64 {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow
            .as_mut()
            .expect("create_plugin_context called before init");

        // Register in REGISTRY BEFORE building the context: the preludes evaluated below subscribe
        // at eval time (the cs2 prelude's HUD click hook, ui.js's onMapStart/onActive/…), and
        // every subscribe native stamps (owner, generation) via `plugin_generation`. If the
        // registration happened after the preludes, those stamps would all be the never-live
        // sentinel 0 and dispatch would silently drop them (the P0-1 defect — they accidentally
        // fired for whichever plugin happened to hold generation 0). Dispatch cannot interleave
        // with this synchronous window, and `clone_plugin_context` degrades to None until the
        // PluginInstance lands in PLUGINS below.
        let generation = REGISTRY.with(|r| r.borrow_mut().insert(id));
        // A fresh load clears any prior FAILED reason for this id (spec §5).
        FAILED_PLUGINS.with(|f| { f.borrow_mut().remove(id); });

        // Build the context in a nested block so the HandleScope borrow on the shared isolate is
        // released before we touch PLUGINS.  Mirrors `init`'s scope construction.
        let g_ctx = {
            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Context::new(hs, Default::default());

            // Stamp the plugin identity (no scope needed — Rust-typed slot).
            let _ = ctx_local.set_slot(std::rc::Rc::new(PluginId(id.to_string())));
            let _ = ctx_local.set_slot(std::rc::Rc::new(InteropGeneration(generation)));

            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            // Full per-context API: install the natives first, THEN evaluate the injected preludes
            // (which build the five module globals + any registered game package globals over those
            // natives and stash them at `globalThis.__s2pkg_*` for `__s2require`).
            let global_obj = ctx_local.global(scope);
            install_natives(scope, global_obj);
            // Framework config templates (__s2_TEMPLATES) must exist before the engine prelude, whose
            // admin/db loaders read them to write the operator's file on first boot.
            run_prelude(scope, "config-templates", &config_templates_prelude());
            run_prelude(scope, "engine-prelude", INJECTED_STD_PRELUDE);
            // @s2script/cs2: provided externally at runtime via register_injected_package
            // (the shim calls s2script_core_register_package at load — see ffi.rs).
            // If not registered, __s2pkg_cs2 stays undefined and require("@s2script/cs2") → null.
            let cs2_src = INJECTED_PACKAGES.with(|p| p.borrow().get("@s2script/cs2").cloned());
            if let Some(src) = cs2_src {
                run_prelude(scope, "@s2script/cs2", &src);
            }

            v8::Global::new(scope.as_ref(), ctx_local)
            // scope, hs, hs_storage drop here — the isolate borrow is released.
        };

        let imports = IFACES.with(|r| r.borrow().import_names(id));
        for name in imports { record_resource(id, generation, plugin::Resource::Import(name)); }
        PLUGINS.with(|p| {
            p.borrow_mut().insert(
                id.to_string(),
                PluginInstance {
                    exports: None,
                    context: g_ctx,
                    config_decls: std::collections::HashMap::new(),
                    phase: crate::plugin::Phase::Loading,
                },
            )
        });
        generation
    })
}

/// The current lifecycle phase of `id`, or `None` if unknown (never loaded / disposed). Used by
/// tests and `unload_plugin`'s phase-aware entry.
pub(crate) fn plugin_phase(id: &str) -> Option<crate::plugin::Phase> {
    PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.phase))
}

/// Dispose a plugin's context: drop its `Global<Context>` (making the context GC-eligible while
/// the isolate is still alive) and remove it from both `PLUGINS` and `REGISTRY`.
///
/// NOTE: the `Global`s pointing INTO this context (handlers/resolvers/exports) must be dropped
/// BEFORE its `Global<Context>` — that ordered teardown is Task 6's ledger job.  For THIS task
/// (minimal per-context install, no such inner Globals yet) dropping the `Global<Context>` is
/// sufficient.
pub(crate) fn dispose_plugin_context(id: &str) {
    // Dropping the Global<Context> here (map removal) is safe: the isolate lives in HOST.
    PLUGINS.with(|p| {
        p.borrow_mut().remove(id);
    });
    REGISTRY.with(|r| {
        r.borrow_mut().remove(id);
    });
}

/// Enter the `id`'s plugin context and evaluate `src` in it (test/integration helper — the
/// per-plugin analogue of `eval`).  Uses the shared isolate from `HOST`; mirrors `eval`'s scope +
/// `TryCatch` construction.  Returns `Err` if `init` hasn't run, the id has no context, or the JS
/// fails to compile/run.
pub(crate) fn eval_in_context(id: &str, src: &str) -> Result<(), String> {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow
            .as_mut()
            .ok_or_else(|| "eval_in_context called before init".to_string())?;

        // Clone the plugin's Global<Context> out of PLUGINS (cheap refcount bump) so we don't hold
        // the PLUGINS borrow across the HandleScope on HOST.isolate.
        let g_ctx = PLUGINS
            .with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))
            .ok_or_else(|| format!("eval_in_context: no context for plugin '{}'", id))?;

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

        let code = v8::String::new(tc, src)
            .ok_or_else(|| "failed to intern source string in V8".to_string())?;

        let script = match v8::Script::compile(tc, code, None) {
            Some(s) => s,
            None => {
                return Err(tc
                    .exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "unknown JavaScript error (compile)".into()));
            }
        };

        match script.run(tc) {
            Some(_) => Ok(()),
            None => Err(tc
                .exception()
                .map(|e| e.to_rust_string_lossy(&*tc))
                .unwrap_or_else(|| "unknown JavaScript error (run)".into())),
        }
    })
}

/// Materialize a plugin's config (defaults ⊕ the override file read via the `config_read` op;
/// auto-generate the file via `config_write` if absent) and return the values JSON to inject.
/// Degrade: no ops → defaults only, no auto-write, still returns the defaults JSON.
#[allow(dead_code)] // synchronous compatibility adapter; periodic loader calls the snapshot form
pub(crate) fn materialize_for_load(id: &str, decls: &std::collections::HashMap<String, crate::config::ConfigEntry>) -> String {
    if decls.is_empty() { return "{}".to_string(); }
    let ops = ENGINE_OPS.with(|o| o.get());
    let cid = std::ffi::CString::new(id).ok();
    // read the override file
    let override_json: Option<String> = (|| {
        let ops = ops?; let f = ops.config_read?; let cid = cid.as_ref()?;
        let ptr = f(cid.as_ptr()); if ptr.is_null() { return None; }
        Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
    })();
    materialize_for_load_snapshot(id, decls, override_json.as_deref())
}

/// Materialize a loader-supplied config snapshot. Filesystem I/O has already completed on the
/// dedicated loader worker; only the existing main-thread merge, diagnostics, and optional default
/// write remain here.
pub(crate) fn materialize_for_load_snapshot(
    id: &str,
    decls: &std::collections::HashMap<String, crate::config::ConfigEntry>,
    override_json: Option<&str>,
) -> String {
    if decls.is_empty() { return "{}".to_string(); }
    let ops = ENGINE_OPS.with(|o| o.get());
    let cid = std::ffi::CString::new(id).ok();
    let was_absent = override_json.is_none();
    let mat = crate::config::materialize_config(decls, override_json);
    for w in &mat.warnings { log_warn(&format!("config('{}'): {}", id, w)); }
    if was_absent {  // auto-generate the default file
        if let (Some(ops), Some(cid)) = (ops, cid.as_ref()) {
            if let Some(wf) = ops.config_write {
                if let Ok(content) = std::ffi::CString::new(crate::config::generate_default_jsonc(decls)) {
                    wf(cid.as_ptr(), content.as_ptr());
                }
            }
        }
    }
    serde_json::to_string(&serde_json::Value::Object(mat.values)).unwrap_or_else(|_| "{}".to_string())
}

/// Store config decls on a plugin's `PluginInstance` (called from the loader right after
/// `load_plugin_js` so `re_materialize_config` can re-run without needing the manifest).
pub(crate) fn store_config_decls(id: &str, decls: std::collections::HashMap<String, crate::config::ConfigEntry>) {
    PLUGINS.with(|p| {
        if let Some(pi) = p.borrow_mut().get_mut(id) {
            pi.config_decls = decls;
        }
    });
}

/// Read the current content of the plugin's config override file via the `config_read` op.
/// Returns `None` if no ops table is wired, the op is absent, or the file doesn't exist yet.
/// Periodic loader reads use owned worker snapshots; this remains the synchronous adapter for
/// explicit raw config APIs, crash reads, and writes that need the engine path policy.
pub(crate) fn config_file_content(id: &str) -> Option<String> {
    let ops = ENGINE_OPS.with(|o| o.get())?;
    let f = ops.config_read?;
    let cid = std::ffi::CString::new(id).ok()?;
    let ptr = f(cid.as_ptr());
    if ptr.is_null() { return None; }
    Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

/// Crash reporter: read an arbitrary configs/<id>.json via the config_read op (the same shim
/// path plugins' configs use). pub(crate) so crash::config can reach it without touching ops.
pub(crate) fn read_engine_config(id: &str) -> Option<String> {
    config_file_content(id)
}

/// Re-materialize a plugin's config after its override file changed: re-read the file, merge with
/// declared defaults, re-inject `globalThis.__s2pkg_config_values`, and fire every `onChange`
/// handler registered by that plugin (via CONFIG_SUBS) with the updated config object as the arg.
///
/// The periodic loader calls the snapshot form below when worker-supplied content changes. This
/// synchronous adapter remains for explicit callers and uses the same context discipline.
///
/// PRECONDITION: call only with `HOST` UNBORROWED (the loader poll runs on the post-`frame_async_drain`
/// path where HOST is free).  Step (2) re-injects via `eval_in_context` (which `borrow_mut`s HOST) and
/// the fire loop then `try_borrow_mut`s — so a caller that invoked this mid-borrow would PANIC at step
/// (2) rather than degrade.  Do not add a call-site that holds the HOST borrow.
pub(crate) fn re_materialize_config(id: &str) {
    let content = config_file_content(id);
    re_materialize_config_snapshot(id, content.as_deref());
}

/// Re-materialize from an owned worker snapshot. This is the periodic loader path and performs no
/// config read; explicit raw config APIs and crash reads continue to use `config_file_content`.
pub(crate) fn re_materialize_config_snapshot(id: &str, override_json: Option<&str>) {
    apply_config_snapshot(id, override_json, false);
}

/// The first asynchronous watch read must reconcile with this plugin's applied values.
/// Suppress only an unchanged initial state (including an auto-generated defaults file).
pub(crate) fn reconcile_initial_config_snapshot(id: &str, override_json: Option<&str>) {
    apply_config_snapshot(id, override_json, true);
}

pub(super) fn config_values_match(id: &str, values_json: &str) -> bool {
    HOST.with(|h| -> Option<bool> {
        let mut host = h.borrow_mut();
        let host = host.as_mut()?;
        let context = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.context.clone()))?;
        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let context = v8::Local::new(&mut hs, &context);
        let scope = &mut v8::ContextScope::new(&mut hs, context);
        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let key = v8::String::new(&mut tc, "__s2pkg_config_values")?;
        let value = context.global(&mut tc).get(&mut tc, key.into())?;
        let json = v8::json::stringify(&mut tc, value)?;
        let proposed = v8::String::new(&mut tc, values_json)?;
        let proposed = v8::json::parse(&mut tc, proposed)?;
        let proposed = v8::json::stringify(&mut tc, proposed)?;
        // Normalize numbers/escapes through the same JSON serializer. Equal values have
        // equal byte lengths regardless of key order. Reject a plugin-expanded object before
        // copying it into Rust: temporary native copies stay bounded by the proposed config.
        if json.utf8_length(&mut tc) != proposed.utf8_length(&mut tc) {
            return Some(false);
        }
        let current: serde_json::Value =
            serde_json::from_str(&json.to_rust_string_lossy(&tc)).ok()?;
        let proposed: serde_json::Value =
            serde_json::from_str(&proposed.to_rust_string_lossy(&tc)).ok()?;
        Some(current == proposed)
    })
    .unwrap_or(false)
}

pub(super) fn apply_config_snapshot(id: &str, override_json: Option<&str>, initial: bool) {
    // (1) Get this plugin's stored config decls (empty → nothing to re-materialize, but still fire).
    let decls = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.config_decls.clone()));
    let Some(decls) = decls else { return };

    // (2) Re-materialize (no ops → defaults only; file exists → override merged) → inject.
    let values_json = materialize_for_load_snapshot(id, &decls, override_json);
    if initial && config_values_match(id, &values_json) { return; }
    let _ = eval_in_context(id, &format!("globalThis.__s2pkg_config_values = {};", values_json));

    // (3) Snapshot CONFIG_SUBS for the "config" name, filtered to this plugin's handlers.
    //     Release the borrow before entering any context.
    let snap: Vec<(String, u64, v8::Global<v8::Function>)> = CONFIG_SUBS.with(|m| {
        m.borrow().snapshot("config")
            .into_iter()
            .filter(|(owner, _, _)| owner == id)
            .collect()
    });
    if snap.is_empty() { return; }

    // (4) Fire loop — mirrors dispatch_game_event (snapshot released; try_borrow_mut guard).
    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, gen, handler_g) in &snap {
            // Liveness check (borrow released before entering the context).
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *gen)) { continue; }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue };

            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            // Read globalThis.__s2pkg_config_values in this context as the handler arg.
            let config_arg: v8::Local<v8::Value> = (|| -> Option<v8::Local<v8::Value>> {
                let global = ctx_local.global(tc);
                let key = v8::String::new(tc, "__s2pkg_config_values")?;
                let val = global.get(tc, key.into())?;
                if val.is_undefined() { None } else { Some(val) }
            })().unwrap_or_else(|| v8::undefined(tc).into());

            let func = v8::Local::new(tc, handler_g);
            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();

            if func.call(tc, recv, &[config_arg]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: re_materialize_config('{}'): onChange '{}': {}", id, owner, msg));
            }
            // tc, tc_storage, scope drop here — TryCatch absorbs any pending exception.
        }
    });
}

/// Native `__s2_config_on_change(handler)` — register an onChange handler for this plugin's
/// config.  The loader detects file changes and calls `re_materialize_config(id)`, which fires
/// all registered handlers with the updated `__s2pkg_config_values` object.
/// Idempotent watch: calling this multiple times seeds the baseline only once per plugin.
pub(super) fn s2_config_on_change(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        // `owner` is needed AFTER subscribing, to seed the file watch, so it is resolved here and the
        // helper resolves it again. Only reached once a row was actually stored.
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        if subscribe_into(scope, &args, &CONFIG_SUBS, "config", 0).is_none() { return; }
        crate::loader::watch_config_for(&owner);  // idempotent; seeds baseline if not yet watched
    }));
}


/// The result of STARTING a plugin load (L1 lifecycle v2). The load's TRANSITION (arm → Active, or
/// teardown → Failed) happens later, in `finalize_loading_plugins` (the sync fast-path runs it inline
/// at the tail of `load_plugin_js`; an async factory settles on a later `frame_async_drain`).
enum LoadStart {
    /// The artifact was valid; the factory was driven and an in-flight `LOADING` entry registered.
    Started,
    /// The bundle is not a `plugin()` artifact and has no `OnPluginStart` (or is a legacy `onLoad`
    /// shape) — fail loud, tear down the fresh context (never run it).
    Refused(String),
    /// Host not initialized / context missing — nothing to do.
    Aborted,
}

/// Load a built plugin bundle `plugin_js` under plugin id `id` (the L1 lifecycle-v2 artifact path).
///
/// Steps: (1) `create_plugin_context(id)` — a fresh per-plugin context with the full injected API
/// (`__s2require` + the engine-generic prelude + any registered game preludes); (2) evaluate the CJS
/// wrapper `(function(require,module,exports){…})(require, module, module.exports)` in that context
/// and CAPTURE the RETURNED `module.exports`; (3) require either `module.exports.default` to be a
/// `plugin()` definition (`{ __s2plugin: 1, factory }`) or `module.exports.OnPluginStart` to be a
/// function (or both) — fail LOUD if neither (reason names `OnPluginStart`); a legacy `onLoad` export
/// is still refused; (4) register an in-flight `LOADING` entry and drive
/// `globalThis.__s2_run_factory(def, exports)` (factory first, then publics). The load-scoped `ctx` is
/// stashed at `globalThis.__s2_load_ctx`; settle (`__s2_load_settled`/`__s2_load_failed`) marks the `LOADING` state,
/// and `finalize_loading_plugins` performs the actual arm-at-Active / teardown-on-Failed transition —
/// inline here for a synchronous factory (the whole base suite), or on a later drain for an async one.
///
/// Degrade-never-crash: a compile/run error logs a named WARN and tears down; no exception propagates
/// (the whole JS run is under a `TryCatch`).
pub(crate) fn load_plugin_js(id: &str, plugin_js: &str, config_values_json: &str) {
    // Defensive guard: if the plugin is already loaded (e.g. the caller is performing a
    // reload but did not call unload_plugin first), tear it down now so the old handler
    // Global/context can never leak into the new instance.  The loader's explicit
    // unload-before-load (T7 reload discipline) makes this a belt-and-suspenders no-op
    // in the normal reload path; it protects against accidental double-loads in other paths.
    if PLUGINS.with(|p| p.borrow().contains_key(id)) {
        log_warn(&format!(
            "WARN: load_plugin_js('{}'): plugin already loaded — unloading old instance first (reload guard)",
            id
        ));
        unload_plugin(id);
    }

    // (1) Fresh context with the full injected API installed.
    create_plugin_context(id);

    // Inject the materialized config as a per-context global BEFORE the plugin evals (so config reads
    // in the factory see it). @s2script/config's getters read globalThis.__s2pkg_config_values.
    let _ = eval_in_context(id, &format!("globalThis.__s2pkg_config_values = {};", config_values_json));

    // The spike's PROVEN wrapper — the outer arrow-IIFE returns `module.exports` so `script.run`
    // hands it straight back to Rust.  `{PLUGIN_JS}` is spliced verbatim.
    let wrapper = format!(
        "(() => {{\n  const module = {{ exports: {{}} }};\n  const require = globalThis.__s2_require;\n  (function (require, module, exports) {{\n{}\n}})(require, module, module.exports);\n  return module.exports;\n}})()",
        plugin_js
    );

    let start = HOST.with(|h| -> LoadStart {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else {
            log_warn("WARN: load_plugin_js called before init");
            return LoadStart::Aborted;
        };

        // Clone the plugin's Global<Context> out of PLUGINS (cheap refcount bump); release the
        // borrow before opening the HandleScope on HOST.isolate.
        let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.context.clone())) else {
            log_warn(&format!("WARN: load_plugin_js('{}'): context missing after create", id));
            return LoadStart::Aborted;
        };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        // Compile+run the wrapper, detect the plugin() artifact, drive the factory — all under one
        // TryCatch so a throwing bundle can't leak a pending exception into later frames.
        let start: LoadStart = 'blk: {
            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let Some(code) = v8::String::new(tc, &wrapper) else {
                break 'blk LoadStart::Refused("failed to intern source".into());
            };
            let ret = match v8::Script::compile(tc, code, None).and_then(|s| s.run(tc)) {
                Some(r) => r,
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "unknown error".into());
                    log_warn(&format!("WARN: load_plugin_js('{}'): eval error: {}", id, msg));
                    crate::crash::report_js_error(id, "load", &msg, "");
                    break 'blk LoadStart::Refused(format!("eval error: {}", msg));
                }
            };
            // The wrapper returns `module.exports` — must be an object.
            let Ok(exports) = v8::Local::<v8::Object>::try_from(ret) else {
                break 'blk LoadStart::Refused("module.exports is not an object".into());
            };

            // (3) The artifact: plugin() default AND/OR export function OnPluginStart.
            let def_obj: Option<v8::Local<v8::Object>> = v8::String::new(tc, "default")
                .and_then(|k| exports.get(tc, k.into()))
                .and_then(|v| v8::Local::<v8::Object>::try_from(v).ok());
            let is_plugin = def_obj
                .map(|o| {
                    let tag_ok = v8::String::new(tc, "__s2plugin")
                        .and_then(|k| o.get(tc, k.into()))
                        .and_then(|t| t.int32_value(tc))
                        .map(|n| n == 1)
                        .unwrap_or(false);
                    let factory_ok = v8::String::new(tc, "factory")
                        .and_then(|k| o.get(tc, k.into()))
                        .map(|f| f.is_function())
                        .unwrap_or(false);
                    tag_ok && factory_ok
                })
                .unwrap_or(false);
            let has_on_plugin_start = v8::String::new(tc, "OnPluginStart")
                .and_then(|k| exports.get(tc, k.into()))
                .map(|v| v.is_function())
                .unwrap_or(false);

            if !is_plugin && !has_on_plugin_start {
                // Fail loud: a legacy onLoad shape, or neither plugin() nor OnPluginStart.
                let has_legacy = v8::String::new(tc, "onLoad")
                    .and_then(|k| exports.get(tc, k.into()))
                    .map(|v| v.is_function())
                    .unwrap_or(false);
                let reason = if has_legacy {
                    "legacy plugin shape (export onLoad) - rebuild with @s2script/sdk: export function OnPluginStart"
                } else {
                    "no export function OnPluginStart"
                };
                log_warn(&format!("WARN: load('{}'): {}", id, reason));
                crate::crash::report_js_error(id, "load", reason, "");
                break 'blk LoadStart::Refused(reason.to_string());
            }
            let def_val: v8::Local<v8::Value> = if is_plugin {
                def_obj.expect("is_plugin implies def_obj is Some").into()
            } else {
                v8::undefined(tc).into()
            };
            let exports_val: v8::Local<v8::Value> = exports.into();

            // Register the in-flight load BEFORE running the factory (a SYNC settle mutates this entry).
            LOADING.with(|l| {
                l.borrow_mut().insert(
                    id.to_string(),
                    LoadingEntry {
                        started_frame: FRAME_COUNTER.with(|c| c.get()),
                        state: SettleState::InFlight,
                        pending_reload: false,
                    },
                )
            });

            // Drive factory + publics: globalThis.__s2_run_factory(def, exports).
            // def may be undefined when the artifact is publics-only.
            let global = tc.get_current_context().global(tc);
            let run_ok = v8::String::new(tc, "__s2_run_factory")
                .and_then(|k| global.get(tc, k.into()))
                .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok())
                .map(|run_f| {
                    let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
                    run_f.call(tc, recv, &[def_val, exports_val]).is_some()
                })
                .unwrap_or(false);
            if !run_ok {
                let msg = tc
                    .exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "factory driver threw".into());
                LOADING.with(|l| {
                    if let Some(e) = l.borrow_mut().get_mut(id) {
                        e.state = SettleState::Failed(msg);
                    }
                });
            }
            LoadStart::Started
        };
        start
    });

    match start {
        // Sync fast-path: a synchronous factory is already Settled, so this arms + reconciles + goes
        // Active within this same call (preserving today's synchronous-load semantics). An async
        // factory stays InFlight and finalizes on a later frame_async_drain.
        LoadStart::Started => finalize_loading_plugins(),
        // Fail loud: record the reason + tear down the fresh (never-Active) context (HOST released).
        LoadStart::Refused(reason) => {
            FAILED_PLUGINS.with(|f| { f.borrow_mut().insert(id.to_string(), reason); });
            unload_partial(id);
        }
        LoadStart::Aborted => {}
    }
}


/// Unload a plugin at a frame boundary (never mid-dispatch): the ledger reverse-walk teardown
/// authority.  Order matches the spike's Global-drop-before-context discipline (all `Global`s
/// pointing INTO the plugin's context are dropped BEFORE its `Global<Context>`, isolate alive):
///
/// (a) `FRAME.remove_by_owner(id)` — drops the plugin's handler `Global<Function>`s + reconciles the
///     detour (removes the `OnGameFrame` detour if this was the only subscriber).
/// (b) best-effort `onUnload` (enter the plugin's context, call `module.exports.onUnload` if present
///     under a `TryCatch` — a throw is logged, teardown proceeds).
/// (c) `REGISTRY.remove(id)` → walk `ledger.teardown_order()` (reverse acquisition): `Timer` → remove
///     from `TIMERS` + drop its `RESOLVERS` entry; `Job` → drop its `RESOLVERS` entry (a late worker
///     completion is then a no-op; decrement `PENDING_JOBS` for a still-pending job we drop); `Hook`
///     → already removed by (a), dropped defensively.  Drops the resolver `Global`s.
/// (d) drop the captured `module.exports` `Global<Object>`.
/// (e) `dispose_plugin_context(id)` — NOW drop the `Global<Context>` (all inner Globals released in
///     a–d, isolate alive → sound, no leak).
/// Unload every loaded plugin in reverse-dependency order (importers before producers), so a
/// consumer's onUnload can still call the producer it depends on. Used by `shutdown` and any
/// full-teardown cascade. Computes the id list and order into owned Vecs (releasing all borrows)
/// before the unload loop so unload_plugin can freely re-enter IFACES/PLUGINS.
pub fn unload_all() {
    let ids = PLUGINS.with(|p| p.borrow().keys().cloned().collect::<Vec<_>>());
    let order = IFACES.with(|r| r.borrow().unload_order(&ids));
    for id in order { unload_plugin(&id); }
}

pub(crate) fn unload_plugin(id: &str) {
    crate::crash::breadcrumb::plugin_unloaded(id);

    // Phase-aware entry (design spec §5.4): a plugin still LOADING never reached Active — it has no
    // hooks to run and its subs are still buffered (nothing to sweep). Seal its ctx, drop the LOADING
    // entry, and walk the PARTIAL ledger (any DB conn / timer / import acquired before it was unloaded).
    if matches!(plugin_phase(id), Some(crate::plugin::Phase::Loading)) {
        let _ = eval_in_context(id, "globalThis.__s2_ctx_seal && globalThis.__s2_ctx_seal();");
        LOADING.with(|l| { l.borrow_mut().remove(id); });
        unload_partial(id);
        return;
    }

    // Active/Unloading: mark Unloading, then the full teardown.
    PLUGINS.with(|p| {
        if let Some(pi) = p.borrow_mut().get_mut(id) {
            pi.phase = crate::plugin::Phase::Unloading;
        }
    });

    // (a) Sweep every owner-scoped subscription store in registration order (design spec §6). This
    // replaces the hand-maintained cascade: each store's `remove_by_owner` closure (registered in
    // `register_builtin_stores`) drops the plugin's handler Globals / rules and runs its own follow-up
    // engine-op (event_unsubscribe, the PRE-hook GameEvent removal, usermsg_hook_unsub, transmit
    // re-push, config unwatch, ConCommand/flag-meta drop, TopMenu-item drop). The FRAME store also
    // reconciles the detour; the trailing refresh_detour here re-applies the combined predicate as the
    // source of truth (idempotent).
    crate::owner_stores::sweep_owner(id);
    refresh_detour();

    // (b) state() handoff + best-effort onUnload in the plugin's OWN context.
    capture_state_and_run_onunload(id);

    // (c)–(e) ledger reverse-walk + iface cleanup + exports/context drop.
    teardown_ledger_and_dispose(id);
}

/// Teardown for a never-Active plugin (a Failed load, or an unload while still Loading — design spec
/// §5.4). Sweeps the owner-scoped stores (a no-op for still-buffered subs), then walks the PARTIAL
/// ledger + disposes the context. Skips `state()`/`onUnload` — the plugin was never Active.
fn unload_partial(id: &str) {
    crate::owner_stores::sweep_owner(id);
    refresh_detour();
    teardown_ledger_and_dispose(id);
}

/// Capture the reload-handoff via the plugin's `state()` hook, then run its `onUnload()` — both off
/// the settled `PluginHooks` object (`pi.exports`), in the plugin's OWN context. Clone the context +
/// hooks out of PLUGINS (borrow released) so the hooks may re-enter PLUGINS/FRAME/etc. without a
/// double borrow. `state()` runs FIRST (serialize via `iface_to_json` → `PENDING_HANDOFF`, WARN on a
/// non-serializable return); `onUnload()`'s return is IGNORED (WARN once if it returns non-undefined —
/// use `state()` for the handoff).
fn capture_state_and_run_onunload(id: &str) {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let Some(host) = borrow.as_mut() else { return };
        let Some((g_ctx, Some(hooks))) =
            PLUGINS.with(|p| p.borrow().get(id).map(|pi| (pi.context.clone(), pi.exports.clone())))
        else {
            return; // no context or no settled hooks → nothing to call
        };

        let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
        let hs = &mut hs;
        let ctx_local = v8::Local::new(hs, &g_ctx);
        let scope = &mut v8::ContextScope::new(hs, ctx_local);

        let mut tc_storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
        let tc = &mut tc;

        let hooks_local = v8::Local::new(tc, &hooks);
        let recv: v8::Local<v8::Value> = v8::undefined(tc).into();

        // (1) state() FIRST — its serializable return becomes the reload-handoff blob.
        if let Some(f) = v8::String::new(tc, "state")
            .and_then(|k| hooks_local.get(tc, k.into()))
            .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok())
        {
            match f.call(tc, recv, &[]) {
                Some(ret) => {
                    if !ret.is_undefined() && !ret.is_null() {
                        match iface_to_json(tc, ret) {
                            Some(blob) => PENDING_HANDOFF.with(|h| {
                                h.borrow_mut().insert(id.to_string(), blob);
                            }),
                            None => log_warn(&format!(
                                "WARN: unload_plugin('{}'): state() return not serializable — no state handoff",
                                id
                            )),
                        }
                    }
                }
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "state() threw".into());
                    log_warn(&format!("WARN: unload_plugin('{}'): state() error: {}", id, msg));
                }
            }
        }

        // (2) onUnload() — return IGNORED (use state() for the handoff).
        if let Some(f) = v8::String::new(tc, "onUnload")
            .and_then(|k| hooks_local.get(tc, k.into()))
            .and_then(|v| v8::Local::<v8::Function>::try_from(v).ok())
        {
            match f.call(tc, recv, &[]) {
                Some(ret) => {
                    if !ret.is_undefined() {
                        log_warn(&format!(
                            "WARN: unload_plugin('{}'): onUnload return is ignored - use state() for the reload handoff",
                            id
                        ));
                    }
                }
                None => {
                    let msg = tc
                        .exception()
                        .map(|e| e.to_rust_string_lossy(&*tc))
                        .unwrap_or_else(|| "onUnload threw".into());
                    log_warn(&format!("WARN: unload_plugin('{}'): onUnload error: {}", id, msg));
                }
            }
        }
    });
}

/// The ledger reverse-walk + iface cleanup + exports/context drop shared by `unload_plugin` (Active)
/// and `unload_partial` (never-Active). `REGISTRY.remove` yields the entry (also making `is_live` false
/// for any lingering resolver of this generation).
fn teardown_ledger_and_dispose(id: &str) {
    // (c) Ledger reverse-walk: the teardown authority.  REGISTRY.remove yields the entry (also makes
    // is_live false for any lingering resolver of this generation).
    if let Some(entry) = REGISTRY.with(|r| r.borrow_mut().remove(id)) {
        for res in entry.ledger.teardown_order() {
            match res {
                plugin::Resource::Timer(tid) => {
                    TIMERS.with(|t| {
                        t.borrow_mut().remove(tid);
                    });
                    DUE_TIMERS.with(|q| q.borrow_mut().retain(|n| *n != tid));
                    TIMER_LEASES.with(|m| m.borrow_mut().remove(&tid));
                    let _ = crate::jobs::take_resolver(tid);
                    // A repeating callback timer re-arms itself, so failing to drop it here would
                    // leave it firing into a dead context forever — the ledger is the teardown
                    // authority precisely so this does not depend on the plugin's own cleanup.
                    TIMER_CBS.with(|m| {
                        m.borrow_mut().remove(&tid);
                    });
                    TIMER_KILLED.with(|k| {
                        k.borrow_mut().remove(&tid);
                    });
                }
                plugin::Resource::Job(jid) => {
                    // The worker may still run; its late completion is a no-op (resolver gone).  Drop
                    // the resolver and, for a still-pending job we own, decrement PENDING_JOBS now so
                    // the (guarded) drain decrement does NOT double-count on the late completion.
                    crate::jobs::drop_if_present(jid);
                }
                plugin::Resource::Hook(sid) => {
                    // Already removed by (a); drop defensively (also catches a hook onUnload added
                    // AFTER (a)'s remove_by_owner).
                    let _ = FRAME.with(|f| f.borrow_mut().unsubscribe(sid));
                }
                plugin::Resource::Interface(name) => {
                    // Prunes IFACE_METHODS by interface NAME. Safe by construction since the
                    // contract-grammar slice: InterfaceRegistry::publish rejects a second live
                    // producer of a name (spec §4.8), so at most one producer can ever hold the
                    // methods being pruned here. (Retires the slice-5 TODO, which asked for a
                    // (producer_id, name) key against a case that can no longer occur.)
                    let removed = IFACES.with(|r| r.borrow_mut().remove_by_producer(id));
                    let subscribers: Vec<crate::interfaces::Subscriber> = removed
                        .into_iter()
                        .flat_map(|(_name, subscribers)| subscribers)
                        .collect();
                    IFACE_SUBS.with(|m| {
                        let mut callbacks = m.borrow_mut();
                        for subscriber in &subscribers {
                            callbacks.remove(&subscriber.sub_id);
                        }
                    });
                    for subscriber in subscribers {
                        release_resource(
                            &subscriber.consumer_id,
                            subscriber.consumer_gen,
                            &plugin::Resource::EventSub(subscriber.sub_id),
                        );
                    }
                    IFACE_METHODS.with(|m| {
                        m.borrow_mut().retain(|(iface, _method), _| iface != &name);
                    });
                }
                plugin::Resource::EventSub(sub_id) => {
                    // Defensive/idempotent: producer teardown may already have dropped this callback
                    // while releasing a surviving consumer's row. Never unwrap/expect/index here.
                    IFACE_SUBS.with(|m| {
                        m.borrow_mut().remove(&sub_id);
                    });
                    // The subscriber row is removed from the producer's list below via
                    // remove_subscribers_by_consumer(id) (belt-and-suspenders for any not yet dropped).
                }
                plugin::Resource::Import(_name) => { /* edge only; no Global to drop */ }
                plugin::Resource::DbConn(h) => {
                    // A late/never `close()` — teardown closes the connection now, passing the
                    // unloading plugin's OWN id as the owner (it owns every handle in its ledger).
                    // Idempotent (an already-closed handle is a harmless no-op inside db::close).
                    crate::db::close(h, id);
                }
                plugin::Resource::WsConn(conn_id) => {
                    // A late/never `close()` — teardown closes the ws connection now regardless of
                    // owner. A missing conn_id is a harmless no-op, including when public retirement
                    // already removed it after a connect failure or close callback.
                    crate::ws::shutdown_conn(conn_id);
                }
                plugin::Resource::NetConn(conn_id) => {
                    // A late/never `close()` — teardown drops the raw socket now regardless of owner
                    // (the ledger owns the id). A missing conn_id is a harmless no-op, including when
                    // public retirement already removed it after a connect failure or close callback.
                    crate::net::shutdown_conn(conn_id);
                }
                plugin::Resource::RemoteDbConn(h) => {
                    // Late/never close() — teardown drops the pool now (idempotent; a wrong/absent
                    // handle is a harmless no-op inside sqldb::close). Passes the unloading plugin's
                    // own id (it owns every handle in its ledger).
                    crate::sqldb::close(h, id);
                }
            }
        }
    }
    // Drop any subscriber rows this plugin (as a consumer) still holds, and its import declarations.
    // Idempotent with the per-resource drops above (remove() is a no-op on missing keys).
    let orphaned = IFACES.with(|r| r.borrow_mut().remove_subscribers_by_consumer(id));
    IFACE_SUBS.with(|m| {
        let mut mm = m.borrow_mut();
        for (_iface, sid) in orphaned {
            mm.remove(&sid);
        }
    });
    IFACES.with(|r| r.borrow_mut().clear_imports(id));
    clear_plugin_publishes(id);
    // Plugin-declared engine calls: drop this plugin's descriptor table (spec §12 "Unload" row). On
    // BOTH teardown paths (Active and never-Active), so a reload always re-resolves from scratch
    // rather than inheriting a stale call id.
    crate::gamedata_calls::drop_plugin(id);
    // Declarative inbound hooks: drop the descriptors this plugin DECLARED. Its subscriptions to
    // other owners' hooks are swept by the `HOOK_MUX` owner-store above. Nothing is uninstalled —
    // an installed detour outlives its declaring plugin by design (spec §6), and its slot is kept so
    // a reload re-uses it instead of burning a second one.
    crate::gamedata_hooks::drop_owner(id);
    // Removing timers/jobs (or an onUnload-added hook) changed the detour predicate — reconcile.
    refresh_detour();

    // (d) Drop the captured module.exports Global<Object> while the isolate is alive (before the
    // context Global).
    PLUGINS.with(|p| {
        if let Some(pi) = p.borrow_mut().get_mut(id) {
            pi.exports = None;
        }
    });

    // (e) NOW drop the Global<Context> (all inner Globals were released in a–d).  dispose_plugin_context
    // removes the PLUGINS entry (dropping the context Global) and the REGISTRY entry (already gone → no-op).
    dispose_plugin_context(id);
}

/// A snapshot of a `LOADING` entry's settle state (owned — `SettleState::Failed(String)` is not
/// `Clone`, so `finalize_loading_plugins` snapshots into this before releasing the LOADING borrow).
enum SettleSnapshot {
    Settled,
    Failed(String),
    InFlight,
}

/// Drive every in-flight load to its transition (design spec §5). Called (1) at the tail of
/// `frame_async_drain` — after the microtask checkpoint that runs factory continuations — and (2)
/// inline at the tail of `load_plugin_js` for the SYNC fast-path (a synchronous factory is already
/// `Settled`, so it arms + reconciles + goes Active within the same call). Must be called HOST-free.
pub(crate) fn finalize_loading_plugins() {
    let frame = FRAME_COUNTER.with(|c| c.get());
    let snapshot: Vec<(String, SettleSnapshot, bool, u64)> = LOADING.with(|l| {
        l.borrow()
            .iter()
            .map(|(id, e)| {
                let s = match &e.state {
                    SettleState::Settled => SettleSnapshot::Settled,
                    SettleState::Failed(m) => SettleSnapshot::Failed(m.clone()),
                    SettleState::InFlight => SettleSnapshot::InFlight,
                };
                (id.clone(), s, e.pending_reload, e.started_frame)
            })
            .collect()
    });

    for (id, state, pending_reload, started) in snapshot {
        match state {
            SettleSnapshot::Settled => {
                // (1) arm: replay buffered registrations + seal the ctx. A registration that throws
                // while arming aborts the arm (host TryCatch → eval_in_context Err) → Failed.
                let arm_ok = eval_in_context(&id, "globalThis.__s2_ctx_arm && globalThis.__s2_ctx_arm();").is_ok();
                if !arm_ok {
                    fail_load(&id, "a registration failed while arming at Active");
                    continue_or_reload(&id, pending_reload);
                    continue;
                }
                // (2) publishes reconciliation MOVES here from the loader (design spec §4).
                if let Err(e) = reconcile_publishes(&id) {
                    fail_load(&id, &format!("publishes: {}", e));
                    continue_or_reload(&id, pending_reload);
                    continue;
                }
                // (3) Active.
                PLUGINS.with(|p| {
                    if let Some(pi) = p.borrow_mut().get_mut(&id) {
                        pi.phase = crate::plugin::Phase::Active;
                    }
                });
                LOADING.with(|l| { l.borrow_mut().remove(&id); });
                if let Some(version) = plugin_manifest_version(&id) {
                    crate::crash::breadcrumb::plugin_loaded(&id, &version);
                }
                log_warn(&format!("[plugins] '{}' Active", id));
                if pending_reload {
                    crate::loader::request_reload(&id);
                }
            }
            SettleSnapshot::Failed(msg) => {
                fail_load(&id, &msg);
                continue_or_reload(&id, pending_reload);
            }
            SettleSnapshot::InFlight if frame.saturating_sub(started) > LOAD_TIMEOUT_FRAMES => {
                let _ = eval_in_context(&id, "globalThis.__s2_ctx_seal && globalThis.__s2_ctx_seal();");
                fail_load(&id, "factory did not settle within ~30s (LOAD_TIMEOUT_FRAMES)");
                continue_or_reload(&id, pending_reload);
            }
            _ => {}
        }
    }

    crate::loader::start_unblocked_waiters(); // T4 provides the real body; a no-op stub until then.
    fire_all_plugins_loaded_if_quiet();
}

/// Fire each Active plugin's pending `OnAllPluginsLoaded` once the load set is quiet
/// (no in-flight factories, no WAITING hard-dep parkers). Idempotent per plugin: the
/// prelude clears `__s2_on_all_plugins_loaded` after one call.
fn fire_all_plugins_loaded_if_quiet() {
    if LOADING.with(|l| !l.borrow().is_empty()) {
        return;
    }
    if crate::loader::has_waiting() {
        return;
    }
    let ids: Vec<String> = PLUGINS.with(|p| {
        p.borrow()
            .iter()
            .filter(|(_, pi)| pi.phase == crate::plugin::Phase::Active)
            .map(|(id, _)| id.clone())
            .collect()
    });
    for id in ids {
        let _ = eval_in_context(
            &id,
            "globalThis.__s2_fire_all_plugins_loaded && globalThis.__s2_fire_all_plugins_loaded();",
        );
    }
}

/// Fail a never-Active load: WARN + report + record the reason + drop the LOADING entry + tear down
/// the fresh (never-Active) context. The plugin is NOT running.
fn fail_load(id: &str, reason: &str) {
    log_warn(&format!(
        "WARN: load('{}') FAILED: {} - tearing down (the plugin is NOT running)",
        id, reason
    ));
    crate::crash::report_js_error(id, "factory", reason, "");
    FAILED_PLUGINS.with(|f| { f.borrow_mut().insert(id.to_string(), reason.to_string()); });
    LOADING.with(|l| { l.borrow_mut().remove(id); });
    unload_partial(id);
}

/// After a failed/timed-out load, if a reload was queued while it was loading, request it now. The
/// `pending_reload` flag is only ever set by T4's waiting-loads machinery, so this is a no-op today.
fn continue_or_reload(id: &str, pending_reload: bool) {
    if pending_reload {
        crate::loader::request_reload(id);
    }
}

/// The manifest version for `id` (set by the loader before load), for the `Active` breadcrumb.
fn plugin_manifest_version(id: &str) -> Option<String> {
    MANIFEST_VERSIONS.with(|m| m.borrow().get(id).cloned())
}

/// Record a plugin's manifest version (called by the loader before `load_plugin_js`), so the
/// `Active`-transition breadcrumb in `finalize_loading_plugins` can carry it without the manifest.
pub(crate) fn set_plugin_version(id: &str, version: &str) {
    MANIFEST_VERSIONS.with(|m| { m.borrow_mut().insert(id.to_string(), version.to_string()); });
}

/// The current frame count (loader-facing getter over `FRAME_COUNTER`). Used by the loader's WAITING
/// window (`start_unblocked_waiters`) and topo batch to bound the hard-dependency wait.
pub(crate) fn current_frame() -> u64 {
    FRAME_COUNTER.with(|c| c.get())
}

/// True while `id` has an in-flight factory load (between `create_plugin_context` and its
/// `Active`/`Failed` transition). The loader uses this to coalesce a reload-while-Loading into a
/// queued `pending_reload` rather than an unload+load race (design spec §5.3).
pub(crate) fn is_loading(id: &str) -> bool {
    LOADING.with(|l| l.borrow().contains_key(id))
}

/// Queue a reload for a plugin that is still Loading: the reload fires from `continue_or_reload` once
/// the in-flight load reaches its transition, instead of tearing down a half-loaded context.
pub(crate) fn queue_pending_reload(id: &str) {
    LOADING.with(|l| { if let Some(e) = l.borrow_mut().get_mut(id) { e.pending_reload = true; } });
}

/// True if `id`'s load FAILED (context disposed) — backs the `failed` state in `sm plugins list`.
pub(crate) fn is_failed(id: &str) -> bool {
    FAILED_PLUGINS.with(|f| f.borrow().contains_key(id))
}

/// Mark a plugin FAILED without it ever loading (loader refusals: apiVersion major, and — B1 —
/// a `compiledAgainst` typesSha256 mismatch). Shows as `failed` in `sm plugins list`; cleared by
/// the next successful load (load_plugin_js clears on fresh load) or by `clear_failed`.
pub(crate) fn set_failed(id: &str, reason: &str) {
    FAILED_PLUGINS.with(|f| { f.borrow_mut().insert(id.to_string(), reason.to_string()); });
}

/// Drop a FAILED entry (loader: the file vanished — a removed plugin is not `failed`, it is gone).
pub(crate) fn clear_failed(id: &str) {
    FAILED_PLUGINS.with(|f| { f.borrow_mut().remove(id); });
}

/// Every plugin id whose load FAILED (for `sm plugins list`'s `failed` state).
pub(crate) fn failed_plugin_ids() -> Vec<String> {
    FAILED_PLUGINS.with(|f| f.borrow().keys().cloned().collect())
}

/// True if an interface `name` currently has a producer (published AND live), regardless of any
/// consumer's version range. The loader's topo/WAITING gate uses this to decide whether a hard
/// dependency is satisfied before starting a consumer's load.
pub(crate) fn iface_published(name: &str) -> bool {
    IFACES.with(|r| r.borrow().producer_of(name).is_some())
}

/// The published contract hash for `name` (empty string = producer ships none), None when
/// unpublished. The loader's `verify_compiled_against` (B1) fail-fast gate reads this.
pub(crate) fn iface_published_types_sha256(name: &str) -> Option<String> {
    IFACES.with(|r| r.borrow().lookup(name).map(|e| e.types_sha256.clone()))
}

/// Slice 5E.3: drop any pending reload-handoff blob for `id` WITHOUT consuming it — called by the
/// loader on a FINAL removal (Vanished) so a deleted plugin's captured state is discarded rather than
/// handed to a future re-add of the same id.
pub(crate) fn clear_pending_handoff(id: &str) {
    PENDING_HANDOFF.with(|h| { h.borrow_mut().remove(id); });
}

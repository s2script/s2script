//! The host-global admin cache — flags, immunity, and command overrides — plus the `__s2_admin_*`
//! natives over it and its teardown registrations.
//!
//! Two tiers (file `admins.json` ⊕ runtime `Admin.add`), each SteamID64 → a u64 flag bitmask, with
//! parallel per-tier immunity levels and (file-tier) per-admin + global command overrides. Shared
//! across all plugin contexts: V8 contexts are isolated, so a runtime add in one plugin must be
//! visible to another's gating — hence host-global rather than per-context. Engine-generic; holds no
//! V8 handles and knows nothing about any game.
//!
//! Extracted from `v8host.rs` under the core-stabilization program — see `crate::usermsg` for the
//! shape (a feature owns its state, natives, dispatch and teardown together).

use crate::v8host::set_native;

thread_local! {
    /// Flag bitmasks per tier. `get` = file ∪ runtime.
    static ADMIN_FILE:    std::cell::RefCell<std::collections::HashMap<String, u64>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static ADMIN_RUNTIME: std::cell::RefCell<std::collections::HashMap<String, u64>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// One-shot guard so admins.json loads once (the first plugin CONTEXT created — the admin prelude
    /// is always injected, like every @s2script/* module — not per plugin).
    static ADMIN_FILE_LOADED: std::cell::Cell<bool> = std::cell::Cell::new(false);

    /// Per-tier immunity levels (mirrors ADMIN_FILE/ADMIN_RUNTIME). get = max(file, runtime).
    static ADMIN_FILE_IMMUNITY:    std::cell::RefCell<std::collections::HashMap<String, i32>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    static ADMIN_RUNTIME_IMMUNITY: std::cell::RefCell<std::collections::HashMap<String, i32>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Per-admin command overrides (file tier only — the resolver merges an admin's groups' override
    /// blocks). sid -> cmd -> (required_mask, is_public). is_public true => anyone (flag "").
    static ADMIN_OVERRIDES: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashMap<String, (u64, bool)>>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Global command overrides (admin_overrides.json). cmd -> (required_mask, is_public).
    static ADMIN_GLOBAL_OVERRIDES: std::cell::RefCell<std::collections::HashMap<String, (u64, bool)>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
}


/// `__s2_admin_set(steamid, flags, immunity, runtime)` — set/overwrite a SteamID's flags + immunity in
/// the file(false)/runtime(true) tier.
fn s2_admin_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let flags = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let immunity = args.get(2).number_value(scope).unwrap_or(0.0) as i32;
        let runtime = args.get(3).boolean_value(scope);
        if runtime {
            ADMIN_RUNTIME.with(|m| { m.borrow_mut().insert(sid.clone(), flags); });
            ADMIN_RUNTIME_IMMUNITY.with(|m| { m.borrow_mut().insert(sid, immunity); });
        } else {
            ADMIN_FILE.with(|m| { m.borrow_mut().insert(sid.clone(), flags); });
            ADMIN_FILE_IMMUNITY.with(|m| { m.borrow_mut().insert(sid, immunity); });
        }
    }));
}

/// `__s2_admin_get(steamid) -> number` — the UNION of both tiers (0 = not an admin).
fn s2_admin_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { rv.set_double(0.0); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let f = ADMIN_FILE.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        let r = ADMIN_RUNTIME.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        rv.set_double((f | r) as f64);
    }));
}

/// `__s2_admin_get_immunity(steamid) -> number` — max immunity across both tiers (0 = none).
fn s2_admin_get_immunity(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { rv.set_double(0.0); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let f = ADMIN_FILE_IMMUNITY.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        let r = ADMIN_RUNTIME_IMMUNITY.with(|m| m.borrow().get(&sid).copied().unwrap_or(0));
        rv.set_double(f.max(r) as f64);
    }));
}

/// `__s2_admin_add_override(steamid, cmd, mask, isPublic)` — a per-admin (file-tier) command override.
fn s2_admin_add_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 4 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let cmd = args.get(1).to_rust_string_lossy(scope);
        let mask = args.get(2).number_value(scope).unwrap_or(0.0) as u64;
        let is_public = args.get(3).boolean_value(scope);
        ADMIN_OVERRIDES.with(|m| {
            m.borrow_mut().entry(sid).or_default().insert(cmd, (mask, is_public));
        });
    }));
}

/// `__s2_admin_set_global_override(cmd, mask, isPublic)` — a global (file-tier) command override.
fn s2_admin_set_global_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let cmd = args.get(0).to_rust_string_lossy(scope);
        let mask = args.get(1).number_value(scope).unwrap_or(0.0) as u64;
        let is_public = args.get(2).boolean_value(scope);
        ADMIN_GLOBAL_OVERRIDES.with(|m| { m.borrow_mut().insert(cmd, (mask, is_public)); });
    }));
}

/// `__s2_admin_override(steamid, cmd) -> string` — "" (none) / "public" / decimal mask. Per-admin beats global.
fn s2_admin_override(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { let s = v8::String::new(scope, "").unwrap(); rv.set(s.into()); return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let cmd = args.get(1).to_rust_string_lossy(scope);
        let hit = ADMIN_OVERRIDES.with(|m| m.borrow().get(&sid).and_then(|c| c.get(&cmd).copied()))
            .or_else(|| ADMIN_GLOBAL_OVERRIDES.with(|m| m.borrow().get(&cmd).copied()));
        let out = match hit {
            None => String::new(),
            Some((_, true)) => "public".to_string(),
            Some((mask, false)) => mask.to_string(),
        };
        let s = v8::String::new(scope, &out).unwrap();
        rv.set(s.into());
    }));
}

/// `__s2_admin_remove(steamid, runtime)` — remove from a tier.
fn s2_admin_remove(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let sid = args.get(0).to_rust_string_lossy(scope);
        let runtime = args.get(1).boolean_value(scope);
        if runtime {
            ADMIN_RUNTIME.with(|m| { m.borrow_mut().remove(&sid); });
            ADMIN_RUNTIME_IMMUNITY.with(|m| { m.borrow_mut().remove(&sid); });
        } else {
            ADMIN_FILE.with(|m| { m.borrow_mut().remove(&sid); });
            ADMIN_FILE_IMMUNITY.with(|m| { m.borrow_mut().remove(&sid); });
        }
    }));
}

/// `__s2_admin_clear_file()` — wipe the file tier (Admin.reload re-reads into it), plus the file-tier
/// immunity map, per-admin overrides, and global overrides (all file-tier-sourced).
fn s2_admin_clear_file(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ADMIN_FILE.with(|m| m.borrow_mut().clear());
        ADMIN_FILE_IMMUNITY.with(|m| m.borrow_mut().clear());
        ADMIN_OVERRIDES.with(|m| m.borrow_mut().clear());
        ADMIN_GLOBAL_OVERRIDES.with(|m| m.borrow_mut().clear());
    }));
}

/// `__s2_admin_mark_loaded() -> boolean` — returns the PRIOR loaded state, then sets it true (one-shot load guard).
fn s2_admin_mark_loaded(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let prev = ADMIN_FILE_LOADED.with(|c| { let p = c.get(); c.set(true); p });
        rv.set_bool(prev);
    }));
}
/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_admin_set", s2_admin_set);
    set_native(scope, global_obj, "__s2_admin_get", s2_admin_get);
    set_native(scope, global_obj, "__s2_admin_get_immunity", s2_admin_get_immunity);
    set_native(scope, global_obj, "__s2_admin_add_override", s2_admin_add_override);
    set_native(scope, global_obj, "__s2_admin_set_global_override", s2_admin_set_global_override);
    set_native(scope, global_obj, "__s2_admin_override", s2_admin_override);
    set_native(scope, global_obj, "__s2_admin_remove", s2_admin_remove);
    set_native(scope, global_obj, "__s2_admin_clear_file", s2_admin_clear_file);
    set_native(scope, global_obj, "__s2_admin_mark_loaded", s2_admin_mark_loaded);
}

/// Reset every slot on a core re-init. `AfterIsolateDrop`: plain Rust maps, no V8 handles.
///
/// ALL SEVEN are registered. Before this module existed only three were (`ADMIN_FILE`,
/// `ADMIN_RUNTIME`, `ADMIN_FILE_LOADED`) — the immunity and override maps were never reset, so a
/// re-init cleared an admin's FLAGS while leaving their IMMUNITY and command overrides behind.
/// `s2_admin_get_immunity` keys on the SteamID alone and never consults the flag maps, so the stale
/// level kept answering for a SteamID that was no longer an admin at all. Co-locating the state with
/// its teardown is what made the omission visible; `admin_reinit_clears_every_tier` pins it.
pub(crate) fn register_singletons() {
    use crate::process_singletons::ResetPhase::AfterIsolateDrop;
    fn reg(name: &'static str, f: impl Fn() + 'static) {
        crate::process_singletons::register(name, AfterIsolateDrop, Box::new(f));
    }
    reg("ADMIN_FILE",              || ADMIN_FILE.with(|m| m.borrow_mut().clear()));
    reg("ADMIN_RUNTIME",           || ADMIN_RUNTIME.with(|m| m.borrow_mut().clear()));
    reg("ADMIN_FILE_LOADED",       || ADMIN_FILE_LOADED.with(|c| c.set(false)));
    reg("ADMIN_FILE_IMMUNITY",     || ADMIN_FILE_IMMUNITY.with(|m| m.borrow_mut().clear()));
    reg("ADMIN_RUNTIME_IMMUNITY",  || ADMIN_RUNTIME_IMMUNITY.with(|m| m.borrow_mut().clear()));
    reg("ADMIN_OVERRIDES",         || ADMIN_OVERRIDES.with(|m| m.borrow_mut().clear()));
    reg("ADMIN_GLOBAL_OVERRIDES",  || ADMIN_GLOBAL_OVERRIDES.with(|m| m.borrow_mut().clear()));
}

// Per-feature tests over the SHARED in-isolate harness (`v8host::frame_tests`) — see `crate::usermsg`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{eval_in_context_string, logger, LOG};
    use crate::v8host::{create_plugin_context, eval_in_context, init, shutdown};

    /// Regression: a core re-init must clear EVERY admin tier, not just flags.
    ///
    /// Only `ADMIN_FILE`, `ADMIN_RUNTIME` and `ADMIN_FILE_LOADED` were ever registered for reset;
    /// the immunity and override maps were not. `s2_admin_get_immunity` keys on the SteamID alone and
    /// never consults the flag maps, so after a re-init a SteamID whose flags were cleared kept
    /// answering with its OLD immunity level — and its old command overrides. Removing that
    /// registration set makes this test fail.
    #[test]
    fn admin_reinit_clears_every_tier() {
        init(logger).unwrap();
        create_plugin_context("ar");
        eval_in_context("ar", r#"
            __s2_admin_set('7001', '255', 99, false);   // file tier,    immunity 99
            __s2_admin_set('7002', '255', 50, true);    // runtime tier, immunity 50
            __s2_admin_add_override('7001', 'sm_slay', 255, false);
            __s2_admin_set_global_override('sm_kick', 255, false);
        "#).unwrap();
        // Assert the writes LANDED first — otherwise the post-re-init assertions pass vacuously.
        assert_eq!(eval_in_context_string("ar", "String(__s2_admin_get_immunity('7001'))"), "99");
        assert_eq!(eval_in_context_string("ar", "String(__s2_admin_get_immunity('7002'))"), "50");
        assert_eq!(eval_in_context_string("ar", "__s2_admin_override('7001','sm_slay')"), "255");
        assert_eq!(eval_in_context_string("ar", "__s2_admin_override('9999','sm_kick')"), "255",
            "the global override answers for any SteamID");
        shutdown();

        // A fresh init must see NOTHING from the previous one — every tier, not just the flag maps.
        init(logger).unwrap();
        create_plugin_context("ar2");
        assert_eq!(eval_in_context_string("ar2", "String(__s2_admin_get('7001'))"), "0",
            "flags must not survive a re-init");
        assert_eq!(eval_in_context_string("ar2", "String(__s2_admin_get_immunity('7001'))"), "0",
            "FILE immunity must not survive a re-init");
        assert_eq!(eval_in_context_string("ar2", "String(__s2_admin_get_immunity('7002'))"), "0",
            "RUNTIME immunity must not survive a re-init");
        assert_eq!(eval_in_context_string("ar2", "__s2_admin_override('7001','sm_slay')"), "",
            "per-admin overrides must not survive a re-init");
        assert_eq!(eval_in_context_string("ar2", "__s2_admin_override('9999','sm_kick')"), "",
            "global overrides must not survive a re-init");
        shutdown();
    }
    /// Slice 6.2 Task 1: two-tier admin cache (file + runtime) UNION, per-tier remove, clear_file,
    /// one-shot load guard, and `client_steamid` degrades to "0" without the op.
    #[test]
    fn admin_cache_two_tier_union_and_guard() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // set file + runtime tiers; get unions them.
        eval_in_context("p", "__s2_admin_set('111', 4, 0, false); __s2_admin_set('111', 1, 0, true);").unwrap(); // file KICK(4) + runtime RESERVATION(1)
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('111'))"), "5"); // 4|1
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('999'))"), "0"); // absent
        // remove runtime tier only → file remains.
        eval_in_context("p", "__s2_admin_remove('111', true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('111'))"), "4");
        // clear_file wipes file tier.
        eval_in_context("p", "__s2_admin_clear_file();").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('111'))"), "0");
        // load guard: the prelude already called __s2_admin_mark_loaded() in create_plugin_context,
        // so subsequent calls return true (already-loaded state).
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_mark_loaded())"), "true");
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_mark_loaded())"), "true");
        // client_steamid degrades to "0" without the op.
        assert_eq!(eval_in_context_string("p", "__s2_client_steamid(0)"), "0");
        // client_kick degrades to a no-op (undefined) without the op.
        assert_eq!(eval_in_context_string("p", "String(__s2_client_kick(0, 'x'))"), "undefined");
        // server_command degrades to a no-op (undefined); server_map_valid to 0; the module wires them.
        assert_eq!(eval_in_context_string("p", "String(__s2_server_command('x'))"), "undefined");
        assert_eq!(eval_in_context_string("p", "String(__s2_server_map_valid('x'))"), "0");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_server.Server.command"), "function");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.isMapValid('x'))"), "false");
        // Slice 6.6: the damage natives degrade without ops (read->0, victim->-1, write no-op) and the
        // @s2script/damage module wires (Damage.onPre a function; DamageInfo reads degrade to 0/null).
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_read_float(68))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_read_int(60))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_victim())"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_damage_write_float(68, 5))"), "undefined");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_damage.Damage.onPre"), "function");
        assert_eq!(eval_in_context_string("p", "String(new __s2pkg_damage.DamageInfo().damage)"), "0");
        assert_eq!(eval_in_context_string("p", "String(new __s2pkg_damage.DamageInfo().victim)"), "null");
        // Slice 6.7: cvar_get degrades to "" without the op; Server.getCvar/setCvar wired.
        assert_eq!(eval_in_context_string("p", "String(__s2_cvar_get('sv_gravity'))"), "");
        assert_eq!(eval_in_context_string("p", "String(__s2_cvar_set('sv_gravity', '800'))"), "false");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_server.Server.getCvar"), "function");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_server.Server.setCvar"), "function");
        // reservedslots+basetriggers: server-info natives degrade (max_clients->0, map_name->"", game_time->0)
        // and the @s2script/server module exposes maxPlayers/mapName/gameTime getters that pass them through.
        assert_eq!(eval_in_context_string("p", "String(__s2_server_max_clients())"), "0");
        assert_eq!(eval_in_context_string("p", "__s2_server_map_name()"), "");
        assert_eq!(eval_in_context_string("p", "typeof __s2_server_map_name()"), "string");
        assert_eq!(eval_in_context_string("p", "String(__s2_server_game_time())"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.maxPlayers)"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.mapName)"), "");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_server.Server.gameTime)"), "0");
        // Slice 6.12: plugin natives degrade (no file-watch in-isolate → empty list, ops false) + module wires.
        assert_eq!(eval_in_context_string("p", "__s2_plugins_list()"), "[]");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_unload('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_reload('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_plugin_load('x'))"), "false");
        assert_eq!(eval_in_context_string("p", "JSON.stringify(__s2pkg_plugins.Plugins.list())"), "[]");
        assert_eq!(eval_in_context_string("p", "typeof __s2pkg_plugins.Plugins.reload"), "function");
        shutdown();
    }

    /// Admin-groups slice Task 1: per-tier immunity (max across tiers) + command overrides (per-admin
    /// beats global; "public" sentinel) + clear_file wiping file immunity/overrides while runtime survives.
    #[test]
    fn admin_immunity_and_overrides() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // immunity: max across tiers
        eval_in_context("p", "__s2_admin_set('222', 4, 30, false); __s2_admin_set('222', 8, 70, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get('222'))"), "12");        // 4|8
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('222'))"), "70"); // max(30,70)
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('999'))"), "0");  // absent
        // overrides: per-admin beats global; public sentinel
        eval_in_context("p", "__s2_admin_set_global_override('sm_x', 2, false); __s2_admin_add_override('222','sm_x',4,false);").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_x')"), "4");    // per-admin wins
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('other','sm_x')"), "2");  // falls to global
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','nope')"), "");     // no override
        eval_in_context("p", "__s2_admin_set_global_override('sm_pub', 0, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_pub')"), "public");
        // clear_file wipes file immunity + overrides + global overrides; runtime immunity survives
        eval_in_context("p", "__s2_admin_clear_file();").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_get_immunity('222'))"), "70"); // runtime kept
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('222','sm_x')"), "");
        assert_eq!(eval_in_context_string("p", "__s2_admin_override('other','sm_x')"), "");
        shutdown();
    }

    /// Slice 6.2 Task 2: `@s2script/admin` prelude module — ADMFLAG constants, Admin.add/get/hasFlags,
    /// root-implies-all, non-admin→null, __s2_admin_check hook, parseAdmins name→bit mapping.
    #[test]
    fn admin_module_flags_api_and_hook() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // ADMFLAG bit values (SM-exact).
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.ADMFLAG.KICK)"), "4");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.ADMFLAG.ROOT)"), String::from("16384"));
        // Admin.add (runtime) + get + hasFlags (exact-subset + root=all).
        eval_in_context("p", "__s2pkg_admin.Admin.add('555', __s2pkg_admin.ADMFLAG.KICK | __s2pkg_admin.ADMFLAG.CHAT);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('555').hasFlags(__s2pkg_admin.ADMFLAG.CHAT))"), "true");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('555').hasFlags(__s2pkg_admin.ADMFLAG.BAN))"), "false");
        eval_in_context("p", "__s2pkg_admin.Admin.add('777', __s2pkg_admin.ADMFLAG.ROOT);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('777').hasFlags(__s2pkg_admin.ADMFLAG.BAN))"), "true"); // root ⇒ all
        // Non-admin → null.
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('000'))"), "null");
        // The check hook is installed + honours the cache (bot slot → steamid "0" → not admin → false).
        assert_eq!(eval_in_context_string("p", "String(typeof globalThis.__s2_admin_check)"), "function");
        assert_eq!(eval_in_context_string("p", "String(globalThis.__s2_admin_check(0, __s2pkg_admin.ADMFLAG.CHAT))"), "false");
        // Hardening: even a misconfigured "0" admin entry must NOT grant a bot/unauth (steamid "0") — forSlot guards it.
        eval_in_context("p", "__s2_admin_set('0', __s2pkg_admin.ADMFLAG.ROOT, 0, true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(globalThis.__s2_admin_check(0, __s2pkg_admin.ADMFLAG.CHAT))"), "false"); // "0" never an admin
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.forSlot(0))"), "null");
        // parseAdmins (renamed from parseFile in the admin-groups slice): name→bit mapping (file-tier path).
        eval_in_context("p", r#"__s2_admin_parseAdmins('{"888":["kick"]}', true);"#).unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('888').hasFlags(__s2pkg_admin.ADMFLAG.KICK))"), "true");
        shutdown();
    }

    /// Admin-groups slice Task 2: the flag-token parser — a compact SM letter-string, an array of names,
    /// a whole-string name, and the 'z'→ROOT letter.
    #[test]
    fn admin_flag_parser() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('bcdefg'))"), "126"); // bits 1..6
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags(['kick','ban']))"), "12"); // KICK|BAN
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('kick'))"), "4");   // whole string = a name
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('z'))"), "16384");  // ROOT
        // SM custom flags: letters o..t are Custom1..Custom6 at bits 15..20 (ROOT holds bit 14).
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('o'))"), "32768");   // CUSTOM1
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('t'))"), "1048576"); // CUSTOM6
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('op'))"), "98304");  // CUSTOM1|CUSTOM2
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('bo'))"), "32770");  // GENERIC|CUSTOM1
        // ...and by name, through the same table lookup the other flags use.
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('custom1'))"), "32768");
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags(['custom1','custom6']))"), "1081344");
        // 'u'..'y' remain unmapped — SM defines no flag there.
        assert_eq!(eval_in_context_string("p", "String(__s2_admin_parseFlags('u'))"), "0");
        shutdown();
    }

    /// Admin-groups slice Task 2: group resolution — an admin's own flags/immunity merge with their
    /// groups' (group flags OR'd in, immunity MAX'd), an unknown group is skipped+WARNed but the admin's
    /// own flags survive, and the full parseGroups→parseAdmins(pushCore) pipeline lands in the core cache.
    #[test]
    fn admin_group_resolution() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        eval_in_context("p", "__s2_admin_parseGroups('{\"G\":{\"flags\":\"cd\",\"immunity\":50}}');").unwrap();
        // own immunity 10 loses to group 50; flags = own(none) ∪ group(KICK|BAN)=12; groups=['G']
        assert_eq!(eval_in_context_string("p",
            "(function(){var r=__s2_admin_resolveEntry({groups:['G'],immunity:10}); return r.mask+'/'+r.immunity+'/'+r.groups.join(',');})()"),
            "12/50/G");
        // unknown group skipped, own flags kept
        assert_eq!(eval_in_context_string("p",
            "(function(){var r=__s2_admin_resolveEntry({groups:['Nope'],flags:['slay']}); return r.mask+'/'+r.groups.length;})()"),
            "32/0");
        // full push: parseGroups then parseAdmins(pushCore) -> Admin.get reads immunity + groups from core+registry
        eval_in_context("p", "__s2_admin_parseAdmins('{\"111\":{\"groups\":[\"G\"],\"immunity\":5}}', true);").unwrap();
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('111').immunity)"), "50");
        assert_eq!(eval_in_context_string("p", "__s2pkg_admin.Admin.get('111').groups.join(',')"), "G");
        assert_eq!(eval_in_context_string("p", "String(__s2pkg_admin.Admin.get('nobody'))"), "null");
        // an override with an unknown flag token is SKIPPED (not installed as a weakening mask-0)
        // 'v' is deliberately a letter SM maps to no flag; 'q' used to serve here, but q..t became
        // CUSTOM3..CUSTOM6 when custom flags landed.
        eval_in_context("p", "__s2_admin_parseGroups('{\"H\":{\"flags\":\"c\",\"overrides\":{\"sm_x\":\"v\",\"sm_y\":\"d\"}}}');").unwrap();
        assert_eq!(eval_in_context_string("p",
            "Object.keys(__s2_admin_resolveEntry({groups:['H']}).overrides).sort().join(',')"), "sm_y");
        shutdown();
    }

    /// Admin-groups slice Task 2: the pure immunity-comparison hook consumed by Player.target's filter —
    /// console is infinite, a non-immune target is always fair game, and equal immunity can target.
    #[test]
    fn admin_can_target_immunity() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(-1, 0, 100))"), "true");  // console infinite
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 0, 0))"), "true");      // non-immune target
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 50, 100))"), "false");  // punch up blocked
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 100, 50))"), "true");   // punch down
        assert_eq!(eval_in_context_string("p", "String(__s2_canTargetImm(0, 50, 50))"), "true");    // equal can target
        shutdown();
    }
}

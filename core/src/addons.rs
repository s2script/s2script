//! `@s2script/sdk/addons` — workshop addons mounted on the server and pushed to every client
//! (`shim/src/addons.cpp`).
//!
//! Engine-generic: every Source 2 server mounts addons into its "GAME" search path and tells each
//! client which ones to load. This module is the thin native layer: list normalization and the
//! download policy live in the prelude and the managing plugin; the per-client handshake runs in
//! the shim, inside the engine hooks that need it.
//!
//! One plugin manages the addon state at a time. The first plugin that changes it owns it; a second
//! plugin's change throws `S2Error { code: "AddonsOwned" }`. When the owner unloads, the store below
//! resets the shim (unmounts everything and clears every list), so a hot reload starts clean and
//! no mount outlives the plugin that made it.

use crate::native::throw_code;
use crate::v8host::{current_plugin, engine_ops, set_native};
use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::c_char;

thread_local! {
    static OWNER: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Room for a comma-separated list of 20-digit ids.
const LIST_CAP: usize = 16 * 1024;

fn cstring(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> CString {
    let s = if v.is_null_or_undefined() { String::new() } else { v.to_rust_string_lossy(scope) };
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// Claim the addon state for the calling plugin. False (with an exception thrown) when another
/// plugin already manages it.
fn claim(scope: &mut v8::PinScope) -> bool {
    let me = current_plugin(scope).unwrap_or_default();
    let holder = OWNER.with(|o| {
        let mut o = o.borrow_mut();
        match o.as_ref() {
            Some(h) if *h != me => Some(h.clone()),
            _ => {
                *o = Some(me);
                None
            }
        }
    });
    match holder {
        None => true,
        Some(h) => {
            throw_code(scope, "AddonsOwned", &format!("addons are managed by plugin '{h}'"));
            false
        }
    }
}

fn read_list(scope: &mut v8::PinScope, f: Option<extern "C" fn(*mut c_char, i32) -> i32>,
             mut rv: v8::ReturnValue) {
    let mut buf = vec![0u8; LIST_CAP];
    let n = f.map(|f| f(buf.as_mut_ptr().cast(), buf.len() as i32)).unwrap_or(0).max(0) as usize;
    let text = String::from_utf8_lossy(&buf[..n.min(buf.len())]).into_owned();
    if let Some(s) = v8::String::new(scope, &text) { rv.set(s.into()); }
}

/// Native `__s2_addons_status() -> number`. Bit 0 mounting, bit 1 map addon list, bit 2 client handshake.
fn s2_addons_status(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(engine_ops().and_then(|o| o.addons_status).map(|f| f()).unwrap_or(0));
    }));
}

/// Native `__s2_addons_mount(id, folder, addToTail) -> number` (the shim's result code).
fn s2_addons_mount(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if !claim(scope) { return; }
        let (id, folder) = (cstring(scope, args.get(0)), cstring(scope, args.get(1)));
        let tail = args.get(2).boolean_value(scope) as i32;
        let Some(f) = engine_ops().and_then(|o| o.addons_mount) else { return };
        rv.set_int32(f(id.as_ptr(), folder.as_ptr(), tail));
    }));
}

/// Native `__s2_addons_unmount(id) -> boolean`.
fn s2_addons_unmount(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if !claim(scope) { return; }
        let id = cstring(scope, args.get(0));
        let Some(f) = engine_ops().and_then(|o| o.addons_unmount) else { return };
        rv.set_bool(f(id.as_ptr()) == 1);
    }));
}

/// Native `__s2_addons_set_list(kind, csv, xuid?)`. kind 0: extra (map loads), 1: every client,
/// 2: one client (xuid).
fn s2_addons_set_list(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !claim(scope) { return; }
        let kind = args.get(0).int32_value(scope).unwrap_or(-1);
        let csv = cstring(scope, args.get(1));
        let Some(ops) = engine_ops() else { return };
        match kind {
            0 => { if let Some(f) = ops.addons_set_extra { f(csv.as_ptr()) } }
            1 => { if let Some(f) = ops.addons_set_client_global { f(csv.as_ptr()) } }
            2 => {
                let xuid = cstring(scope, args.get(2));
                if let Some(f) = ops.addons_set_client { f(xuid.as_ptr(), csv.as_ptr()) }
            }
            _ => {}
        }
    }));
}

/// Native `__s2_addons_configure(extraTimeout, connectTimeout, cacheClients, cacheSeconds)`.
fn s2_addons_configure(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !claim(scope) { return; }
        let num = |scope: &mut v8::PinScope, i: i32| {
            args.get(i).number_value(scope).filter(|n| n.is_finite()).unwrap_or(0.0)
        };
        let (extra, connect) = (num(scope, 0), num(scope, 1));
        let cache = args.get(2).boolean_value(scope) as i32;
        let cache_seconds = num(scope, 3);
        if let Some(f) = engine_ops().and_then(|o| o.addons_configure) { f(extra, connect, cache, cache_seconds) }
    }));
}

/// Native `__s2_addons_clear_cache(xuid?)`. Forgets what one client (or every client) downloaded.
fn s2_addons_clear_cache(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !claim(scope) { return; }
        let xuid = cstring(scope, args.get(0));
        if let Some(f) = engine_ops().and_then(|o| o.addons_clear_client_cache) { f(xuid.as_ptr()) }
    }));
}

/// Native `__s2_addons_current_map() -> string`.
fn s2_addons_current_map(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_list(scope, engine_ops().and_then(|o| o.addons_current_map), rv);
    }));
}

/// Native `__s2_addons_mounted() -> string`.
fn s2_addons_mounted(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_list(scope, engine_ops().and_then(|o| o.addons_mounted), rv);
    }));
}

/// Native `__s2_addons_client_list(xuid) -> string`.
fn s2_addons_client_list(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let xuid = cstring(scope, args.get(0));
        let mut buf = vec![0u8; LIST_CAP];
        let n = engine_ops().and_then(|o| o.addons_client_list)
            .map(|f| f(xuid.as_ptr(), buf.as_mut_ptr().cast(), buf.len() as i32)).unwrap_or(0).max(0) as usize;
        let text = String::from_utf8_lossy(&buf[..n.min(buf.len())]).into_owned();
        if let Some(s) = v8::String::new(scope, &text) { rv.set(s.into()); }
    }));
}

/// Native `__s2_addons_owner() -> string | null`. The plugin that manages the addon state.
fn s2_addons_owner(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if let Some(o) = OWNER.with(|o| o.borrow().clone()) {
            if let Some(s) = v8::String::new(scope, &o) { rv.set(s.into()); }
        }
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_addons_status", s2_addons_status);
    set_native(scope, global_obj, "__s2_addons_mount", s2_addons_mount);
    set_native(scope, global_obj, "__s2_addons_unmount", s2_addons_unmount);
    set_native(scope, global_obj, "__s2_addons_set_list", s2_addons_set_list);
    set_native(scope, global_obj, "__s2_addons_configure", s2_addons_configure);
    set_native(scope, global_obj, "__s2_addons_clear_cache", s2_addons_clear_cache);
    set_native(scope, global_obj, "__s2_addons_current_map", s2_addons_current_map);
    set_native(scope, global_obj, "__s2_addons_mounted", s2_addons_mounted);
    set_native(scope, global_obj, "__s2_addons_client_list", s2_addons_client_list);
    set_native(scope, global_obj, "__s2_addons_owner", s2_addons_owner);
}

/// The owning plugin's unload resets the shim: every mount comes off and every list is emptied.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "ADDONS_OWNER",
        Box::new(|owner| {
            let was_owner = OWNER.with(|o| {
                let mut o = o.borrow_mut();
                if o.as_deref() == Some(owner) { *o = None; true } else { false }
            });
            if was_owner {
                if let Some(f) = engine_ops().and_then(|o| o.addons_reset) { f() }
            }
        }),
        Box::new(|_ids| {}),
        Box::new(|| OWNER.with(|o| *o.borrow_mut() = None)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string};
    use crate::v8host::{create_plugin_context, init, set_engine_ops, shutdown, unload_plugin, S2EngineOps};
    use std::os::raw::c_int;
    use std::sync::Mutex;

    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn log(s: String) { LOG.lock().unwrap().push(s); }
    fn take() -> Vec<String> { std::mem::take(&mut *LOG.lock().unwrap()) }
    fn text(p: *const c_char) -> String {
        unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    extern "C" fn status() -> c_int { 7 }
    extern "C" fn mount(id: *const c_char, folder: *const c_char, tail: c_int) -> c_int {
        log(format!("mount {} {} {}", text(id), text(folder), tail));
        1
    }
    extern "C" fn unmount(id: *const c_char) -> c_int { log(format!("unmount {}", text(id))); 1 }
    extern "C" fn set_extra(csv: *const c_char) { log(format!("extra {}", text(csv))); }
    extern "C" fn set_global(csv: *const c_char) { log(format!("global {}", text(csv))); }
    extern "C" fn set_client(x: *const c_char, csv: *const c_char) { log(format!("client {} {}", text(x), text(csv))); }
    extern "C" fn configure(a: f64, b: f64, c: c_int, d: f64) { log(format!("configure {a} {b} {c} {d}")); }
    extern "C" fn reset() { log("reset".into()); }
    extern "C" fn mounted(out: *mut c_char, cap: c_int) -> c_int {
        let s = b"111,222";
        let n = s.len().min(cap as usize - 1);
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), out.cast(), n); *out.add(n) = 0; }
        n as c_int
    }

    fn ops() -> S2EngineOps {
        S2EngineOps {
            addons_status: Some(status), addons_mount: Some(mount), addons_unmount: Some(unmount),
            addons_set_extra: Some(set_extra), addons_set_client_global: Some(set_global),
            addons_set_client: Some(set_client), addons_configure: Some(configure),
            addons_reset: Some(reset), addons_mounted: Some(mounted),
            ..S2EngineOps::none()
        }
    }

    #[test]
    fn natives_reach_the_shim_and_the_owner_unload_resets_it() {
        init(dummy_logger()).unwrap();
        set_engine_ops(Some(ops()));
        take();
        create_plugin_context("addons_a");
        create_plugin_context("addons_b");
        let r = eval_in_context_string("addons_a", r#"
            var a = globalThis.__s2pkg_addons.Addons;
            a.setExtraAddons(["111", 222, " 111 "]);
            a.setClientAddons(["333"]);
            a.setClientAddons(["444"], "76561198000000001");
            a.configure({ extraAddonTimeout: 5, connectTimeout: 20, cacheClients: true, cacheSeconds: 60 });
            JSON.stringify([a.status().mount, a.mount("111", "/w/111"), a.mounted(), a.owner()]);
        "#);
        assert_eq!(r, r#"[true,"mounted",["111","222"],"addons_a"]"#);
        let other = eval_in_context_string("addons_b", r#"
            try { globalThis.__s2pkg_addons.Addons.setExtraAddons(["9"]); "no" } catch (e) { e.code }
        "#);
        assert_eq!(other, "AddonsOwned");
        assert_eq!(take(), vec![
            "extra 111,222".to_string(),
            "global 333".into(),
            "client 76561198000000001 444".into(),
            "configure 5 20 1 60".into(),
            "mount 111 /w/111 0".into(),
        ]);
        unload_plugin("addons_a");
        assert_eq!(take(), vec!["reset".to_string()]);
        // The state is free again.
        let r = eval_in_context_string("addons_b", r#"
            globalThis.__s2pkg_addons.Addons.setExtraAddons([]); globalThis.__s2pkg_addons.Addons.owner();
        "#);
        assert_eq!(r, "addons_b");
        shutdown();
    }
}

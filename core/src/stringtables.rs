//! Network string tables (`@s2script/sdk/stringtables`) — the natives behind `StringTables` and the
//! `StringTable` handle. Engine-generic: every Source 2 game networks its precache lists, effect
//! names and the like through the same INetworkStringTableContainer; table and string names are
//! the caller's.
//!
//! A table handle is plain data, `{ id, name }`. The engine rebuilds its tables on a map change, so
//! nothing here keeps a pointer: every op takes the id AND the name the handle was made with, and
//! the shim re-finds the table by id and requires the name to still match. A handle kept across a
//! map change answers null / -1 / false instead of reaching a freed table.
//!
//! No thread-local state and no teardown: an added string or a user-data write is the engine's
//! state, the same as it would be from any other server code, and lives until the table is rebuilt.

use crate::net::{bytes_to_uint8array, js_bytes_arg};
use crate::v8host::{engine_ops_ref, set_native};
use std::ffi::CString;
use std::os::raw::{c_char, c_int};

/// Largest string or user-data blob a read will grow its buffer to. The engine bounds user data
/// far below this; the cap only keeps a corrupt length from allocating without limit.
const MAX_READ: usize = 1 << 20;

/// Read a variable-length result: `f(buf, cap)` returns the full length, or a negative "none".
/// Grows the buffer once when the first try was too small.
fn read_grown(first: usize, mut f: impl FnMut(*mut u8, c_int) -> c_int) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; first];
    let n = f(buf.as_mut_ptr(), buf.len() as c_int);
    if n < 0 {
        return None;
    }
    let n = n as usize;
    if n > buf.len() {
        if n > MAX_READ {
            return None;
        }
        buf = vec![0u8; n];
        let m = f(buf.as_mut_ptr(), buf.len() as c_int);
        if m < 0 || m as usize > buf.len() {
            return None;
        }
        buf.truncate(m as usize);
    } else {
        buf.truncate(n);
    }
    Some(buf)
}

fn c_arg(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<CString> {
    CString::new(v.to_rust_string_lossy(scope)).ok()
}

fn int_arg(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<c_int> {
    let n = v.number_value(scope)?;
    if !n.is_finite() || n.fract() != 0.0 || n < i32::MIN as f64 || n > i32::MAX as f64 {
        return None;
    }
    Some(n as c_int)
}

/// `__s2_st_count() -> number` — tables in the container; 0 when unavailable.
fn s2_st_count(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_count) else { return };
        rv.set_int32(f().max(0));
    }));
}

/// `__s2_st_find(name) -> number` — the table id, or -1.
fn s2_st_find(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_find) else { return };
        let Some(name) = c_arg(scope, args.get(0)) else { return };
        if name.as_bytes().is_empty() { return; }
        rv.set_int32(f(name.as_ptr()).max(-1));
    }));
}

/// `__s2_st_name(id) -> string | null` — the name of the table with that id right now.
fn s2_st_name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_name) else { return };
        let Some(id) = int_arg(scope, args.get(0)) else { return };
        let Some(bytes) = read_grown(256, |p, cap| f(id, p as *mut c_char, cap)) else { return };
        if let Some(s) = v8::String::new(scope, &String::from_utf8_lossy(&bytes)) {
            rv.set(s.into());
        }
    }));
}

/// `__s2_st_num_strings(id, name) -> number` — strings in the table, or -1 for a stale handle.
fn s2_st_num_strings(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_num_strings) else { return };
        let (Some(id), Some(name)) = (int_arg(scope, args.get(0)), c_arg(scope, args.get(1))) else { return };
        rv.set_int32(f(id, name.as_ptr()).max(-1));
    }));
}

/// `__s2_st_find_string(id, name, value) -> number` — the string's index, or -1.
fn s2_st_find_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_find_string) else { return };
        let (Some(id), Some(name), Some(value)) =
            (int_arg(scope, args.get(0)), c_arg(scope, args.get(1)), c_arg(scope, args.get(2))) else { return };
        rv.set_int32(f(id, name.as_ptr(), value.as_ptr()).max(-1));
    }));
}

/// `__s2_st_get_string(id, name, index) -> string | null`.
fn s2_st_get_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_get_string) else { return };
        let (Some(id), Some(name), Some(index)) =
            (int_arg(scope, args.get(0)), c_arg(scope, args.get(1)), int_arg(scope, args.get(2))) else { return };
        let Some(bytes) = read_grown(512, |p, cap| f(id, name.as_ptr(), index, p as *mut c_char, cap)) else { return };
        if let Some(s) = v8::String::new(scope, &String::from_utf8_lossy(&bytes)) {
            rv.set(s.into());
        }
    }));
}

/// `__s2_st_get_data(id, name, index) -> Uint8Array | null` — a copy of the string's user data.
/// An entry with no user data is an empty array; null is a stale handle or a bad index.
fn s2_st_get_data(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_get_data) else { return };
        let (Some(id), Some(name), Some(index)) =
            (int_arg(scope, args.get(0)), c_arg(scope, args.get(1)), int_arg(scope, args.get(2))) else { return };
        let Some(bytes) = read_grown(1024, |p, cap| f(id, name.as_ptr(), index, p, cap)) else { return };
        rv.set(bytes_to_uint8array(scope, &bytes));
    }));
}

/// `__s2_st_set_data(id, name, index, data) -> boolean` — data is a Uint8Array or a string (UTF-8).
fn s2_st_set_data(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_set_data) else { return };
        let (Some(id), Some(name), Some(index)) =
            (int_arg(scope, args.get(0)), c_arg(scope, args.get(1)), int_arg(scope, args.get(2))) else { return };
        let data = js_bytes_arg(scope, args.get(3));
        if data.len() > MAX_READ { return; }
        rv.set_bool(f(id, name.as_ptr(), index, data.as_ptr(), data.len() as c_int) == 1);
    }));
}

/// `__s2_st_add_string(id, name, value, data?) -> number` — the new (or existing) index, or -1.
/// Without `data` the entry keeps whatever user data it has (none for a new one).
fn s2_st_add_string(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        let Some(f) = engine_ops_ref().and_then(|o| o.stringtable_add_string) else { return };
        let (Some(id), Some(name), Some(value)) =
            (int_arg(scope, args.get(0)), c_arg(scope, args.get(1)), c_arg(scope, args.get(2))) else { return };
        if value.as_bytes().is_empty() { return; }
        let r = if args.get(3).is_null_or_undefined() {
            f(id, name.as_ptr(), value.as_ptr(), std::ptr::null(), -1)
        } else {
            let data = js_bytes_arg(scope, args.get(3));
            if data.len() > MAX_READ { return; }
            f(id, name.as_ptr(), value.as_ptr(), data.as_ptr(), data.len() as c_int)
        };
        rv.set_int32(r.max(-1));
    }));
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_st_count", s2_st_count);
    set_native(scope, global_obj, "__s2_st_find", s2_st_find);
    set_native(scope, global_obj, "__s2_st_name", s2_st_name);
    set_native(scope, global_obj, "__s2_st_num_strings", s2_st_num_strings);
    set_native(scope, global_obj, "__s2_st_find_string", s2_st_find_string);
    set_native(scope, global_obj, "__s2_st_get_string", s2_st_get_string);
    set_native(scope, global_obj, "__s2_st_get_data", s2_st_get_data);
    set_native(scope, global_obj, "__s2_st_set_data", s2_st_set_data);
    set_native(scope, global_obj, "__s2_st_add_string", s2_st_add_string);
}

#[cfg(test)]
mod tests {
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, mock_event_ops};
    use crate::v8host::{create_plugin_context, init, set_engine_ops, shutdown, S2EngineOps};
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};
    use std::sync::Mutex;

    /// One mock table: id 3, "EffectDispatch". Its strings, each with user data.
    static TABLE: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());

    fn reset_table() {
        let mut t = TABLE.lock().unwrap();
        t.clear();
        t.push(("ParticleEffect".into(), vec![1, 2, 3]));
        t.push(("x".repeat(700), Vec::new()));   // longer than the first read buffer
    }
    fn s(p: *const c_char) -> String { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
    fn live(id: c_int, name: *const c_char) -> bool { id == 3 && s(name) == "EffectDispatch" }
    fn copy_out(src: &[u8], out: *mut u8, cap: c_int) -> c_int {
        if src.len() <= cap as usize {
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), out, src.len()) };
        }
        src.len() as c_int
    }

    extern "C" fn m_count() -> c_int { 4 }   // ids 0..3; only 3 exists in the mock
    extern "C" fn m_find(name: *const c_char) -> c_int { if s(name) == "EffectDispatch" { 3 } else { -1 } }
    extern "C" fn m_name(id: c_int, out: *mut c_char, cap: c_int) -> c_int {
        if id != 3 { return -1; }
        copy_out(b"EffectDispatch", out as *mut u8, cap)
    }
    extern "C" fn m_num(id: c_int, name: *const c_char) -> c_int {
        if !live(id, name) { return -1; }
        TABLE.lock().unwrap().len() as c_int
    }
    extern "C" fn m_find_string(id: c_int, name: *const c_char, v: *const c_char) -> c_int {
        if !live(id, name) { return -1; }
        let v = s(v);
        TABLE.lock().unwrap().iter().position(|e| e.0 == v).map(|i| i as c_int).unwrap_or(-1)
    }
    extern "C" fn m_get_string(id: c_int, name: *const c_char, i: c_int, out: *mut c_char, cap: c_int) -> c_int {
        if !live(id, name) { return -1; }
        let t = TABLE.lock().unwrap();
        match t.get(i as usize) { Some(e) if i >= 0 => copy_out(e.0.as_bytes(), out as *mut u8, cap), _ => -1 }
    }
    extern "C" fn m_get_data(id: c_int, name: *const c_char, i: c_int, out: *mut u8, cap: c_int) -> c_int {
        if !live(id, name) { return -1; }
        let t = TABLE.lock().unwrap();
        match t.get(i as usize) { Some(e) if i >= 0 => copy_out(&e.1, out, cap), _ => -1 }
    }
    extern "C" fn m_set_data(id: c_int, name: *const c_char, i: c_int, d: *const u8, n: c_int) -> c_int {
        if !live(id, name) { return 0; }
        let mut t = TABLE.lock().unwrap();
        let Some(e) = t.get_mut(i as usize) else { return 0 };
        e.1 = unsafe { std::slice::from_raw_parts(d, n as usize) }.to_vec();
        1
    }
    extern "C" fn m_add(id: c_int, name: *const c_char, v: *const c_char, d: *const u8, n: c_int) -> c_int {
        if !live(id, name) { return -1; }
        let data = if n < 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(d, n as usize) }.to_vec() };
        let mut t = TABLE.lock().unwrap();
        t.push((s(v), data));
        (t.len() - 1) as c_int
    }

    fn ops() -> S2EngineOps {
        S2EngineOps {
            stringtable_count: Some(m_count),
            stringtable_find: Some(m_find),
            stringtable_name: Some(m_name),
            stringtable_num_strings: Some(m_num),
            stringtable_find_string: Some(m_find_string),
            stringtable_get_string: Some(m_get_string),
            stringtable_get_data: Some(m_get_data),
            stringtable_set_data: Some(m_set_data),
            stringtable_add_string: Some(m_add),
            ..mock_event_ops()
        }
    }

    /// find -> a { id, name } handle whose reads, writes and adds go through the ops with both the
    /// id and the name; a long string grows the read buffer; user data round-trips as bytes.
    #[test]
    fn string_table_handle_reads_writes_and_adds() {
        let _ = init(dummy_logger());
        reset_table();
        set_engine_ops(Some(ops()));
        create_plugin_context("stt");
        let r = eval_in_context_string("stt", r#"
            var ST = __s2pkg_stringtables.StringTables;
            var t = ST.find("EffectDispatch");
            var out = [ST.count(), t.id, t.name, t.count(), t.findString("ParticleEffect"),
                       t.findString("Nope"), t.getString(0), t.getString(1).length, t.getString(9),
                       Array.from(t.getData(0)).join("."), t.getData(1).length,
                       t.setData(1, new Uint8Array([7, 8])), Array.from(t.getData(1)).join("."),
                       t.setData(0, "hi"), Array.from(t.getData(0)).join("."),
                       t.addString("my/thing"), t.addString("withdata", new Uint8Array([9])),
                       Array.from(t.getData(3)).join("."), t.count(),
                       ST.find("Nope"), ST.get(3).name, ST.get(4), ST.all().length];
            out.map(String).join(",")
        "#);
        assert_eq!(r, "4,3,EffectDispatch,2,0,-1,ParticleEffect,700,null,1.2.3,0,true,7.8,true,104.105,2,3,9,4,null,EffectDispatch,null,1");
        shutdown();
    }

    /// A handle whose table was rebuilt under another name, and every native without ops, degrade.
    #[test]
    fn stale_handles_and_missing_ops_degrade() {
        let _ = init(dummy_logger());
        reset_table();
        set_engine_ops(Some(ops()));
        create_plugin_context("sts");
        let r = eval_in_context_string("sts", r#"
            var stale = new (Object.getPrototypeOf(__s2pkg_stringtables.StringTables.find("EffectDispatch")).constructor)(3, "Gone");
            [stale.count(), stale.getString(0), stale.getData(0), stale.setData(0, "x"),
             stale.findString("ParticleEffect"), stale.addString("y")].map(String).join(",")
        "#);
        assert_eq!(r, "-1,null,null,false,-1,-1");
        set_engine_ops(None);
        let r = eval_in_context_string("sts", r#"
            var ST = __s2pkg_stringtables.StringTables;
            [ST.count(), ST.find("EffectDispatch"), ST.get(3), ST.all().length].map(String).join(",")
        "#);
        assert_eq!(r, "0,null,null,0");
        shutdown();
    }
}

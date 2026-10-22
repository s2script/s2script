//! Raw memory access for `@s2script/sdk/unsafe` (performance plan, Stage 3): the natives behind
//! `Ptr`, the raw tier every struct view can be built on.
//!
//! UNSAFE by design, like the rest of that module. An address is whatever a plugin was handed (an
//! engine call's result, an entity's address, a field it dereferenced), and nothing here can prove
//! it still points at what the plugin thinks it does. The checks below only refuse addresses that
//! can never be valid user-space memory: non-integers, the null page, and anything at or above
//! 2^47 (the top of the x86-64 user half), which also keeps every address exact in an f64. A
//! wrong address that passes them faults like any Metamod plugin's would, and the crash reporter
//! names the plugin.
//!
//! Addresses travel as JS numbers. `Ptr` (core/js/prelude.js) wraps one so a plain number cannot
//! be passed by mistake, and adds the field offset before calling in, so these take a final
//! address. Scalar reads answer NaN for a refused address, the same shape as the entity fast path.
//!
//! Engine-generic: no game type, offset or class name appears here.

use crate::native::{native, Meta, S2Error};

/// The first address past the x86-64 user half. Every valid user-space address is below it.
const USER_TOP: f64 = 140_737_488_355_328.0; // 2^47
/// Addresses below this are the null page and never mapped.
const NULL_PAGE: f64 = 65_536.0;

/// Read kinds. They match the entity read kinds (`KIND_*` in core/src/entity.rs and `K` in the
/// prelude), so one JS table serves both.
const KIND_I32: i32 = 1;
const KIND_F32: i32 = 2;
const KIND_BOOL: i32 = 3;
const KIND_I8: i32 = 4;
const KIND_I16: i32 = 5;
const KIND_U8: i32 = 6;
const KIND_U16: i32 = 7;
const KIND_U32: i32 = 8;
const KIND_F64: i32 = 11;

/// `addr` as a pointer to `size` readable bytes, or `None` when it can never be valid.
pub(crate) fn checked(addr: f64, size: usize) -> Option<*mut u8> {
    if !(addr >= NULL_PAGE) || addr.fract() != 0.0 || addr + size as f64 > USER_TOP {
        return None;
    }
    Some(addr as u64 as usize as *mut u8)
}

fn size_of_kind(kind: i32) -> Option<usize> {
    Some(match kind {
        KIND_I8 | KIND_U8 | KIND_BOOL => 1,
        KIND_I16 | KIND_U16 => 2,
        KIND_I32 | KIND_U32 | KIND_F32 => 4,
        KIND_F64 => 8,
        _ => return None,
    })
}

/// Read a scalar of `kind` at `p`. Unaligned reads are allowed: engine structs are packed however
/// the compiler laid them out, and a plugin may compute any address.
///
/// SAFETY: `p` must be readable for the kind's size.
unsafe fn read_kind(p: *const u8, kind: i32) -> f64 {
    use std::ptr::read_unaligned as rd;
    match kind {
        KIND_I32 => rd(p as *const i32) as f64,
        KIND_U32 => rd(p as *const u32) as f64,
        KIND_F32 => rd(p as *const f32) as f64,
        KIND_BOOL => (rd(p) != 0) as i32 as f64,
        KIND_I8 => rd(p as *const i8) as f64,
        KIND_U8 => rd(p) as f64,
        KIND_I16 => rd(p as *const i16) as f64,
        KIND_U16 => rd(p as *const u16) as f64,
        KIND_F64 => rd(p as *const f64),
        _ => f64::NAN,
    }
}

/// Write `value` as `kind` at `p`, truncating integers the way the entity writes do.
///
/// SAFETY: `p` must be writable for the kind's size.
unsafe fn write_kind(p: *mut u8, kind: i32, value: f64) {
    use std::ptr::write_unaligned as wr;
    let int = value as i64;
    match kind {
        KIND_I32 => wr(p as *mut i32, int as i32),
        KIND_U32 => wr(p as *mut u32, int as u32),
        KIND_F32 => wr(p as *mut f32, value as f32),
        KIND_BOOL => wr(p, (value != 0.0 && !value.is_nan()) as u8),
        KIND_I8 => wr(p as *mut i8, int as i8),
        KIND_U8 => wr(p, int as u8),
        KIND_I16 => wr(p as *mut i16, int as i16),
        KIND_U16 => wr(p as *mut u16, int as u16),
        KIND_F64 => wr(p as *mut f64, value),
        _ => {}
    }
}

fn invalid(addr: f64) -> S2Error {
    S2Error::new("bad_address", format!("{addr} is not a user-space address"))
}

native! {
    /// The scalar of `kind` at `addr`, or NaN when `addr` can never be valid or `kind` is not a
    /// scalar kind. A float that really holds NaN also reads NaN.
    fast fn __s2_mem_read(addr: f64, kind: i32) -> f64 {
        crate::crash::breadcrumb::note_engine_op("mem_read");
        match size_of_kind(kind).and_then(|n| checked(addr, n)) {
            // SAFETY: unsafe tier — the plugin vouches for the address (module docs).
            Some(p) => unsafe { read_kind(p, kind) },
            None => f64::NAN,
        }
    }
}

native! {
    /// Write `value` as `kind` at `addr`. False when `addr` can never be valid or `kind` is not a
    /// scalar kind.
    fast fn __s2_mem_write(addr: f64, kind: i32, value: f64) -> bool {
        crate::crash::breadcrumb::note_engine_op("mem_write");
        match size_of_kind(kind).and_then(|n| checked(addr, n)) {
            Some(p) => {
                // SAFETY: unsafe tier — the plugin vouches for the address (module docs).
                unsafe { write_kind(p, kind, value) };
                true
            }
            None => false,
        }
    }
}

native! {
    /// The pointer stored at `addr`, as an address. 0 when it holds null; NaN when `addr` can never
    /// be valid or the stored value can't be an address either (in the null page, or outside user
    /// space), so the bad hop fails here rather than at a later read.
    fast fn __s2_mem_read_ptr(addr: f64) -> f64 {
        crate::crash::breadcrumb::note_engine_op("mem_read");
        let Some(p) = checked(addr, 8) else { return f64::NAN };
        // SAFETY: unsafe tier — the plugin vouches for the address (module docs).
        let v = unsafe { std::ptr::read_unaligned(p as *const u64) };
        if v == 0 {
            0.0
        } else if (v as f64) >= NULL_PAGE && (v as f64) < USER_TOP {
            v as f64
        } else {
            f64::NAN
        }
    }
}

native! {
    /// Store `target` (an address, or 0 for null) as a pointer at `addr`.
    fn __s2_mem_write_ptr(addr: f64, target: f64) -> Result<(), S2Error> {
        let p = checked(addr, 8).ok_or_else(|| invalid(addr))?;
        if target != 0.0 && checked(target, 0).is_none() {
            return Err(invalid(target));
        }
        crate::crash::breadcrumb::note_engine_op("mem_write");
        // SAFETY: unsafe tier — the plugin vouches for the address (module docs).
        unsafe { std::ptr::write_unaligned(p as *mut u64, target as u64) };
        Ok(())
    }
}

native! {
    /// The 64-bit integer at `addr` as a decimal string (signed when `signed`). A JS number cannot
    /// hold every 64-bit value, so this never answers a number.
    fn __s2_mem_read_wide(addr: f64, signed: bool) -> Result<String, S2Error> {
        let p = checked(addr, 8).ok_or_else(|| invalid(addr))?;
        crate::crash::breadcrumb::note_engine_op("mem_read");
        // SAFETY: unsafe tier — the plugin vouches for the address (module docs).
        let v = unsafe { std::ptr::read_unaligned(p as *const u64) };
        Ok(if signed { (v as i64).to_string() } else { v.to_string() })
    }
}

native! {
    /// The NUL-terminated string at `addr`, reading at most `max_len` bytes. Invalid UTF-8 is
    /// replaced, not refused.
    fn __s2_mem_read_string(addr: f64, max_len: i32) -> Result<String, S2Error> {
        let n = max_len.max(0) as usize;
        let p = checked(addr, n).ok_or_else(|| invalid(addr))?;
        crate::crash::breadcrumb::note_engine_op("mem_read");
        // SAFETY: unsafe tier — the plugin vouches for `max_len` readable bytes (module docs).
        let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, n) };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

native! {
    /// Write `s` NUL-terminated into the `max_len`-byte buffer at `addr`, truncating on a UTF-8
    /// boundary so the buffer always ends in a NUL. False when nothing fits (`max_len` < 1).
    fn __s2_mem_write_string(addr: f64, max_len: i32, s: String) -> Result<bool, S2Error> {
        if max_len < 1 {
            return Ok(false);
        }
        let n = max_len as usize;
        let p = checked(addr, n).ok_or_else(|| invalid(addr))?;
        let mut len = s.len().min(n - 1);
        while !s.is_char_boundary(len) {
            len -= 1;
        }
        crate::crash::breadcrumb::note_engine_op("mem_write");
        // SAFETY: unsafe tier — the plugin vouches for `max_len` writable bytes (module docs).
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), p, len);
            *p.add(len) = 0;
        }
        Ok(true)
    }
}

native! {
    /// `count` consecutive f32s at `addr` (a vector, an angle, a colour as floats).
    fn __s2_mem_read_floats(addr: f64, count: i32) -> Result<Vec<f64>, S2Error> {
        let n = count.clamp(0, 64) as usize;
        let p = checked(addr, n * 4).ok_or_else(|| invalid(addr))?;
        crate::crash::breadcrumb::note_engine_op("mem_read");
        // SAFETY: unsafe tier — the plugin vouches for the address (module docs).
        Ok((0..n).map(|i| unsafe { std::ptr::read_unaligned((p as *const f32).add(i)) } as f64).collect())
    }
}

native! {
    /// The live address of the entity `(index, id)`, resolved through the books like every entity
    /// read; 0 when the reference is stale. The address is only good until the entity is deleted,
    /// and nothing tracks that for you: this is the unsafe tier.
    fast fn __s2_ent_address(index: f64, id: f64) -> f64 {
        let p = crate::entity::resolve_for_address(index, id);
        if p.is_null() { 0.0 } else { p as usize as f64 }
    }
}

/// This feature's natives, for `install` and the generated `.d.ts`.
pub(crate) const NATIVES: &[&Meta] = &[
    &__s2_mem_read::META,
    &__s2_mem_write::META,
    &__s2_mem_read_ptr::META,
    &__s2_mem_write_ptr::META,
    &__s2_mem_read_wide::META,
    &__s2_mem_read_string::META,
    &__s2_mem_write_string::META,
    &__s2_mem_read_floats::META,
    &__s2_ent_address::META,
];

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    crate::native::install(scope, global_obj, NATIVES);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_that_can_never_be_valid_are_refused() {
        assert!(checked(0.0, 4).is_none(), "null");
        assert!(checked(4096.0, 4).is_none(), "null page");
        assert!(checked(65_536.5, 4).is_none(), "fraction");
        assert!(checked(f64::NAN, 4).is_none(), "NaN");
        assert!(checked(-65_536.0, 4).is_none(), "negative");
        assert!(checked(USER_TOP - 2.0, 4).is_none(), "straddles the top of user space");
        assert!(checked(USER_TOP - 4.0, 4).is_some());
        assert!(checked(65_536.0, 4).is_some());
    }

    #[test]
    fn scalar_reads_and_writes_round_trip_unaligned() {
        let mut buf = vec![0u8; 32];
        let base = buf.as_mut_ptr() as usize as f64 + 1.0; // deliberately misaligned
        for (kind, v) in [(KIND_I32, -7.0), (KIND_U32, 4_000_000_000.0), (KIND_F32, 1.5), (KIND_I8, -2.0),
                          (KIND_U8, 200.0), (KIND_I16, -300.0), (KIND_U16, 60_000.0), (KIND_F64, 0.1), (KIND_BOOL, 1.0)] {
            assert!(__s2_mem_write::call(base, kind, v));
            assert_eq!(__s2_mem_read::call(base, kind), v, "kind {kind}");
        }
        assert!(__s2_mem_write::call(base, KIND_U8, 300.0));
        assert_eq!(__s2_mem_read::call(base, KIND_U8), 44.0, "integers truncate like entity writes");
        assert!(__s2_mem_read::call(base, 99).is_nan(), "unknown kind");
        assert!(!__s2_mem_write::call(base, 99, 1.0));
        assert!(__s2_mem_read::call(0.0, KIND_I32).is_nan());
        drop(buf);
    }

    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, mock_event_ops};
    use crate::v8host::{create_plugin_context, init, set_engine_ops, shutdown, S2EngineOps};
    use std::os::raw::{c_char, c_int};

    /// `Ptr` end to end over memory this test owns: typed reads and writes, strings, floats,
    /// 64-bit values as strings, pointer fields followed by `deref` and by the `*Via` chain reads
    /// generated views use, and the refusals.
    #[test]
    fn ptr_reads_writes_and_follows_pointers_over_real_memory() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("raw");
        let mut inner = vec![0u8; 16];
        inner[4..8].copy_from_slice(&77i32.to_ne_bytes());
        let mut buf = vec![0u8; 128];
        buf[0..4].copy_from_slice(&(-5i32).to_ne_bytes());
        buf[4..8].copy_from_slice(&2.5f32.to_ne_bytes());
        buf[8] = 1;
        buf[16..24].copy_from_slice(&(inner.as_ptr() as u64).to_ne_bytes());
        buf[24..32].copy_from_slice(&u64::MAX.to_ne_bytes());
        buf[32..37].copy_from_slice(b"hello");
        for (i, f) in [1.0f32, 2.0, 3.0].iter().enumerate() {
            buf[40 + i * 4..44 + i * 4].copy_from_slice(&f.to_ne_bytes());
        }
        // offset 56: zero (a null pointer field); offset 88: a value in the null page
        buf[88..96].copy_from_slice(&0x1000u64.to_ne_bytes());
        let addr = buf.as_ptr() as usize;
        let out = eval_in_context_string("raw", &format!(r#"
            (function () {{
              const {{ Ptr }} = __s2require("@s2script/sdk/unsafe");
              const p = Ptr.from({addr});
              const code = (f) => {{ try {{ f(); return "ok"; }} catch (e) {{ return e.code || e.name; }} }};
              const r = [];
              r.push(p.readInt32(0), p.readFloat32(4), p.readBool(8));
              r.push(p.deref(16).readInt32(4), p.readInt32Via([16], 4), String(p.readInt32Via([56], 0)));
              r.push(p.readUInt64(24), p.readInt64(24));
              r.push(p.readString(32, 16), p.readFloats(40, 3).join("/"));
              p.writeInt32(64, 123456); p.writeFloat32(68, -1.5); p.writeString(72, 4, "abcdef");
              r.push(p.readInt32(64), p.readFloat32(68), p.readString(72, 8));
              p.writePtr(80, p.add(64)); r.push(p.deref(80).readInt32(0), p.deref(56).isNull());
              r.push(code(() => Ptr.NULL.readInt32(0)), code(() => Ptr.from(123)), code(() => p.deref(24)), code(() => p.deref(88)));
              r.push(p.add(4).equals(Ptr.from({addr} + 4)), String(Ptr.from(0) === Ptr.NULL));
              r.push(code(() => {{ Ptr.prototype.readInt32 = null; }}), typeof p.readInt32);
              return r.join(",");
            }})()
        "#));
        assert_eq!(
            out,
            "-5,2.5,true,77,77,null,18446744073709551615,-1,hello,1/2/3,123456,-1.5,abc,123456,true,\
             bad_address,bad_address,bad_address,bad_address,true,true,ok,function"
        );
        assert_eq!(i32::from_ne_bytes(buf[64..68].try_into().unwrap()), 123456, "writes land in real memory");
        assert_eq!(&buf[72..76], b"abc\0");
        drop(inner);
        drop(buf);
        shutdown();
    }

    /// Stage 3 measurement: a `Ptr` field read, raw and through a prototype-accessor view (the
    /// shape generated struct views have). `cargo test --release -p s2script-core ptr_read_bench --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ptr_read_bench() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("bench");
        let buf = vec![7u8; 64];
        let addr = buf.as_ptr() as usize;
        let out = eval_in_context_string("bench", &format!(r#"
            (function () {{
              const {{ Ptr }} = __s2require("@s2script/sdk/unsafe");
              const p = Ptr.from({addr});
              const proto = {{}};
              Object.defineProperty(proto, "health", {{ get: function () {{ return this.ref.readInt32(8); }} }});
              const view = Object.create(proto); view.ref = p;
              const N = 2000000;
              function run(f) {{
                let best = Infinity, sink = 0;
                for (let r = 0; r < 5; r++) {{
                  const t0 = Number(__s2_hrtime_ns ? __s2_hrtime_ns() : Date.now() * 1e6);
                  for (let i = 0; i < N; i++) sink += f();
                  const t1 = Number(__s2_hrtime_ns ? __s2_hrtime_ns() : Date.now() * 1e6);
                  best = Math.min(best, (t1 - t0) / N);
                }}
                return best.toFixed(1) + (sink === -1 ? "" : "");
              }}
              return "raw " + run(() => p.readInt32(8)) + " ns, view " + run(() => view.health) + " ns";
            }})()
        "#));
        println!("Ptr read: {out}");
        drop(buf);
        shutdown();
    }

    thread_local! {
        static LAST_GP: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
        static RET: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    extern "C" fn fake_resolve(
        _kind: *const c_char, _module: *const c_char, _pattern: *const c_char,
        _resolve: *const c_char, _class_name: *const c_char, _vtable_index: c_int,
        _validate_json: *const c_char, _reason_out: *mut c_char, _reason_cap: c_int,
    ) -> c_int { 3 }

    extern "C" fn fake_invoke(
        _call_id: c_int, _ent_index: c_int, _ent_serial: c_int, _subobj_off: c_int,
        gp: *const u64, _gp_kind: *const u8, gp_count: c_int,
        _fp: *const f64, _fp_count: c_int,
        _strs: *const *const c_char, _vecs: *const f32,
        _ret_kind: c_int, ret_out: *mut u64,
    ) -> c_int {
        let args = (0..gp_count.max(0) as usize).map(|i| unsafe { *gp.add(i) }).collect();
        LAST_GP.with(|g| *g.borrow_mut() = args);
        unsafe { *ret_out = RET.with(|r| r.get()) };
        1
    }

    /// A declared call with `ptr` arguments and a `ptr` return: a `Ptr` crosses as its address,
    /// null as a null pointer, anything else is refused, and the 64-bit result comes back as a `Ptr`
    /// (`Ptr.NULL` for null, null for an address outside user space).
    #[test]
    fn engine_calls_take_and_return_ptr() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps {
            engine_call_resolve: Some(fake_resolve),
            engine_call_invoke: Some(fake_invoke),
            ..mock_event_ops()
        }));
        create_plugin_context("ptrcall");
        crate::gamedata_calls::register_plugin(
            "ptrcall",
            r#"{"calls":{"Find":{"receiver":{"kind":"none"},
                "target":{"kind":"signature","name":"F","module":"libserver.so","pattern":"55 48","resolve":"direct"},
                "args":["ptr","int","ptr"],"returns":"ptr"}}}"#,
        );
        assert_eq!(crate::gamedata_calls::status("ptrcall", "Find"), "available");
        let mut target = vec![0u8; 8];
        target[0..4].copy_from_slice(&41i32.to_ne_bytes());
        let addr = target.as_ptr() as usize as u64;
        RET.with(|r| r.set(addr));
        let call = |src: &str| eval_in_context_string("ptrcall", &format!(r#"
            (function () {{
              const {{ Ptr, Engine }} = __s2require("@s2script/sdk/unsafe");
              const find = Engine.call("Find");
              const r = {src};
              return r === null ? "null" : r instanceof Ptr ? (r.isNull() ? "NULL" : String(r.readInt32(0))) : String(r);
            }})()
        "#));
        assert_eq!(call(&format!("find(Ptr.from({addr}), 9, null)")), "41");
        assert_eq!(LAST_GP.with(|g| g.borrow().clone()), vec![addr, 9, 0]);
        LAST_GP.with(|g| g.borrow_mut().clear());
        assert_eq!(call(&format!("find({addr}, 9, null)")), "null", "a bare number is not a Ptr");
        assert!(LAST_GP.with(|g| g.borrow().is_empty()), "and the engine is never called");
        RET.with(|r| r.set(0));
        assert_eq!(call("find(null, 1, null)"), "NULL");
        RET.with(|r| r.set(u64::MAX));
        assert_eq!(call("find(null, 1, null)"), "null");
        drop(target);
        set_engine_ops(None);
        shutdown();
    }
}

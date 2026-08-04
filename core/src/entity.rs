//! Engine-generic entity access, in two halves.
//!
//! **The pure half (top of file):** raw entity-field access — V8-free pointer-arithmetic read/write
//! plus guards, deliberately engine-free so it is unit-testable without an isolate. Its tests run
//! with no V8 at all, and that property is preserved.
//!
//! **The V8 half (bottom of file):** the `__s2_ent_*` / `__s2_entity_*` natives over those helpers,
//! the entity lifecycle mux, its dispatch, and the teardown registration. These moved here from
//! `v8host.rs` under the core-stabilization program — the pure helpers above were their single
//! largest dependency (30 call sites), so keeping them apart split one feature across two files.
//!
//! EntityRef MARSHALLING stays in `v8host`: `ent_op_serial` (decode `{index,id}` to a books-validated
//! `(index, serial)`) and `build_entity_ref` (mint one back) are used by the trace and dispatch
//! natives too, so they are shared host primitives rather than this feature's private helpers.

/// Read an i32 at `base + offset`. Returns 0 on a null base or negative offset (degrade-safe).
pub fn read_i32(base: *const u8, offset: i32) -> i32 {
    if base.is_null() || offset < 0 {
        return 0;
    }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *const i32) }
}

/// Write an i32 at `base + offset`. No-op on a null base or negative offset (degrade-safe).
pub fn write_i32(base: *mut u8, offset: i32, value: i32) {
    if base.is_null() || offset < 0 {
        return;
    }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *mut i32) = value; }
}

/// CS2 `CEntityHandle` index/serial bit-split (NUM_ENT_ENTRY_BITS). Confirmed by the Slice-5A spike;
/// see docs/superpowers/specs/2026-07-01-slice-5a-spike-findings.md.
// TODO(gamedata): migrate to a regenerable gamedata file with the other engine-struct facts.
pub const HANDLE_ENTRY_BITS: u32 = 15; // <-- SET FROM SPIKE FINDINGS

/// Read a u32 at `base + offset`. Returns 0 on a null base or negative offset (degrade-safe).
pub fn read_u32(base: *const u8, offset: i32) -> u32 {
    if base.is_null() || offset < 0 {
        return 0;
    }
    // SAFETY: caller supplies a live entity pointer + a fixed in-struct offset.
    unsafe { *(base.add(offset as usize) as *const u32) }
}

/// Read a pointer field at `base + offset`. Returns null on a null base or negative offset.
pub fn read_ptr(base: *const u8, offset: i32) -> *const u8 {
    if base.is_null() || offset < 0 {
        return std::ptr::null();
    }
    // SAFETY: caller supplies a live entity pointer + a fixed in-struct offset.
    unsafe { *(base.add(offset as usize) as *const *const u8) }
}

/// Read a u64 at `base + offset`. 0 on null base / negative offset.
pub fn read_u64(base: *const u8, offset: i32) -> u64 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const u64) }
}
/// Read an i64 at `base + offset`. 0 on null base / negative offset.
pub fn read_i64(base: *const u8, offset: i32) -> i64 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const i64) }
}
/// Read an f64 at `base + offset`. 0.0 on null base / negative offset.
pub fn read_f64(base: *const u8, offset: i32) -> f64 {
    if base.is_null() || offset < 0 { return 0.0; }
    unsafe { *(base.add(offset as usize) as *const f64) }
}
/// Read a NUL-terminated string of at most `max_len` bytes at `base + offset` (an inline `char[N]`
/// buffer), UTF-8-lossy → an owned `String` (a COPY; the pointer never leaves core). Empty on null
/// base / negative offset / non-positive `max_len`.
pub fn read_string(base: *const u8, offset: i32, max_len: i32) -> String {
    if base.is_null() || offset < 0 || max_len <= 0 { return String::new(); }
    let start = unsafe { base.add(offset as usize) };
    let max = max_len as usize;
    let mut len = 0usize;
    unsafe {
        while len < max && *start.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(start, len)).into_owned()
    }
}

/// Write `bytes` as a bounded, NUL-terminated string into an inline `char[max_len]` buffer at
/// `base + offset`. Copies `min(bytes.len(), max_len - 1)` bytes then writes a single NUL terminator —
/// so it NEVER writes past `base + offset + max_len - 1` (one byte is always reserved for the NUL).
/// No-op on a null base, negative offset, or `max_len <= 0` (degrade-safe). The pointer stays in core;
/// the caller resolves it serial-gated and discards it within the native.
pub fn write_string(base: *mut u8, offset: i32, max_len: i32, bytes: &[u8]) {
    if base.is_null() || offset < 0 || max_len <= 0 { return; }
    let start = unsafe { base.add(offset as usize) };
    let cap = max_len as usize;                       // cap >= 1 (max_len > 0)
    let n = core::cmp::min(bytes.len(), cap - 1);     // reserve 1 byte for the NUL terminator
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset; `n < cap`
    // and the NUL lands at `n < max_len`, so no byte past `offset + max_len - 1` is touched.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), start, n);
        *start.add(n) = 0;
    }
}

/// Read an f32 at `base + offset`. 0.0 on null base / negative offset (degrade-safe).
pub fn read_f32(base: *const u8, offset: i32) -> f32 {
    if base.is_null() || offset < 0 { return 0.0; }
    unsafe { *(base.add(offset as usize) as *const f32) }
}
/// Write an f32 at `base + offset`. No-op on null base / negative offset.
pub fn write_f32(base: *mut u8, offset: i32, value: f32) {
    if base.is_null() || offset < 0 { return; }
    unsafe { *(base.add(offset as usize) as *mut f32) = value; }
}
/// Read a bool (a single byte; any non-zero is true). false on null / negative offset.
pub fn read_bool(base: *const u8, offset: i32) -> bool {
    if base.is_null() || offset < 0 { return false; }
    unsafe { *base.add(offset as usize) != 0 }
}
/// Write a bool as a single byte (1/0). No-op on null / negative offset.
pub fn write_bool(base: *mut u8, offset: i32, value: bool) {
    if base.is_null() || offset < 0 { return; }
    unsafe { *base.add(offset as usize) = if value { 1 } else { 0 }; }
}
/// Read an i8, sign-extended to i32. 0 on null / negative offset.
pub fn read_i8(base: *const u8, offset: i32) -> i32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const i8) as i32 }
}
/// Read an i16, sign-extended to i32. 0 on null / negative offset.
pub fn read_i16(base: *const u8, offset: i32) -> i32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const i16) as i32 }
}
/// Read a u8, zero-extended to u32. 0 on null / negative offset.
pub fn read_u8(base: *const u8, offset: i32) -> u32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *base.add(offset as usize) as u32 }
}
/// Read a u16, zero-extended to u32. 0 on null / negative offset.
pub fn read_u16(base: *const u8, offset: i32) -> u32 {
    if base.is_null() || offset < 0 { return 0; }
    unsafe { *(base.add(offset as usize) as *const u16) as u32 }
}

/// Write an i8 (truncated from an i32). No-op on null / negative offset.
pub fn write_i8(base: *mut u8, offset: i32, value: i32) {
    if base.is_null() || offset < 0 { return; }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *mut i8) = value as i8; }
}
/// Write an i16 (truncated from an i32). No-op on null / negative offset.
pub fn write_i16(base: *mut u8, offset: i32, value: i32) {
    if base.is_null() || offset < 0 { return; }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *mut i16) = value as i16; }
}
/// Write a u8 (truncated from an i32; e.g. 300 -> 44). No-op on null / negative offset.
pub fn write_u8(base: *mut u8, offset: i32, value: i32) {
    if base.is_null() || offset < 0 { return; }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *base.add(offset as usize) = value as u8; }
}
/// Write a u16 (truncated from an i32). No-op on null / negative offset.
pub fn write_u16(base: *mut u8, offset: i32, value: i32) {
    if base.is_null() || offset < 0 { return; }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *mut u16) = value as u16; }
}
/// Write a u32. No-op on null / negative offset.
pub fn write_u32(base: *mut u8, offset: i32, value: u32) {
    if base.is_null() || offset < 0 { return; }
    // SAFETY: caller supplies a live entity pointer + a schema-resolved in-object offset.
    unsafe { *(base.add(offset as usize) as *mut u32) = value; }
}

/// Decode a `CEntityHandle` uint32 into `(index, serial)` using the CS2 bit-split.
pub fn decode_handle(handle: u32) -> (i32, i32) {
    let index = (handle & ((1u32 << HANDLE_ENTRY_BITS) - 1)) as i32;
    let serial = (handle >> HANDLE_ENTRY_BITS) as i32;
    (index, serial)
}

/// True iff a captured `ref_serial` still matches the entity system's `current_serial` for that
/// index. Both must be valid (`>= 0`); an empty slot reports `-1` and never matches.
pub fn resolve(current_serial: i32, ref_serial: i32) -> bool {
    current_serial >= 0 && ref_serial >= 0 && current_serial == ref_serial
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct Fake { pad: [u8; 8], health: i32, more: i32 }

    #[test]
    fn write_then_read_roundtrips_at_offset() {
        let mut f = Fake { pad: [0; 8], health: 100, more: 7 };
        let base = &mut f as *mut Fake as *mut u8;
        let off = 8; // offset of `health`
        assert_eq!(read_i32(base as *const u8, off), 100);
        write_i32(base, off, 1234);
        assert_eq!(read_i32(base as *const u8, off), 1234);
        assert_eq!(f.more, 7, "adjacent field untouched");
    }

    #[test]
    fn guards_null_and_negative_offset() {
        assert_eq!(read_i32(std::ptr::null(), 8), 0);
        assert_eq!(read_i32(std::ptr::null(), -4), 0);
        // write to null / negative offset must not crash and must be a no-op:
        write_i32(std::ptr::null_mut(), 8, 1);
        let mut v: i32 = 5;
        write_i32(&mut v as *mut i32 as *mut u8, -4, 9);
        assert_eq!(v, 5);
    }

    #[test]
    fn decode_handle_is_inverse_of_encode() {
        // BITS-agnostic proof the bit-math is a correct inverse (the exact BITS value is
        // validated live in the gate; here we prove decode∘encode == identity for that split).
        let bits = HANDLE_ENTRY_BITS;
        let encode = |index: u32, serial: u32| (serial << bits) | (index & ((1 << bits) - 1));
        for &(i, s) in &[(0u32, 0u32), (1, 1), (64, 3), ((1 << bits) - 1, 7)] {
            let (di, ds) = decode_handle(encode(i, s));
            assert_eq!(di, i as i32, "index round-trips");
            assert_eq!(ds, s as i32, "serial round-trips");
        }
    }

    #[test]
    fn resolve_matches_only_equal_nonneg_serials() {
        assert!(resolve(5, 5));
        assert!(!resolve(5, 6), "mismatch (reused slot) is invalid");
        assert!(!resolve(-1, -1), "empty slot (-1) is never valid");
        assert!(!resolve(-1, 5));
        assert!(!resolve(5, -1));
    }

    #[test]
    fn read_u32_and_read_ptr_guard_null_and_negative() {
        assert_eq!(read_u32(std::ptr::null(), 4), 0);
        assert_eq!(read_u32(std::ptr::null(), -4), 0);
        assert!(read_ptr(std::ptr::null(), 8).is_null());
        assert!(read_ptr(&0u8 as *const u8, -8).is_null());
    }

    #[test]
    fn read_u32_reads_at_offset() {
        #[repr(C)]
        struct Fake { pad: [u8; 4], handle: u32 }
        let f = Fake { pad: [0; 4], handle: 0xDEAD_BEEF };
        let base = &f as *const Fake as *const u8;
        assert_eq!(read_u32(base, 4), 0xDEAD_BEEF);
    }

    #[test]
    fn read_ptr_reads_a_pointer_field() {
        let target: u8 = 42;
        #[repr(C)]
        struct Fake { pad: [u8; 8], p: *const u8 }
        let f = Fake { pad: [0; 8], p: &target as *const u8 };
        let base = &f as *const Fake as *const u8;
        let got = read_ptr(base, 8);
        assert!(!got.is_null());
        assert_eq!(unsafe { *got }, 42);
    }

    #[test]
    fn read_write_f32_roundtrips() {
        #[repr(C)]
        struct Fake { pad: [u8; 4], f: f32 }
        let mut x = Fake { pad: [0; 4], f: 0.0 };
        let base = &mut x as *mut Fake as *mut u8;
        write_f32(base, 4, 12.5);
        assert_eq!(read_f32(base as *const u8, 4), 12.5);
    }

    #[test]
    fn read_write_bool_roundtrips_and_reads_nonzero_as_true() {
        #[repr(C)]
        struct Fake { pad: [u8; 4], b: u8 }
        let mut x = Fake { pad: [0; 4], b: 0 };
        let base = &mut x as *mut Fake as *mut u8;
        assert_eq!(read_bool(base as *const u8, 4), false);
        write_bool(base, 4, true);
        assert_eq!(read_bool(base as *const u8, 4), true);
        assert_eq!(x.b, 1);
        // any non-zero byte reads as true:
        x.b = 0x7F;
        assert_eq!(read_bool(base as *const u8, 4), true);
    }

    #[test]
    fn read_i8_i16_sign_extend() {
        #[repr(C)]
        struct Fake { i8v: i8, pad: u8, i16v: i16 }
        let x = Fake { i8v: -1, pad: 0, i16v: -1000 };
        let base = &x as *const Fake as *const u8;
        assert_eq!(read_i8(base, 0), -1);       // 0xFF -> -1 (sign-extended to i32)
        assert_eq!(read_i16(base, 2), -1000);   // sign-extended
    }

    #[test]
    fn read_u8_u16_zero_extend() {
        #[repr(C)]
        struct Fake { u8v: u8, pad: u8, u16v: u16 }
        let x = Fake { u8v: 0xFF, pad: 0, u16v: 0xFFFF };
        let base = &x as *const Fake as *const u8;
        assert_eq!(read_u8(base, 0), 255);      // zero-extended, not -1
        assert_eq!(read_u16(base, 2), 65535);
    }

    #[test]
    fn narrow_int_writes_roundtrip_truncate_and_signextend() {
        #[repr(C)]
        struct Fake { u8v: u8, i8v: i8, u16v: u16, i16v: i16, u32v: u32 }
        let mut f = Fake { u8v: 0, i8v: 0, u16v: 0, i16v: 0, u32v: 0 };
        let base = &mut f as *mut Fake as *mut u8;
        // u8 round-trip (offset 0)
        write_u8(base, 0, 200);
        assert_eq!(read_u8(base as *const u8, 0), 200);
        // u8 truncation: 300 & 0xFF == 44
        write_u8(base, 0, 300);
        assert_eq!(read_u8(base as *const u8, 0), 44);
        // i8 negative round-trip (offset 1; sign-extended on read)
        write_i8(base, 1, -5);
        assert_eq!(read_i8(base as *const u8, 1), -5);
        // u16 round-trip (offset 2)
        write_u16(base, 2, 60000);
        assert_eq!(read_u16(base as *const u8, 2), 60000);
        // i16 negative round-trip (offset 4)
        write_i16(base, 4, -1000);
        assert_eq!(read_i16(base as *const u8, 4), -1000);
        // u32 round-trip beyond i32::MAX (offset 8)
        write_u32(base, 8, 0xDEAD_BEEF);
        assert_eq!(read_u32(base as *const u8, 8), 0xDEAD_BEEF);
    }

    #[test]
    fn narrow_int_writes_guard_null_and_negative_offset() {
        // writes to null / negative offset must not crash and must be no-ops:
        write_i8(std::ptr::null_mut(), 0, 1);
        write_i16(std::ptr::null_mut(), 0, 1);
        write_u8(std::ptr::null_mut(), 0, 1);
        write_u16(std::ptr::null_mut(), 0, 1);
        write_u32(std::ptr::null_mut(), 0, 1);
        let mut v: u32 = 5;
        write_u32(&mut v as *mut u32 as *mut u8, -4, 9);
        assert_eq!(v, 5);
    }

    #[test]
    fn typed_reads_guard_null_and_negative_offset() {
        assert_eq!(read_f32(std::ptr::null(), 4), 0.0);
        assert_eq!(read_f32(std::ptr::null(), -4), 0.0);
        assert_eq!(read_bool(std::ptr::null(), 4), false);
        assert_eq!(read_i8(std::ptr::null(), 0), 0);
        assert_eq!(read_u16(std::ptr::null(), 2), 0);
        // writes to null / negative offset must not crash + must be a no-op:
        write_f32(std::ptr::null_mut(), 4, 1.0);
        write_bool(std::ptr::null_mut(), 4, true);
        let mut v: f32 = 5.0;
        write_f32(&mut v as *mut f32 as *mut u8, -4, 9.0);
        assert_eq!(v, 5.0);
    }

    #[test]
    fn read_u64_i64_f64_roundtrip() {
        #[repr(C)]
        struct Fake { pad: [u8; 8], u: u64, i: i64, f: f64 }
        let x = Fake { pad: [0; 8], u: 76561198000000000, i: -9000000000, f: 6.5 }; // u > 2^53
        let base = &x as *const Fake as *const u8;
        assert_eq!(read_u64(base, 8), 76561198000000000);
        assert_eq!(read_i64(base, 16), -9000000000);
        assert_eq!(read_f64(base, 24), 6.5);
    }

    #[test]
    fn read_string_nul_terminated_and_bounded() {
        // "hi\0" then junk within a char[8] buffer.
        let buf: [u8; 8] = [b'h', b'i', 0, b'X', b'Y', 0, 0, 0];
        let base = buf.as_ptr();
        assert_eq!(read_string(base, 0, 8), "hi");            // stops at the first NUL
        assert_eq!(read_string(base, 3, 8), "XY");            // reads from an offset, stops at NUL
        // max_len bounds the scan even without a NUL:
        let full: [u8; 4] = [b'a', b'b', b'c', b'd'];         // no NUL
        assert_eq!(read_string(full.as_ptr(), 0, 4), "abcd");
        assert_eq!(read_string(full.as_ptr(), 0, 2), "ab");   // bounded by max_len
    }

    #[test]
    fn sixtyfour_bit_and_string_guard_null_and_negative_offset() {
        assert_eq!(read_u64(std::ptr::null(), 8), 0);
        assert_eq!(read_i64(std::ptr::null(), -8), 0);
        assert_eq!(read_f64(std::ptr::null(), 8), 0.0);
        assert_eq!(read_string(std::ptr::null(), 0, 8), "");
        let b: [u8; 2] = [b'x', 0];
        assert_eq!(read_string(b.as_ptr(), -1, 8), "");       // negative offset
        assert_eq!(read_string(b.as_ptr(), 0, 0), "");        // non-positive max_len
    }

    #[test]
    fn write_string_writes_nul_terminates_and_truncates() {
        // char[8] prefilled with 0xFF so a write's extent + the terminator are exactly visible.
        let mut buf: [u8; 8] = [0xFF; 8];
        write_string(buf.as_mut_ptr(), 0, 8, b"hi");
        assert_eq!(&buf[0..3], b"hi\0");                       // 'h','i', then the NUL
        assert_eq!(buf[3], 0xFF, "no write past the string + its NUL");

        // Write into a char[5] window at offset 3 → bytes 3,4,5 = 'a','b','\0'; the prefix untouched.
        let mut b2: [u8; 8] = [0xFF; 8];
        write_string(b2.as_mut_ptr(), 3, 5, b"ab");
        assert_eq!(&b2[3..6], b"ab\0");
        assert_eq!(b2[0], 0xFF, "bytes before the offset untouched");

        // Truncation: a string longer than max_len-1 is cut and STILL NUL-terminated at max_len-1;
        // the byte at max_len is never touched (the bound).
        let mut b3: [u8; 8] = [0x11; 8];
        write_string(b3.as_mut_ptr(), 0, 4, b"abcdef");       // char[4] → 'a','b','c','\0'
        assert_eq!(&b3[0..4], b"abc\0");
        assert_eq!(b3[4], 0x11, "never writes past max_len-1 (the bound)");

        // char[1] → exactly one NUL (empty string), nothing past it.
        let mut b4: [u8; 4] = [0x22; 4];
        write_string(b4.as_mut_ptr(), 0, 1, b"xyz");
        assert_eq!(b4[0], 0);
        assert_eq!(b4[1], 0x22, "char[1] writes exactly one NUL, nothing past it");
    }

    #[test]
    fn write_string_guards_null_and_bad_bounds() {
        // null base / negative offset / non-positive max_len are all no-ops (no crash, no write):
        write_string(std::ptr::null_mut(), 0, 8, b"hi");
        let mut buf: [u8; 4] = [0x33; 4];
        write_string(buf.as_mut_ptr(), -1, 4, b"hi");         // negative offset → no-op
        write_string(buf.as_mut_ptr(), 0, 0, b"hi");          // non-positive max_len → no-op
        write_string(buf.as_mut_ptr(), 0, -4, b"hi");         // negative max_len → no-op
        assert_eq!(buf, [0x33; 4], "all guarded calls left the buffer untouched");
    }
}

// ---------------------------------------------------------------------------
// The V8 surface — see the module header for why it lives beside the pure half.
// ---------------------------------------------------------------------------

use crate::v8host::{
    build_entity_ref, current_plugin, engine_ops, ent_op_serial, fan_out, js_ent_id,
    schema_offset_cached, set_native, subscribe_into, Delivery, Instrument,
};
use std::os::raw::{c_int, c_void};

// Field-type kind codes — a JS<->core contract, mirrored in INJECTED_STD_PRELUDE's `K`. Keep in lockstep.
const KIND_I32: i64 = 1;
const KIND_F32: i64 = 2;
const KIND_BOOL: i64 = 3;
const KIND_I8: i64 = 4;
const KIND_I16: i64 = 5;
const KIND_U8: i64 = 6;
const KIND_U16: i64 = 7;
const KIND_U32: i64 = 8;
const KIND_U64: i64 = 9;
const KIND_I64: i64 = 10;
const KIND_F64: i64 = 11;

// ^ These MUST live in the same module as the natives that `match` on them. When they were left
// behind in `v8host.rs`, every `KIND_*` arm here silently became an irrefutable BINDING pattern
// rather than a constant comparison — so the first arm matched everything and the rest were dead.
// It compiles; only the `unreachable pattern` / `should have a snake case name` warnings show it.

thread_local! {
    /// Entity lifecycle listeners: `Entity.onCreate/onSpawn/onDelete(className, handler)` mux, keyed
    /// `"<kind>\0<className>"` (kind = "create"/"spawn"/"delete"; className "*" = all). Notify-only,
    /// dispatched SYNCHRONOUSLY from the shim's IEntityListener callback. The listener is registered
    /// for the process lifetime on the first-ever subscribe, so there is no engine-op on an emptied
    /// name. `remove_by_owner` on unload; reset on shutdown so a re-init starts empty.
    static ENTITY_MUX: std::cell::RefCell<crate::channels::Channels<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::channels::Channels::new());
}

/// Native `__s2_ent_ref_valid(index, id) -> boolean`.
/// True iff the books say (index, id) is live (and the slot re-validates in the identity chunk).
fn s2_ent_ref_valid(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        rv.set_bool(!entity_resolve_ptr(index, id).is_null());
    }));
}

/// Native `__s2_ent_ref_read(index, serial, offset, kind) -> number|boolean|null`. Serial-gated typed read.
fn s2_ent_ref_read(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::crash::breadcrumb::note_engine_op("ent_ref_read");
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(3).integer_value(scope).unwrap_or(0);
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }               // invalid → null (already set)
        let p = ent as *const u8;
        match kind {
            KIND_I32  => rv.set_int32(crate::entity::read_i32(p, off)),
            KIND_F32  => rv.set_double(crate::entity::read_f32(p, off) as f64),
            KIND_BOOL => rv.set_bool(crate::entity::read_bool(p, off)),
            KIND_I8   => rv.set_int32(crate::entity::read_i8(p, off)),
            KIND_I16  => rv.set_int32(crate::entity::read_i16(p, off)),
            KIND_U8   => rv.set_double(crate::entity::read_u8(p, off) as f64),
            KIND_U16  => rv.set_double(crate::entity::read_u16(p, off) as f64),
            KIND_U32  => rv.set_double(crate::entity::read_u32(p, off) as f64),
            KIND_U64  => { let bi = v8::BigInt::new_from_u64(scope, crate::entity::read_u64(p, off)); rv.set(bi.into()); }
            KIND_I64  => { let bi = v8::BigInt::new_from_i64(scope, crate::entity::read_i64(p, off)); rv.set(bi.into()); }
            KIND_F64  => rv.set_double(crate::entity::read_f64(p, off)),
            _         => { /* unknown kind → leave null */ }
        }
    }));
}

/// Native `__s2_ent_ref_write(index, serial, offset, kind, value) -> boolean`. Serial-gated typed write
/// (I32/F32/BOOL + narrow ints I8/I16/U8/U16/U32; 64-bit writes deferred → false).
fn s2_ent_ref_write(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::crash::breadcrumb::note_engine_op("ent_ref_write");
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(3).integer_value(scope).unwrap_or(0);
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }               // invalid → false (already set)
        match kind {
            KIND_I32  => crate::entity::write_i32(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_F32  => crate::entity::write_f32(ent, off, args.get(4).number_value(scope).unwrap_or(0.0) as f32),
            KIND_BOOL => crate::entity::write_bool(ent, off, args.get(4).boolean_value(scope)),
            KIND_I8   => crate::entity::write_i8(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_I16  => crate::entity::write_i16(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_U8   => crate::entity::write_u8(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_U16  => crate::entity::write_u16(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as i32),
            KIND_U32  => crate::entity::write_u32(ent, off, args.get(4).integer_value(scope).unwrap_or(0) as u32),
            _         => return,                   // unknown / deferred write kind (64-bit) → false
        }
        rv.set_bool(true);
    }));
}

/// Native `__s2_ent_ref_read_string(index, serial, offset, maxLen) -> string|null`. Serial-gated;
/// returns a COPIED string (the pointer never crosses to JS). null on a stale ref.
fn s2_ent_ref_read_string(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let max_len = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }                 // invalid → null (already set)
        let s = crate::entity::read_string(ent as *const u8, off, max_len);
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_ent_ref_write_string(index, serial, offset, maxLen, str) -> boolean`. Serial-gated
/// mirror of `read_string`: writes a bounded, NUL-terminated string into an inline `char[maxLen]` field
/// (truncated to `maxLen-1` bytes + always NUL-terminated; never past the bound). The raw pointer is
/// resolved + used entirely in core and never crosses to JS. false on a stale/invalid ref.
fn s2_ent_ref_write_string(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let max_len = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        let s = args.get(4).to_rust_string_lossy(scope);
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }                 // invalid → false (already set)
        crate::entity::write_string(ent, off, max_len, s.as_bytes());
        rv.set_bool(true);
    }));
}

/// Native `__s2_ent_ref_read_floats(index, serial, offset, count) -> number[] | null`. Serial-gated;
/// reads `count` (1..=4) contiguous f32s into a JS array (a COPY; the pointer never crosses to JS).
/// null on a stale/invalid ref or an out-of-range count.
fn s2_ent_ref_read_floats(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let count = args.get(3).integer_value(scope).unwrap_or(0) as i32;
        if count <= 0 || count > 4 { return; }          // only small fixed vectors (Vector..Vector4D)
        if off < 0 { return; }                           // schema-miss sentinel (-1) → null (not a partial read)
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }                     // stale/invalid → null (already set)
        let p = ent as *const u8;
        let arr = v8::Array::new(scope, count);
        for i in 0..count {
            let v = crate::entity::read_f32(p, off + i * 4) as f64;
            let num = v8::Number::new(scope, v);
            arr.set_index(scope, i as u32, num.into());
        }
        rv.set(arr.into());
    }));
}

/// Native `__s2_ent_ref_read_floats_chain(index, serial, ptrOffs, finalOff, count) -> number[] | null`.
/// Follows a chain of pointer derefs (each i32 offset in the `ptrOffs` JS array), then reads `count` (1..=4)
/// contiguous f32s at `finalOff` into a COPIED JS array. Serial-gated at the root entity; each hop null-checked;
/// the raw intermediate pointers never cross to JS. null on a stale root / a null hop / a bad chain/offset/count.
fn s2_ent_ref_read_floats_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let final_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let count = args.get(4).integer_value(scope).unwrap_or(0) as i32;
        if count <= 0 || count > 4 || final_off < 0 { return; }
        // args[2] must be an array of pointer offsets:
        let Ok(chain) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return; };
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }                     // stale/invalid root → null (already set)
        let mut p = ent as *const u8;
        for i in 0..chain.length() {
            let off = chain.get_index(scope, i).and_then(|v| v.integer_value(scope)).unwrap_or(-1) as i32;
            if off < 0 { return; }                       // bad offset in the chain → null
            p = crate::entity::read_ptr(p, off);
            if p.is_null() { return; }                   // a null hop (broken chain) → null
        }
        let out = v8::Array::new(scope, count);
        for i in 0..count {
            let v = crate::entity::read_f32(p, final_off + i * 4) as f64;
            let num = v8::Number::new(scope, v);
            out.set_index(scope, i as u32, num.into());
        }
        rv.set(out.into());
    }));
}

/// Native `__s2_ent_ref_read_chain(index, serial, pathOffs, finalOff, kind) -> value | null`. Follows a chain
/// of pointer derefs (each i32 offset in `pathOffs`), then reads a SCALAR of `kind` at `finalOff`. Serial-gated
/// at the root; each hop null-checked; the raw intermediate pointers never cross to JS. Vectors use
/// __s2_ent_ref_read_floats_chain; handles = read KIND_U32 here then __s2_handle_decode in JS.
fn s2_ent_ref_read_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::crash::breadcrumb::note_engine_op("ent_ref_read_chain");
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let final_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(4).integer_value(scope).unwrap_or(0);
        if final_off < 0 { return; }
        let Ok(path) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return; };
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }
        let mut p = ent as *const u8;
        for i in 0..path.length() {
            let off = path.get_index(scope, i).and_then(|v| v.integer_value(scope)).unwrap_or(-1) as i32;
            if off < 0 { return; }
            p = crate::entity::read_ptr(p, off);
            if p.is_null() { return; }
        }
        let off = final_off;
        match kind {
            KIND_I32  => rv.set_int32(crate::entity::read_i32(p, off)),
            KIND_F32  => rv.set_double(crate::entity::read_f32(p, off) as f64),
            KIND_BOOL => rv.set_bool(crate::entity::read_bool(p, off)),
            KIND_I8   => rv.set_int32(crate::entity::read_i8(p, off)),
            KIND_I16  => rv.set_int32(crate::entity::read_i16(p, off)),
            KIND_U8   => rv.set_double(crate::entity::read_u8(p, off) as f64),
            KIND_U16  => rv.set_double(crate::entity::read_u16(p, off) as f64),
            KIND_U32  => rv.set_double(crate::entity::read_u32(p, off) as f64),
            KIND_U64  => { let bi = v8::BigInt::new_from_u64(scope, crate::entity::read_u64(p, off)); rv.set(bi.into()); }
            KIND_I64  => { let bi = v8::BigInt::new_from_i64(scope, crate::entity::read_i64(p, off)); rv.set(bi.into()); }
            KIND_F64  => rv.set_double(crate::entity::read_f64(p, off)),
            _ => { }   // unknown kind → leave null
        }
    }));
}

/// Native `__s2_ent_ref_write_chain(index, serial, pathOffs, finalOff, kind, value) -> boolean`.
/// Write mirror of `s2_ent_ref_read_chain`: serial-gates the root, derefs each i32 offset in `pathOffs`
/// (each hop null-checked; raw intermediate pointers never cross to JS), then writes a SCALAR of `kind`
/// at `finalOff`. Returns false on a stale ref, an unresolved hop, a bad `finalOff`, or an unknown/64-bit
/// kind. Does NOT call notifyStateChanged (the caller decides). The final ptr is only ever written in-core.
/// Callers: the CS2 fire gate (m_flNextAttack, F32) + flag-clearing on a pointer sub-object (i32/u32/u8).
fn s2_ent_ref_write_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::crash::breadcrumb::note_engine_op("ent_ref_write_chain");
        rv.set_bool(false);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let final_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let kind = args.get(4).integer_value(scope).unwrap_or(0);
        if final_off < 0 { return; }
        let Ok(path) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return; };
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }
        let mut p = ent as *const u8;
        for i in 0..path.length() {
            let off = path.get_index(scope, i).and_then(|v| v.integer_value(scope)).unwrap_or(-1) as i32;
            if off < 0 { return; }
            p = crate::entity::read_ptr(p, off);
            if p.is_null() { return; }
        }
        let dst = p as *mut u8;
        let off = final_off;
        match kind {
            KIND_I32  => crate::entity::write_i32(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_F32  => crate::entity::write_f32(dst, off, args.get(5).number_value(scope).unwrap_or(0.0) as f32),
            KIND_BOOL => crate::entity::write_bool(dst, off, args.get(5).boolean_value(scope)),
            KIND_I8   => crate::entity::write_i8(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_I16  => crate::entity::write_i16(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_U8   => crate::entity::write_u8(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_U16  => crate::entity::write_u16(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as i32),
            KIND_U32  => crate::entity::write_u32(dst, off, args.get(5).integer_value(scope).unwrap_or(0) as u32),
            _ => return,   // unknown / 64-bit kind → false (already set)
        }
        rv.set_bool(true);
    }));
}

/// Native `__s2_ent_ref_state_changed(index, serial, offset)`.
/// Resolves (index, serial) then calls `ent_state_changed` engine-op. No-op on stale ref / no ops.
fn s2_ent_ref_state_changed(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as c_int;
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }               // invalid → no-op
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.ent_state_changed else { return };
        func(ent as *mut c_void, off);
    }));
}

/// Native `__s2_ent_id_for_index(index) -> number`. The books id for a slot-derived
/// index (Player.fromSlot / Pawn.forSlot), or 0 when the books say not-live. Books
/// only — no engine memory. Replaces the retired `__s2_ent_current_serial` idiom.
fn s2_ent_id_for_index(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_double(0.0);
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        if let Some((id, _)) = crate::entity_live::lookup(index) { rv.set_double(id as f64); }
    }));
}

/// Deliver an entity lifecycle event to the `Entity.on{Create,Spawn,Delete}` subscribers. Called from
/// ffi.rs's `s2script_core_dispatch_entity_event` (the shim's IEntityListener callback). `kind` is
/// "create"/"spawn"/"delete"; the mux is keyed `"<kind>\0<className>"` with a `"<kind>\0*"` wildcard.
/// Notify-only. Mirrors `dispatch_client_event`: snapshot (release the mux borrow), `try_borrow_mut`
/// re-entrancy guard, per-sub `is_live` + context clone + HandleScope/ContextScope/TryCatch + WARN-on-throw.
/// The entity crosses as a packed handle → a serial-gated EntityRef (null if stale/free — the exact-(-1)
/// + resolve-null discipline of `dispatch_output`); className is passed as a 2nd arg (always valid).
///
/// `dispatch_entity_event` = **bookkeeping** (`entity_live::on_created`/`on_spawned`/`on_deleted`,
/// in `ffi.rs`) + this JS fan-out. A replayed `"create"` would RESURRECT a since-deleted entity in
/// the books (`on_created` is an unconditional insert), which is why the shim queues
/// `replay_entity_event` and never the dispatch entry (contract §6.1).
pub(crate) fn dispatch_entity_event(kind: &str, class_name: &str, handle: i32) -> Delivery {
    replay_entity_event(kind, class_name, handle)
}

/// Native `__s2_entity_create(className) -> EntityRef | null`. Over the `entity_create` op
/// (`UTIL_CreateEntityByName`, sig-resolved shim-side). The op returns a packed `CEntityHandle`
/// (`ToInt()`); the raw `CBaseEntity*` never crosses to JS — the handle is decoded (pure bit-math)
/// and re-validated live (`entity_resolve_ptr`) before building a serial-gated `EntityRef`, mirroring
/// the `s2_trace` hit-entity pattern. Degrades to `null` with no op / a 0 handle / a same-frame stale
/// decode (every in-isolate test hits this path).
fn s2_entity_create(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let name = args.get(0).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_create) {
            let handle = func(cname.as_ptr());
            if handle != 0 {
                let (index, serial) = crate::entity::decode_handle(handle as u32);
                // The create listener fed the books synchronously via the ffi entry — adoption is the proof.
                if let Some(id) = crate::entity_live::adopt(index, serial) {
                    rv.set(build_entity_ref(scope, index, id));
                }
            }
        }
    }));
}

/// Native `__s2_entity_find_by_class(className) -> EntityRef[]`. Over the `entity_find_by_class` op
/// (the shim iterates the entity-identity list, comparing each `CEntityIdentity::m_designerName`).
/// Returns serial-gated EntityRefs — each (index, serial) is re-validated via `entity_resolve_ptr`
/// (like `s2_entity_create`) before building the ref; the raw pointer never crosses to JS.
/// Degrades to an empty array with no op / a null className. The out-buffer is bounded at 1024.
fn s2_entity_find_by_class(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let empty = v8::Array::new(scope, 0);
        rv.set(empty.into());
        let name = args.get(0).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = engine_ops();
        let Some(func) = ops.and_then(|o| o.entity_find_by_class) else { return };
        const CAP: usize = 1024;
        let mut idxs = vec![0i32; CAP];
        let mut sers = vec![0i32; CAP];
        let total = func(cname.as_ptr(), idxs.as_mut_ptr(), sers.as_mut_ptr(), CAP as i32);
        let n = (total.max(0) as usize).min(CAP);
        let arr = v8::Array::new(scope, 0);
        let mut w: u32 = 0;
        for i in 0..n {
            let (index, serial) = (idxs[i], sers[i]);
            if let Some(id) = crate::entity_live::adopt(index, serial) {
                let r = build_entity_ref(scope, index, id);
                arr.set_index(scope, w, r);
                w += 1;
            }
        }
        rv.set(arr.into());
    }));
}

/// Native `__s2_entity_spawn(index, serial) -> boolean`. Serial-gated `DispatchSpawn`. Degrades to
/// `false` with no op / a stale ref.
fn s2_entity_spawn(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_spawn) { rv.set_bool(func(index, serial) != 0); }
    }));
}

/// Native `__s2_ent_set_model(index, serial, modelName) -> boolean`. Serial-gated; over the
/// `entity_set_model` op (`CBaseEntity::SetModel`, sig-resolved shim-side). Gives a runtime entity a
/// model + its collision — a runtime `trigger_multiple` needs this for a physics volume that fires
/// touch. Degrades to `false` with no op / a stale ref / a NUL in the name. Never throws.
fn s2_ent_set_model(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let name = args.get(2).to_rust_string_lossy(scope);
        let cname = match std::ffi::CString::new(name) { Ok(c) => c, Err(_) => return };
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_set_model) {
            rv.set_bool(func(index, serial, cname.as_ptr()) != 0);
        }
    }));
}

/// Native `__s2_ent_set_gravity_scale(index, serial, scale) -> boolean`. Serial-gated; over the
/// `entity_set_gravity_scale` op (`CBaseEntity::SetGravityScale`, sig-resolved shim-side).
///
/// This exists INSTEAD of a schema write and that is the point: the engine setter early-returns when
/// the value is unchanged and maintains a second field (`m_flActualGravityScale`), so a plugin that
/// writes `m_flGravityScale` directly observes no effect. A non-finite scale is rejected here rather
/// than passed on. Degrades to `false` with no op / a stale ref. Never throws.
fn s2_ent_set_gravity_scale(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let scale = args.get(2).number_value(scope).unwrap_or(f64::NAN);
        if !scale.is_finite() { return }
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_set_gravity_scale) {
            rv.set_bool(func(index, serial, scale as f32) != 0);
        }
    }));
}

/// Native `__s2_ent_apply_abs_velocity_impulse(index, serial, [x,y,z]) -> boolean`. Serial-gated;
/// over the `entity_apply_abs_velocity_impulse` op (`CBaseEntity::ApplyAbsVelocityImpulse`).
///
/// Additive and physics-aware, unlike writing `m_vecAbsVelocity` (which skips the partition/physics
/// update). A non-array or non-finite component is rejected here — the engine reads all three floats
/// through the pointer, so a NaN would reach it. A zero impulse is a legal no-op the callee
/// early-outs on. Degrades to `false` with no op / a stale ref. Never throws.
fn s2_ent_apply_abs_velocity_impulse(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let Some(v) = read_vec3_opt(scope, args.get(2)) else { return };
        if !v.iter().all(|c| c.is_finite()) { return }
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_apply_abs_velocity_impulse) {
            rv.set_bool(func(index, serial, v.as_ptr()) != 0);
        }
    }));
}

/// Native `__s2_ent_stop_sound(index, serial, soundName) -> boolean`. Serial-gated; over the
/// `entity_stop_sound` op (`CBaseEntity::StopSound`). The counterpart to `sound_emit`. A NUL in the
/// name degrades to `false` rather than truncating. Never throws.
fn s2_ent_stop_sound(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let name = args.get(2).to_rust_string_lossy(scope);
        let Ok(cname) = std::ffi::CString::new(name) else { return };
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_stop_sound) {
            rv.set_bool(func(index, serial, cname.as_ptr()) != 0);
        }
    }));
}

/// Native `__s2_ent_set_body_group_by_name(index, serial, name, group) -> boolean`. Serial-gated;
/// over the `entity_set_body_group_by_name` op (`CBaseModelEntity::SetBodyGroupByName`).
///
/// The schema route is unavailable: `m_bodyGroupChoices` is a `CUtlOrderedMap`, not a scalar. `group`
/// is 32-bit callee-side, so it is range-checked here rather than silently truncated. Degrades to
/// `false` with no op / a stale ref / a NUL in the name. Never throws.
fn s2_ent_set_body_group_by_name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let name = args.get(2).to_rust_string_lossy(scope);
        let Ok(cname) = std::ffi::CString::new(name) else { return };
        let group = args.get(3).integer_value(scope).unwrap_or(0);
        let Ok(group) = i32::try_from(group) else { return };
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_set_body_group_by_name) {
            rv.set_bool(func(index, serial, cname.as_ptr(), group) != 0);
        }
    }));
}

/// Native `__s2_ent_set_model_scale(index, serial, scale) -> boolean`. Serial-gated; over the
/// `entity_set_model_scale` op (`CBaseModelEntity::SetModelScale`).
///
/// The argument shape is confirmed by disassembly (float in xmm0); the NAME is a catalogue
/// attribution the function body does not by itself prove — see the gamedata comment. Calling it is
/// memory-safe either way. A non-finite scale is rejected. Degrades to `false` with no op / a stale
/// ref. Never throws.
fn s2_ent_set_model_scale(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let scale = args.get(2).number_value(scope).unwrap_or(f64::NAN);
        if !scale.is_finite() { return }
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_set_model_scale) {
            rv.set_bool(func(index, serial, scale as f32) != 0);
        }
    }));
}

/// Native `__s2_entity_teleport(index, serial, originArr|null, anglesArr|null, velArr|null) -> boolean`.
/// Each array arg is independently optional (a non-3-element/non-array value degrades to a null pointer
/// for that component, matching the shim's nullable `Vector*`/`QAngle*`/`Vector*` ABI). Degrades to
/// `false` with no op / a stale ref.
fn s2_entity_teleport(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let origin = read_vec3_opt(scope, args.get(2));
        let angles = read_vec3_opt(scope, args.get(3));
        let vel    = read_vec3_opt(scope, args.get(4));
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_teleport) {
            let op = origin.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            let ap = angles.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            let vp = vel.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
            rv.set_bool(func(index, serial, op, ap, vp) != 0);
        }
    }));
}

/// Native `__s2_entity_remove(index, serial) -> boolean`. Serial-gated `UTIL_Remove`. Degrades to
/// `false` with no op / a stale ref.
fn s2_entity_remove(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_remove) { rv.set_bool(func(index, serial) != 0); }
    }));
}

/// Native `__s2_entity_fire_input(index, serial, input, value, actIdx, actSerial, callerIdx,
/// callerSerial, delay) -> boolean`. Over the `entity_fire_input` engine op (`AddEntityIOEvent`, the
/// game's own input-firing path, sig-resolved shim-side). `actIdx`/`callerIdx` < 0 = no
/// activator/caller (the shim passes null). Degrades to `false` with no op / a stale target ref.
fn s2_entity_fire_input(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let input = args.get(2).to_rust_string_lossy(scope);
        let value = args.get(3).to_rust_string_lossy(scope);
        // E1: optional activator/caller (args 4/5, 6/7) — a JS -1 / id-0 / translation miss all
        // collapse to (-1, -1) = "no activator/caller" (the shim passes null).
        let (act_idx, act_serial) = ent_op_serial(scope, args.get(4), args.get(5)).unwrap_or((-1, -1));
        let (caller_idx, caller_serial) = ent_op_serial(scope, args.get(6), args.get(7)).unwrap_or((-1, -1));
        let delay = args.get(8).number_value(scope).unwrap_or(0.0) as f32;
        let Ok(input_c) = std::ffi::CString::new(input) else { return };
        let Ok(value_c) = std::ffi::CString::new(value) else { return };
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_fire_input) {
            rv.set_bool(func(
                index, serial, input_c.as_ptr(), value_c.as_ptr(),
                act_idx, act_serial, caller_idx, caller_serial, delay,
            ) != 0);
        }
    }));
}

/// Native `__s2_entity_spawn_kv(index, serial, keys[], types[], values[]) -> boolean`. Over the
/// `entity_spawn_kv` op (DispatchSpawn with a shim-built CEntityKeyValues). All three arrays must be
/// same-length; keys/values are strings (interior NUL -> false), types are ints. Degrades to false
/// with no op / stale serial / malformed args (every in-isolate test).
fn s2_entity_spawn_kv(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let keys_arr = match v8::Local::<v8::Array>::try_from(args.get(2)) { Ok(a) => a, Err(_) => return };
        let types_arr = match v8::Local::<v8::Array>::try_from(args.get(3)) { Ok(a) => a, Err(_) => return };
        let vals_arr = match v8::Local::<v8::Array>::try_from(args.get(4)) { Ok(a) => a, Err(_) => return };
        let n = keys_arr.length();
        if types_arr.length() != n || vals_arr.length() != n { return; }
        let mut keys_c: Vec<std::ffi::CString> = Vec::with_capacity(n as usize);
        let mut vals_c: Vec<std::ffi::CString> = Vec::with_capacity(n as usize);
        let mut types_v: Vec<c_int> = Vec::with_capacity(n as usize);
        for i in 0..n {
            let k = match keys_arr.get_index(scope, i) { Some(v) => v.to_rust_string_lossy(scope), None => return };
            let val = match vals_arr.get_index(scope, i) { Some(v) => v.to_rust_string_lossy(scope), None => return };
            let t = match types_arr.get_index(scope, i) { Some(v) => v.integer_value(scope).unwrap_or(-1) as i32, None => return };
            if !(0..=3).contains(&t) { return; }
            let kc = match std::ffi::CString::new(k) { Ok(c) => c, Err(_) => return };
            let vc = match std::ffi::CString::new(val) { Ok(c) => c, Err(_) => return };
            // BYTE-length guard (the true choke point): the JS prelude caps UTF-16 .length at 1024,
            // but CKV3Arena's AddPage() aborts the WHOLE process on a string whose UTF-8 BYTE length
            // exceeds ~2KB — and a BMP char (CJK, U+0800..U+FFFF) is 3 UTF-8 bytes/code-unit, so 1024
            // code units can be ~3KB. Re-check the exact UTF-8 byte length here (free — the CString is
            // built) and fail the WHOLE map closed (no partial spawn) BEFORE any engine call.
            if kc.as_bytes().len() > 1024 || vc.as_bytes().len() > 1024 { return; }
            keys_c.push(kc); vals_c.push(vc); types_v.push(t);
        }
        let key_ptrs: Vec<*const std::os::raw::c_char> = keys_c.iter().map(|c| c.as_ptr()).collect();
        let val_ptrs: Vec<*const std::os::raw::c_char> = vals_c.iter().map(|c| c.as_ptr()).collect();
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_spawn_kv) {
            rv.set_bool(func(index, serial, n as c_int, key_ptrs.as_ptr(), types_v.as_ptr(), val_ptrs.as_ptr()) != 0);
        }
    }));
}

/// Native `__s2_entity_listener_on(kind, className, handler)`. Subscribes a JS fn to the entity
/// lifecycle mux (entity-listeners slice), keyed `"<kind>\0<className>"`. On the FIRST-EVER subscribe
/// (the mux was empty), calls the `entity_listener_install` engine op so the shim lazily registers its
/// IEntityListener (zero cost when no plugin subscribes). Degrade-never-crash: no op → the subscribe
/// still records, the engine just never delivers.
fn s2_entity_listener_on(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let kind = args.get(0).to_rust_string_lossy(scope);
        let class_name = args.get(1).to_rust_string_lossy(scope);
        let key = format!("{}\0{}", kind, class_name);
        // WHOLE-STORE emptiness, not per-channel: the IEntityListener is registered once for the
        // process. Sampled BEFORE subscribing.
        let first_ever = ENTITY_MUX.with(|m| m.borrow().is_empty());
        let Some((sub_id, _)) = subscribe_into(scope, &args, &ENTITY_MUX, &key, 2) else { return };
        if first_ever {
            if let Some(func) = engine_ops().and_then(|o| o.entity_listener_install) {
                let _ = func();
            }
        }
        rv.set(v8::Number::new(scope, sub_id as f64).into());
    }));
}

/// Native `__s2_entity_listener_off(kind, className)`. Drops the CURRENT plugin's subs for the exact
/// `"<kind>\0<className>"` key (best-effort, mirrors `s2_output_unsubscribe`). The IEntityListener stays
/// installed (unload/reload cleanup runs via `remove_by_owner`); this is available as a primitive.
fn s2_entity_listener_off(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let kind = args.get(0).to_rust_string_lossy(scope);
        let class_name = args.get(1).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let key = format!("{}\0{}", kind, class_name);
        ENTITY_MUX.with(|m| { m.borrow_mut().remove_by_owner_on(&key, &owner); });
    }));
}

/// Native `__s2_entity_subobj_vcall(index, serial, subObjOffset, vtableIndex, argIndex, argSerial)
/// -> boolean`. Calls a `.text`-validated vtable slot on the sub-object at `subObjOffset` (e.g.
/// ItemServices' `RemoveWeapons`/`DropActivePlayerWeapon`), optionally passing a second
/// serial-gated entity arg (`argIndex < 0` = no arg, e.g. no active weapon to pass). Degrades to
/// `false` with no op / a stale root or arg ref / an unresolved sub-object / an out-of-`.text`
/// vtable slot (shim-side guard).
fn s2_entity_subobj_vcall(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as i32;
        let vtable_index = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        // E1: optional second entity arg (args 4/5) — a JS -1 / id-0 / translation miss → (-1, -1) = "no arg".
        let (arg_idx, arg_serial) = ent_op_serial(scope, args.get(4), args.get(5)).unwrap_or((-1, -1));
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_subobj_vcall) {
            rv.set_bool(func(index, serial, off, vtable_index, arg_idx, arg_serial) != 0);
        }
    }));
}

/// Native `__s2_entity_read_handle_vector(index, serial, ptrOffs, vectorOff, maxCount) ->
/// EntityRef[]`. Follows the `ptrOffs` pointer-deref chain from the root entity (e.g. to a
/// WeaponServices sub-object), then reads a `CUtlVector<CHandle>` at `vectorOff` (size@+0,
/// elements@+8, shim-side) — each packed handle is decoded and `entity_resolve_ptr`-validated
/// before becoming a serial-gated `EntityRef` (raw pointers never cross to JS; `maxCount`-capped,
/// itself clamped to `[0, 256]`). Degrades to `[]` with no op / a stale root / an unresolved chain.
fn s2_entity_read_handle_vector(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let arr = v8::Array::new(scope, 0);
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { rv.set(arr.into()); return };
        let ptr_offs = read_int_array(scope, args.get(2));      // Vec<i32>, [] if not an array
        let vector_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let max_count = (args.get(4).integer_value(scope).unwrap_or(0) as i32).clamp(0, 256);
        let ops = engine_ops();
        if let Some(func) = ops.and_then(|o| o.entity_read_handle_vector) {
            let mut out = vec![0i32; max_count as usize];
            let n = func(index, serial, ptr_offs.as_ptr(), ptr_offs.len() as i32, vector_off, max_count, out.as_mut_ptr());
            let mut w = 0u32;
            for k in 0..(n.max(0) as usize).min(max_count as usize) {
                let (i, s) = crate::entity::decode_handle(out[k] as u32);
                if let Some(id) = crate::entity_live::adopt(i, s) {
                    let er = build_entity_ref(scope, i, id);
                    arr.set_index(scope, w, er);
                    w += 1;
                }
            }
        }
        rv.set(arr.into());
    }));
}

/// Native `__s2_entity_name(index, serial) -> string | null`. Reads CEntityIdentity::m_name via the
/// `entity_name` op; copies the C string now. null = stale/invalid/no-ops; "" = entity has no targetname.
fn s2_entity_name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.entity_name else { return };
        let ptr = func(index, serial);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_entity_target(index, id) -> string | null`. Reads `CBaseEntity::m_target` (a
/// `CUtlSymbolLarge`) directly from the entity's OWN instance memory — unlike `m_name`, which lives on
/// the identity and is read through the shim's dedicated `entity_name` op, `m_target` has no such op,
/// so this resolves its offset via the SAME live `__s2_schema_offset` cache the plugin-declared-call
/// `receiver.via` hop and `DamageInfo` use (schema-resolved, never baked — spec §10), then follows the
/// already books/serial-gated entity pointer itself. A `CUtlSymbolLarge` is a single pointer-sized
/// field (`const char* m_pString`); its own `String()` accessor falls back to `""` when that pointer
/// is null, which is exactly the convention followed here. null = stale/invalid ref, unresolved
/// offset, or no ops (mirrors `s2_entity_name`'s null = stale/invalid/no-ops contract); "" = the
/// entity has no target.
fn s2_entity_target(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let id = js_ent_id(scope, args.get(1));
        let ent = entity_resolve_ptr(index, id);
        if ent.is_null() { return; }               // invalid → null (already set)
        let off = schema_offset_cached("CBaseEntity", "m_target");
        if off < 0 { return; }                      // unresolved offset → null (already set)
        let sym_ptr = crate::entity::read_u64(ent as *const u8, off) as usize as *const std::os::raw::c_char;
        if sym_ptr.is_null() {
            if let Some(js) = v8::String::new(scope, "") { rv.set(js.into()); }
            return;
        }
        let s = unsafe { std::ffi::CStr::from_ptr(sym_ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}

/// Native `__s2_ent_identity_flags(index, id) -> number | null`. CEntityIdentity::m_flags
/// read from the identity SLOT via the ent_identity_flags op (books-translated id →
/// engine serial; chunk-validated shim-side) — NEVER via instance+0x10. The E1
/// replacement for the retired readInt32Via([16], 48) staging-flag chain.
fn s2_ent_identity_flags(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.ent_identity_flags else { return };
        let flags = func(index, serial);
        if flags >= 0 { rv.set_double(flags as f64); }
    }));
}

/// Native `__s2_ent_identity_flags_clear(index, id, mask) -> number | null`. Drops `mask`'s bits from
/// CEntityIdentity::m_flags and returns the result. CLEAR-ONLY, and EF_IS_INVALID_EHANDLE is refused
/// shim-side — a plugin must never be able to present a dead slot as live.
fn s2_ent_identity_flags_clear(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some((index, serial)) = ent_op_serial(scope, args.get(0), args.get(1)) else { return };
        let mask = args.get(2).uint32_value(scope).unwrap_or(0);
        if mask == 0 { return; }
        let Some(ops) = engine_ops() else { return };
        let Some(func) = ops.ent_identity_flags_clear else { return };
        let flags = func(index, serial, mask);
        if flags >= 0 { rv.set_double(flags as f64); }
    }));
}

/// Publish this feature's natives. Called from `v8host`'s `install_natives`.
pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_ent_ref_valid", s2_ent_ref_valid);
    set_native(scope, global_obj, "__s2_ent_ref_read", s2_ent_ref_read);
    set_native(scope, global_obj, "__s2_ent_ref_write", s2_ent_ref_write);
    set_native(scope, global_obj, "__s2_ent_ref_read_string", s2_ent_ref_read_string);
    set_native(scope, global_obj, "__s2_ent_ref_write_string", s2_ent_ref_write_string);
    set_native(scope, global_obj, "__s2_ent_ref_read_floats", s2_ent_ref_read_floats);
    set_native(scope, global_obj, "__s2_ent_ref_read_floats_chain", s2_ent_ref_read_floats_chain);
    set_native(scope, global_obj, "__s2_ent_ref_read_chain", s2_ent_ref_read_chain);
    set_native(scope, global_obj, "__s2_ent_ref_write_chain", s2_ent_ref_write_chain);
    set_native(scope, global_obj, "__s2_ent_ref_state_changed", s2_ent_ref_state_changed);
    set_native(scope, global_obj, "__s2_ent_id_for_index", s2_ent_id_for_index);
    set_native(scope, global_obj, "__s2_entity_create", s2_entity_create);
    set_native(scope, global_obj, "__s2_entity_find_by_class", s2_entity_find_by_class);
    set_native(scope, global_obj, "__s2_entity_spawn", s2_entity_spawn);
    set_native(scope, global_obj, "__s2_ent_set_model", s2_ent_set_model);
    // Entity-property slice: five engine setters with no usable schema-write equivalent.
    set_native(scope, global_obj, "__s2_ent_set_gravity_scale", s2_ent_set_gravity_scale);
    set_native(scope, global_obj, "__s2_ent_apply_abs_velocity_impulse", s2_ent_apply_abs_velocity_impulse);
    set_native(scope, global_obj, "__s2_ent_stop_sound", s2_ent_stop_sound);
    set_native(scope, global_obj, "__s2_ent_set_body_group_by_name", s2_ent_set_body_group_by_name);
    set_native(scope, global_obj, "__s2_ent_set_model_scale", s2_ent_set_model_scale);
    set_native(scope, global_obj, "__s2_entity_teleport", s2_entity_teleport);
    set_native(scope, global_obj, "__s2_entity_remove", s2_entity_remove);
    set_native(scope, global_obj, "__s2_entity_fire_input", s2_entity_fire_input);
    set_native(scope, global_obj, "__s2_entity_spawn_kv", s2_entity_spawn_kv);
    set_native(scope, global_obj, "__s2_entity_listener_on", s2_entity_listener_on);
    set_native(scope, global_obj, "__s2_entity_listener_off", s2_entity_listener_off);
    set_native(scope, global_obj, "__s2_entity_subobj_vcall", s2_entity_subobj_vcall);
    set_native(scope, global_obj, "__s2_entity_read_handle_vector", s2_entity_read_handle_vector);
    set_native(scope, global_obj, "__s2_entity_name", s2_entity_name);
    set_native(scope, global_obj, "__s2_entity_target", s2_entity_target);
    set_native(scope, global_obj, "__s2_ent_identity_flags", s2_ent_identity_flags);
    set_native(scope, global_obj, "__s2_ent_identity_flags_clear", s2_ent_identity_flags_clear);
}

/// The owner-scoped store: the IEntityListener stays registered for the process lifetime — no
/// engine-op follow-up on an emptied name.
pub(crate) fn register_store() {
    crate::owner_stores::register(
        "ENTITY_MUX",
        Box::new(|owner| { ENTITY_MUX.with(|m| { m.borrow_mut().remove_by_owner(owner); }); }),
        Box::new(|ids| { ENTITY_MUX.with(|m| { m.borrow_mut().remove_by_ids(ids); }); }),
        Box::new(|| { ENTITY_MUX.with(|m| *m.borrow_mut() = crate::channels::Channels::new()); }),
    );
}
/// Resolve (index, host-id) to a live entity pointer, or null. Resolution order
/// (north-star §3.1 — cheapest & safest first):
///   1. THE BOOKS: `engine_serial_for(index, id)` (LIVE[index].id == id), else null.
///      No engine memory touched.
///   2. Defense-in-depth: the shim validates the stored engine serial in the
///      system-owned identity CHUNK (`ent_resolve`, the s2_deref_handle idiom) and
///      returns m_pInstance — instance memory is never read to decide liveness.
///   3. Only the CALLER derefs the instance, block-scoped within one native.
/// The raw pointer stays in Rust — it never crosses to JS. Errors fall toward null,
/// never toward a deref.
fn entity_resolve_ptr(index: i32, id: u64) -> *mut u8 {
    let Some(engine_serial) = crate::entity_live::engine_serial_for(index, id) else {
        return std::ptr::null_mut();
    };
    let Some(ops) = engine_ops() else { return std::ptr::null_mut() };
    let Some(resolve) = ops.ent_resolve else { return std::ptr::null_mut() };
    resolve(index, engine_serial) as *mut u8
}

/// The JS half of `dispatch_entity_event`, and NOTHING else — no books feed, safe to run a frame
/// late.
///
/// A deferred `"delete"` delivers a `null` EntityRef: `ffi.rs` books the delete immediately after
/// the (skipped) dispatch, so by drain time `entity_live::adopt` fails. Accepted — it is the same
/// books-gated degrade any stale ref already gets, the className still identifies what died, and it
/// is strictly more than today's total drop. Documented in the entity `.d.ts`.
pub(crate) fn replay_entity_event(kind: &str, class_name: &str, handle: i32) -> Delivery {
    // Snapshot the exact-class key + the "<kind>\0*" wildcard (skip the wild when class == "*", else
    // the same key would be snapshotted twice). Both taken before any JS runs.
    let exact = format!("{}\0{}", kind, class_name);
    let mut snap = ENTITY_MUX.with(|m| m.borrow().snapshot(&exact));
    if class_name != "*" {
        let wild = format!("{}\0*", kind);
        snap.extend(ENTITY_MUX.with(|m| m.borrow().snapshot(&wild)));
    }
    let label = format!("dispatch_entity_event('{}','{}')", kind, class_name);
    fan_out(&snap, &label, Instrument::none(), |tc| {
        let entity_val: v8::Local<v8::Value> = if handle == -1 {
            v8::null(tc).into()
        } else {
            let (idx, ser) = crate::entity::decode_handle(handle as u32);
            // Books-adopt (delete dispatches still adopt — the ffi entry removes AFTER dispatch).
            match crate::entity_live::adopt(idx, ser) {
                Some(id) => build_entity_ref(tc, idx, id),
                None => v8::null(tc).into(),
            }
        };
        let class_val: v8::Local<v8::Value> = match v8::String::new(tc, class_name) {
            Some(s) => s.into(),
            None => v8::undefined(tc).into(),
        };
        Some(vec![entity_val, class_val])
    })
}

/// Like `read_vec3` but returns `None` when the arg isn't a 3-number array (for nullable teleport args —
/// `origin`/`angles`/`velocity` are each independently optional).
fn read_vec3_opt(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Option<[f32; 3]> {
    let arr = v8::Local::<v8::Array>::try_from(v).ok()?;
    if arr.length() != 3 { return None; }
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        out[i as usize] = arr.get_index(scope, i)?.number_value(scope).unwrap_or(0.0) as f32;
    }
    Some(out)
}

/// Read a JS array of numbers into a `Vec<i32>`. Returns `[]` if `v` isn't an array (or on any
/// per-element read failure the remaining elements are skipped) — never panics on bad input.
fn read_int_array(scope: &mut v8::PinScope, v: v8::Local<v8::Value>) -> Vec<i32> {
    let Ok(arr) = v8::Local::<v8::Array>::try_from(v) else { return Vec::new() };
    let len = arr.length();
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        let Some(el) = arr.get_index(scope, i) else { break };
        out.push(el.integer_value(scope).unwrap_or(0) as i32);
    }
    out
}

// The V8-surface tests, over the SHARED in-isolate harness (`v8host::frame_tests`). Kept separate
// from the pure pointer-arithmetic tests above, which need no isolate — see `crate::usermsg`.
//
// The EntityRef MARSHALLING tests (`iface_call_return_rehydrates_entityref` and friends) stay in
// `v8host.rs` with the interfaces cluster, and the reload/state-handoff one stays with lifecycle:
// they assert on how an EntityRef crosses a boundary, not on what these natives do.
#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string, load_body,
        mock_event_ops, read_global_string};
    use crate::v8host::frame_tests::eval_std;
    use crate::v8host::{create_plugin_context, init, set_engine_ops, shutdown, S2EngineOps};

    // --- E1 fake slot-side plumbing: a fake `ent_resolve` op + a books seed. Replaces the retired
    // FakeEnt/FakeIdent instance-identity fakes (nothing reads instance identity anymore — resolution
    // is books + slot op). The fake resolver answers for exactly one armed (index, engine_serial). ---
    thread_local! {
        static FAKE_RESOLVE_KEY: std::cell::Cell<(i32, i32)> = std::cell::Cell::new((-1, -1));
        static FAKE_RESOLVE_PTR: std::cell::Cell<*mut std::os::raw::c_void> =
            std::cell::Cell::new(std::ptr::null_mut());
    }
    extern "C" fn fake_ent_resolve(idx: c_int, serial: c_int) -> *mut std::os::raw::c_void {
        if (idx, serial) == FAKE_RESOLVE_KEY.with(|c| c.get()) { FAKE_RESOLVE_PTR.with(|c| c.get()) }
        else { std::ptr::null_mut() }
    }



    thread_local! { static FAKE_FLAGS: std::cell::Cell<i64> = std::cell::Cell::new(-1); }
    extern "C" fn fake_ent_identity_flags(idx: c_int, serial: c_int) -> i64 {
        if (idx, serial) == FAKE_RESOLVE_KEY.with(|c| c.get()) { FAKE_FLAGS.with(|c| c.get()) } else { -1 }
    }
    /// Slice 5A Task 3: the six (index, serial) natives degrade safely when no engine-ops table
    /// is wired (no crash, no UB — they return -1/false/null/no-op as documented).
    /// `__s2_handle_decode` is pure bit-math and works without ops.
    #[test]
    fn ent_ref_natives_degrade_without_engine_ops() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops table → every entity op is a safe miss
        create_plugin_context("p");
        // id_for_index → 0 (empty books) ; valid → false ; read → null ; write → false ; state_changed → no-op/undefined
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_id_for_index(1))"), "0");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_valid(1, 7))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read(1, 7, 8, 1))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write(1, 7, 8, 1, 5))"), "false");
        // handle_decode is PURE (no ops needed). BITS-agnostic assertion: 64 < 2^7 <= 2^HANDLE_ENTRY_BITS,
        // so index==64, serial==0 for any real bit-split (the exact split is validated live in the gate).
        assert_eq!(eval_in_context_string("p", "var d=__s2_handle_decode(64); d[0]+','+d[1]"), "64,0");
        shutdown();
    }

    thread_local! { static ENTITY_INSTALL_CALLS: std::cell::Cell<i32> = std::cell::Cell::new(0); }
    extern "C" fn capture_entity_install() -> c_int { ENTITY_INSTALL_CALLS.with(|c| c.set(c.get() + 1)); 1 }

    /// Seed the books + arm the fake slot resolver for (index, serial). Returns the minted host id.
    /// The backing buffer is a leaked 4KB zeroed block (writable, long-lived).
    fn arm_fake_entity(index: i32, serial: i32) -> u64 {
        let id = crate::entity_live::on_created(index, serial);
        let buf: &'static mut [u8; 4096] = Box::leak(Box::new([0u8; 4096]));
        FAKE_RESOLVE_KEY.with(|c| c.set((index, serial)));
        FAKE_RESOLVE_PTR.with(|c| c.set(buf.as_mut_ptr() as *mut std::os::raw::c_void));
        id
    }

    /// E1 ACCEPTANCE (unit form of the changelevel repro): a ref held across a map start
    /// resolves null/false/dead — even though the ENGINE-side slot still reads live (the
    /// fake resolver still answers). Stage 1 (the books) wins; the old design green-lit
    /// exactly this case into a UAF.
    #[test]
    fn stale_ref_after_map_start_is_null_not_uaf() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        let _id = arm_fake_entity(42, 7);
        set_engine_ops(Some(S2EngineOps { ent_resolve: Some(fake_ent_resolve), ..mock_event_ops() }));
        create_plugin_context("cl");
        eval_in_context_string("cl", r#"
            var E = __s2require("@s2script/sdk/entity").EntityRef;
            globalThis.__ref = new E(42, __s2_ent_id_for_index(42));
            globalThis.__before = __ref.isValid() + ":" + (__ref.readInt32(8) !== null);
            "ok"
        "#);
        assert_eq!(eval_in_context_string("cl", "globalThis.__before"), "true:true",
                   "live before the transition (books + fake slot agree)");
        // The implicit entity epoch — exactly what `s2script_core_dispatch_map_start` does to the
        // books UNCONDITIONALLY before the JS dispatch (Task 4). The fake engine slot STILL resolves
        // (simulating freed-but-unchanged memory) — the books alone must kill the ref.
        crate::entity_live::clear_for_map_transition();
        assert_eq!(eval_in_context_string("cl", "String(__ref.isValid())"), "false");
        assert_eq!(eval_in_context_string("cl", "String(__ref.readInt32(8))"), "null");
        assert_eq!(eval_in_context_string("cl", "String(__ref.writeInt32(8, 5))"), "false");
        shutdown();
    }

    /// E1 ACCEPTANCE: cross-map (index, serial) aliasing is impossible — the SAME engine
    /// pair re-created after a clear gets a FRESH host id; a ref captured before the
    /// transition stays dead (Candidate D's win over the bare (index,serial) table).
    #[test]
    fn same_index_serial_on_new_map_does_not_revive_old_refs() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        let old_id = arm_fake_entity(64, 3);
        set_engine_ops(Some(S2EngineOps { ent_resolve: Some(fake_ent_resolve), ..mock_event_ops() }));
        crate::entity_live::clear_for_map_transition();          // the epoch (map start)
        let new_id = crate::entity_live::on_created(64, 3);      // same pair, new map
        assert!(new_id > old_id);
        create_plugin_context("alias");
        eval_in_context_string("alias", &format!(r#"
            var E = __s2require("@s2script/sdk/entity").EntityRef;
            globalThis.__old = String(new E(64, {old_id}).isValid());
            globalThis.__new = String(new E(64, {new_id}).isValid());
            "ok"
        "#));
        assert_eq!(eval_in_context_string("alias", "__old"), "false", "old id never revives");
        assert_eq!(eval_in_context_string("alias", "__new"), "true");
        shutdown();
    }

    /// E1: identityFlags reads the SLOT (books-translated) — live flags cross; a stale ref
    /// or missing op degrades to null. This is the primitive pawn.isValid's staging check
    /// rides on, with the [16]->48 instance chain gone.
    #[test]
    fn identity_flags_is_slot_side_and_books_gated() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        let id = arm_fake_entity(9, 4);
        FAKE_FLAGS.with(|c| c.set(0x104));            // arbitrary flags incl. bit 2 (staging)
        set_engine_ops(Some(S2EngineOps {
            ent_identity_flags: Some(fake_ent_identity_flags), ..mock_event_ops() }));
        create_plugin_context("fl");
        eval_in_context_string("fl", &format!(r#"
            var E = __s2require("@s2script/sdk/entity").EntityRef;
            globalThis.__live  = String(new E(9, {id}).identityFlags());
            globalThis.__stale = String(new E(9, {id} + 1).identityFlags());
            "ok"
        "#));
        assert_eq!(eval_in_context_string("fl", "__live"), "260");
        assert_eq!(eval_in_context_string("fl", "__stale"), "null", "books gate the flags read");
        shutdown();
    }

    /// dispatch_entity_event delivers to the matching kind+class subscriber AND the "*" wildcard, with
    /// (entity, className). With no engine ops entity_resolve_ptr degrades to null, so the entity arg is
    /// null (also forced by handle=-1) and we assert on the className arg. Mirrors map_start_dispatch.
    #[test]
    fn entity_event_dispatch_delivers_class_to_matching_subscriber() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pel");
        eval_in_context_string("pel", r#"
            globalThis.__hits = [];
            var E = __s2pkg_entity.Entity;
            E.onSpawn("weapon_ak47", function (e, cls) { globalThis.__hits.push("exact:" + cls + ":" + (e === null)); });
            E.onSpawn("*",           function (e, cls) { globalThis.__hits.push("star:"  + cls + ":" + (e === null)); });
            E.onCreate("weapon_ak47", function (e, cls) { globalThis.__hits.push("create:" + cls); });
            "ok"
        "#);
        let _ = dispatch_entity_event("spawn", "weapon_ak47", -1);   // hits the exact + the "*" spawn subs, NOT the create sub
        assert_eq!(eval_in_context_string("pel", "globalThis.__hits.slice().sort().join('|')"),
                   "exact:weapon_ak47:true|star:weapon_ak47:true");
        shutdown();
    }

    /// kind separation: a "spawn" subscriber does NOT fire on a "delete"/"create" dispatch.
    #[test]
    fn entity_event_dispatch_respects_kind() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pel2");
        eval_in_context_string("pel2", r#"
            globalThis.__n = 0;
            __s2pkg_entity.Entity.onSpawn("*", function () { globalThis.__n++; });
            "ok"
        "#);
        let _ = dispatch_entity_event("delete", "prop_physics", -1);
        let _ = dispatch_entity_event("create", "prop_physics", -1);
        assert_eq!(eval_in_context_string("pel2", "String(globalThis.__n)"), "0", "spawn sub must not fire on delete/create");
        let _ = dispatch_entity_event("spawn", "prop_physics", -1);
        assert_eq!(eval_in_context_string("pel2", "String(globalThis.__n)"), "1");
        shutdown();
    }

    /// First-ever subscribe calls entity_listener_install exactly once; a second subscribe does not.
    #[test]
    fn entity_listener_install_called_once_on_first_subscribe() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps { entity_listener_install: Some(capture_entity_install), ..mock_event_ops() }));
        ENTITY_INSTALL_CALLS.with(|c| c.set(0));
        create_plugin_context("pel3");
        eval_in_context_string("pel3", r#"__s2pkg_entity.Entity.onSpawn("a", function(){}); "ok""#);
        assert_eq!(ENTITY_INSTALL_CALLS.with(|c| c.get()), 1, "install on first subscribe");
        eval_in_context_string("pel3", r#"__s2pkg_entity.Entity.onDelete("b", function(){}); "ok""#);
        assert_eq!(ENTITY_INSTALL_CALLS.with(|c| c.get()), 1, "no second install");
        shutdown();
    }

    /// dispatch_entity_event delivers a LIVE (non-null, books-adopted) EntityRef when the handle
    /// adopts: with the books seeded (via arm_fake_entity) and the fake `ent_resolve` slot op wired,
    /// the mint site adopts the decoded handle and the handler receives an EntityRef whose
    /// `isValid()===true` and whose `index`/`id` match the books. Exercises the non-null branch the
    /// other three tests (handle=-1) never hit; complements the live gate (which observed `valid=true`).
    #[test]
    fn entity_event_dispatch_delivers_live_entityref() {
        crate::entity_live::reset_for_tests();
        let _ = init(dummy_logger());
        let handle: u32 = 0x0001_8005;                       // decodes to a live (idx>0, serial>0)
        let (idx, serial) = crate::entity::decode_handle(handle);
        assert!(idx > 0 && serial > 0, "chosen handle must decode to a live (idx>0, serial>0), got ({idx},{serial})");
        let id = arm_fake_entity(idx, serial);               // seed the books + arm the fake slot resolver
        set_engine_ops(Some(S2EngineOps { ent_resolve: Some(fake_ent_resolve), ..mock_event_ops() }));
        create_plugin_context("pel4");
        eval_in_context_string("pel4", r#"
            globalThis.__got = "none";
            __s2pkg_entity.Entity.onSpawn("weapon_ak47", function (e, cls) {
                globalThis.__got = (e === null) ? "null"
                    : ("live:" + cls + ":" + e.isValid() + ":" + e.index + ":" + e.id);
            });
            "ok"
        "#);
        let _ = dispatch_entity_event("spawn", "weapon_ak47", handle as i32);
        assert_eq!(eval_in_context_string("pel4", "globalThis.__got"),
                   format!("live:weapon_ak47:true:{idx}:{id}"));
        shutdown();
    }

    /// Slice 5A Task 4: `EntityRef` from `@s2script/entity` degrades safely when no engine-ops table
    /// is wired — `isValid` returns false, `readInt32` returns null, `writeInt32` returns false.
    /// This is the failing test: EntityRef must be exported by the prelude (Step 3 makes it pass).
    #[test]
    fn entity_ref_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_body("er", r#"
            const { EntityRef } = require("@s2script/entity");
            const ref = new EntityRef(1, 7);
            globalThis.__valid = String(ref.isValid());       // "false"
            globalThis.__read  = String(ref.readInt32(8));    // "null"
            globalThis.__write = String(ref.writeInt32(8, 5));// "false"
        "#, "{}");
        assert_eq!(read_global_string("er", "__valid"), "false");
        assert_eq!(read_global_string("er", "__read"), "null");
        assert_eq!(read_global_string("er", "__write"), "false");
        shutdown();
    }

    /// The write-chain native degrades safely on every bad input (no live entity in the test isolate).
    #[test]
    fn ent_ref_write_chain_degrades_safely() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // stale/absent root ref (index 1, serial 7) → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, [0], 8, 2, 1.5))"), "false");
        // finalOff < 0 → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, [0], -1, 2, 1.5))"), "false");
        // non-array path arg → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, 5, 8, 2, 1.5))"), "false");
        // unknown kind (99) → false
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_write_chain(1, 7, [0], 8, 99, 1.5))"), "false");
        // the prelude wrapper forwards + degrades to false on a stale ref
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1, 7).writeFloat32Via([0], 8, 1.5))"#), "false");
        shutdown();
    }

    /// Entity-creation lifecycle slice: `createEntity` degrades to `null` with no `entity_create`
    /// op (e.g. every in-isolate test) — never a crash.
    #[test]
    fn entity_create_native_degrades_to_null_without_op() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        let out = eval_in_context_string("p", r#"
            const { createEntity } = __s2pkg_entity;
            String(createEntity("env_beam"))
        "#);
        assert_eq!(out, "null");
        shutdown();
    }

    /// entity_name slice: `EntityRef.name` (and the raw `__s2_entity_name` native) degrade to `null`
    /// with no `entity_name` op (e.g. every in-isolate test) — never a crash.
    #[test]
    fn entity_name_degrades_to_null_without_ops() {
        init(dummy_logger()).unwrap();
        // No ENGINE_OPS are installed in-isolate -> the op is absent -> both paths return null.
        let out = eval_std("en1", r#"
            var EntityRef = globalThis.__s2pkg_entity.EntityRef;
            var direct = __s2_entity_name(5, 7);
            var viaRef = new EntityRef(5, 7).name;
            JSON.stringify({ direct: direct, viaRef: viaRef });
        "#);
        assert_eq!(out, r#"{"direct":null,"viaRef":null}"#);
        shutdown();
    }

    /// entity_target slice: `EntityRef.target` (and the raw `__s2_entity_target` native) degrade to
    /// `null` with no ops (e.g. every in-isolate test) — never a crash.
    #[test]
    fn entity_target_degrades_to_null_without_ops() {
        init(dummy_logger()).unwrap();
        // No ENGINE_OPS are installed in-isolate -> entity_resolve_ptr is null -> both paths return null.
        let out = eval_std("et1", r#"
            var EntityRef = globalThis.__s2pkg_entity.EntityRef;
            var direct = __s2_entity_target(5, 7);
            var viaRef = new EntityRef(5, 7).target;
            JSON.stringify({ direct: direct, viaRef: viaRef });
        "#);
        assert_eq!(out, r#"{"direct":null,"viaRef":null}"#);
        shutdown();
    }
}

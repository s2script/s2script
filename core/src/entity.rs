//! Engine-generic raw entity-field access (V8-free helpers + guards). The entity-SYSTEM lookups
//! (by index / handle) and NetworkStateChanged live in v8host (they need engine pointers); this
//! module owns the pointer-arithmetic read/write so it is unit-testable without an engine.

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

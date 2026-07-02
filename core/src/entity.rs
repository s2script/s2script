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
}

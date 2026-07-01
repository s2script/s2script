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
}

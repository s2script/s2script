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
}

//! A few counters shared between the host and every plugin context's interface facades.
//!
//! The memory is plain Rust memory, so the registries can bump the epoch without touching V8;
//! `v8host::interop_facade` exposes the same bytes to each context as an `Int32Array`. Everything
//! here runs on the isolate's thread.
//!
//! - `DEPTH`: active interop calls (the 32-call recursion limit).
//! - `EPOCH`: bumped by every change that could affect a facade binding: a plugin registered or
//!   removed, an interface published or removed, a plugin's imports changing. A facade bound at an
//!   older epoch rebinds before it calls, so it can never call a retired producer.
//! - `PROFILING`: facades take the native path while the profiler records.

use std::cell::Cell;

pub(crate) const DEPTH: usize = 0;
pub(crate) const EPOCH: usize = 1;
pub(crate) const PROFILING: usize = 2;
pub(crate) const COUNT: usize = 4;

thread_local! {
    /// Leaked once per thread: the views V8 holds over it must never dangle.
    static SLOTS: &'static [Cell<i32>; COUNT] = Box::leak(Box::new(Default::default()));
}

/// The slots' address and size in bytes, for wrapping in a V8 backing store.
pub(crate) fn memory() -> (*mut std::ffi::c_void, usize) {
    SLOTS.with(|s| (s.as_ptr() as *mut std::ffi::c_void, COUNT * 4))
}

pub(crate) fn get(index: usize) -> i32 {
    SLOTS.with(|s| s[index].get())
}

pub(crate) fn set(index: usize, value: i32) {
    SLOTS.with(|s| s[index].set(value));
}

/// Invalidate every facade binding.
pub(crate) fn bump_epoch() {
    set(EPOCH, get(EPOCH).wrapping_add(1));
}

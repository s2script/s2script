//! Outbound-native nest token.
//!
//! A JS `FunctionCallback` already has a live `FunctionCallbackInfo` on the stack. rusty_v8
//! 149.4.0 can build `CallbackScope` from that handle without taking `HOST` (see
//! `promise_reject_cb`). Publishing the pointer for the FFI window is what lets inbound
//! `fan_out_inner` run other plugins while the caller is paused in the native.
//!
//! Empty stack = true engine inbound (or a native that must not nest, e.g. `__s2_defer_selftest`).
//! `#63` still applies: do not reconstruct `&mut Isolate` from a raw pointer.

use std::cell::RefCell;

use v8::FunctionCallbackArguments;
use v8::FunctionCallbackInfo;

/// Backstop against engine → JS → engine → JS bombs. Same-hook skip is the precise latch;
/// this is just a ceiling.
pub(crate) const MAX_NEST: usize = 8;

thread_local! {
    static NEST: RefCell<Vec<*const FunctionCallbackInfo>> = const { RefCell::new(Vec::new()) };
}

/// rusty_v8 149.4.0: `FunctionCallbackArguments { info, data: Option<Local>, length: Option<i32> }`
/// is not `repr(C)` and does not expose `info`. `data` starts as `None` (a null word). Scan
/// pointer-sized slots for the first non-null — that is `info` (`&FunctionCallbackInfo`).
fn info_ptr(args: &FunctionCallbackArguments<'_>) -> *const FunctionCallbackInfo {
    let base = args as *const FunctionCallbackArguments as *const usize;
    for i in 0..4 {
        let w = unsafe { *base.add(i) };
        if w != 0 {
            return w as *const FunctionCallbackInfo;
        }
    }
    std::ptr::null()
}

/// RAII push of the current native's `FunctionCallbackInfo` for the engine FFI window.
pub(crate) struct NestGuard;

impl NestGuard {
    /// `None` if the nest is already at [`MAX_NEST`] — caller still invokes the engine; inbound
    /// fan-out then sees an empty extra frame and uses the `#63` skip/defer path.
    pub(crate) fn push(args: &FunctionCallbackArguments<'_>) -> Option<Self> {
        let ok = NEST.with(|n| {
            let mut v = n.borrow_mut();
            if v.len() >= MAX_NEST {
                return false;
            }
            v.push(info_ptr(args));
            true
        });
        ok.then_some(NestGuard)
    }
}

impl Drop for NestGuard {
    fn drop(&mut self) {
        NEST.with(|n| {
            n.borrow_mut().pop();
        });
    }
}

/// Innermost published handle, if any. The pointer is valid only while the matching [`NestGuard`]
/// is alive (the originating `FunctionCallback` is still on the stack).
pub(crate) fn top() -> Option<*const FunctionCallbackInfo> {
    NEST.with(|n| n.borrow().last().copied())
}

/// Run `f` with the current native published as a nest token.
pub(crate) fn with_outbound<R>(args: &FunctionCallbackArguments<'_>, f: impl FnOnce() -> R) -> R {
    let _guard = NestGuard::push(args);
    f()
}

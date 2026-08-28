//! Outbound-native nest token.
//!
//! A JS `FunctionCallback` already has a live `FunctionCallbackInfo` on the stack. rusty_v8
//! 149.4.0 can build `CallbackScope` from that handle without taking `HOST` (see
//! `promise_reject_cb`). Publishing the pointer for the FFI window is what lets inbound
//! `fan_out_inner` run other plugins while the caller is paused in the native.
//!
//! Empty stack = true engine inbound (C-ABI, no live `FunctionCallbackInfo`) or a
//! native that has not published a token. `#63` still applies: do not reconstruct
//! `&mut Isolate` from a raw pointer.

use std::cell::RefCell;

use v8::FunctionCallbackArguments;
use v8::FunctionCallbackInfo;

/// Backstop against engine → JS → engine → JS bombs. Same-hook skip is the precise latch;
/// this is just a ceiling.
pub(crate) const MAX_NEST: usize = 8;

thread_local! {
    static NEST: RefCell<Vec<*const FunctionCallbackInfo>> = const { RefCell::new(Vec::new()) };
}

/// Byte offset of the `info` field inside `FunctionCallbackArguments`, discovered once at runtime.
///
/// rusty_v8 149.4.0 declares
/// `FunctionCallbackArguments { info: &FunctionCallbackInfo, data: Option<Local>, length: Option<int> }`
/// with no `repr(C)`, so field order is the compiler's choice and is not part of the crate's
/// contract — it can change with a rustc or crate bump.
///
/// This used to guess: take the first non-null word and publish it as `info`. When the compiler
/// put `data` or `length` first, we published a non-pointer as a `FunctionCallbackInfo`,
/// `fan_out_inner` built a `CallbackScope` from it, and `GetIsolate()` dereferenced it at `+0x28`.
/// Observed on a live server as `SIGSEGV` reading `0x00007fff00000028` with
/// `rdi = 0x00007fff00000000` — a value with its whole low half zeroed, i.e. not a pointer.
///
/// So do not guess and do not pattern-match on what a pointer "looks like" either: build an
/// `args` from a pointer we already know, via the crate's own public constructor, and record which
/// word it landed in. That is exact for whatever layout this build actually chose.
fn info_offset() -> Option<usize> {
    static OFFSET: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        // A real owned allocation, so the reference we hand the constructor points at live memory.
        // Nothing ever reads through it as a `FunctionCallbackInfo`; it is only ever compared.
        let probe: Box<[usize; 8]> = Box::new([0; 8]);
        let raw = Box::into_raw(probe);
        let want = raw as usize;
        let found = {
            let args = FunctionCallbackArguments::from_function_callback_info(unsafe {
                &*(raw as *const FunctionCallbackInfo)
            });
            let base = &args as *const FunctionCallbackArguments as *const usize;
            // 4 words covers `info` + `data` + `length` plus any tail padding.
            (0..4).find(|i| unsafe { *base.add(*i) } == want)
        };
        unsafe { drop(Box::from_raw(raw)) };
        found
    })
}

/// The current native's `FunctionCallbackInfo`, or null if the layout probe failed.
///
/// Null publishes an empty nest frame, which sends `fan_out_inner` down the documented skip/defer
/// path — the same one an engine-inbound call with no live `FunctionCallbackInfo` takes. Losing
/// nesting is a graceful degrade; dereferencing a guess is not.
fn info_ptr(args: &FunctionCallbackArguments<'_>) -> *const FunctionCallbackInfo {
    let Some(off) = info_offset() else { return std::ptr::null() };
    let base = args as *const FunctionCallbackArguments as *const usize;
    unsafe { *base.add(off) as *const FunctionCallbackInfo }
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

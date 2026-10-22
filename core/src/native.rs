//! Generated native bindings (performance plan, Stage 2): write a native once, as a typed Rust
//! function, and get the V8 slow callback, the V8 fast-API entry, its typing and its docs from it.
//!
//! ```ignore
//! native! {
//!     /// The current event's int field by interned key (see `__s2_event_key`).
//!     fast fn __s2_event_get_int_k(key: i32) -> i32 { … }
//! }
//! ```
//!
//! expands to a module named after the native holding:
//!
//! * `call` — the body, as an ordinary Rust function;
//! * `slow` — the V8 callback: checks arity, decodes each argument ([`FromJs`]), runs `call`
//!   under `catch_unwind`, and encodes the result ([`IntoJs`]);
//! * with `fast`: `fast` and `CFN`, the fast-API entry V8 calls from optimised code. The compiler
//!   refuses `fast` unless every argument and the result are fast-eligible ([`FastArg`],
//!   [`FastRet`]), so a slow-only type cannot slip onto the fast path;
//! * `META` — name, parameters, TypeScript types and doc comment. Each feature lists its natives
//!   in one `NATIVES` slice, which is both what `install` registers and what
//!   `natives.generated.d.ts` is rendered from (a test fails when the committed file drifts).
//!
//! **One failure convention.** A native's result type says how it fails, and nothing else does:
//! `T` cannot fail; `Option<T>` is `T | null`; `Result<T, S2Error>` throws an `S2Error` (an `Error`
//! whose `name` is `"S2Error"` and whose `code` names the failure). Too few arguments throw
//! `S2Error` code `"arity"`, and a panic throws code `"panic"` instead of vanishing. That replaces
//! the hand-written mix of `0`, `-1`, `""`, `false`, `null` and silent returns.

use v8::fast_api::{CTypeInfo, Type as FastType};

/// A native's typed failure, thrown into JS as an `S2Error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct S2Error {
    pub code: &'static str,
    pub message: String,
}

impl S2Error {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// Throw `err` as `Error { name: "S2Error", code, message }`.
pub(crate) fn throw(scope: &mut v8::PinScope, err: S2Error) {
    let Some(msg) = v8::String::new(scope, &err.message) else { return };
    let exc = v8::Exception::error(scope, msg);
    if let Ok(obj) = v8::Local::<v8::Object>::try_from(exc) {
        let set = |scope: &mut v8::PinScope, k: &str, v: &str| {
            if let (Some(k), Some(v)) = (v8::String::new(scope, k), v8::String::new(scope, v)) {
                obj.set(scope, k.into(), v.into());
            }
        };
        set(scope, "name", "S2Error");
        set(scope, "code", err.code);
    }
    scope.throw_exception(exc);
}

/// Decode argument `i`. Numbers and booleans coerce as JS does (`ToInt32`, `ToNumber`, …), which is
/// also what the fast path accepts.
pub(crate) trait FromJs: Sized {
    fn from_js(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Result<Self, S2Error>;
}

impl FromJs for i32 {
    fn from_js(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Result<Self, S2Error> {
        Ok(args.get(i).int32_value(scope).unwrap_or(0))
    }
}
impl FromJs for u32 {
    fn from_js(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Result<Self, S2Error> {
        Ok(args.get(i).uint32_value(scope).unwrap_or(0))
    }
}
impl FromJs for f64 {
    fn from_js(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Result<Self, S2Error> {
        Ok(args.get(i).number_value(scope).unwrap_or(f64::NAN))
    }
}
impl FromJs for f32 {
    fn from_js(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Result<Self, S2Error> {
        Ok(args.get(i).number_value(scope).unwrap_or(f64::NAN) as f32)
    }
}
impl FromJs for bool {
    fn from_js(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Result<Self, S2Error> {
        Ok(args.get(i).boolean_value(scope))
    }
}
impl FromJs for String {
    fn from_js(scope: &mut v8::PinScope, args: &v8::FunctionCallbackArguments, i: i32) -> Result<Self, S2Error> {
        Ok(args.get(i).to_rust_string_lossy(scope))
    }
}

/// Encode a result into the return value.
pub(crate) trait IntoJs {
    fn into_js(self, scope: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error>;
}

impl IntoJs for () {
    fn into_js(self, _: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        rv.set_undefined();
        Ok(())
    }
}
impl IntoJs for i32 {
    fn into_js(self, _: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        rv.set_int32(self);
        Ok(())
    }
}
impl IntoJs for u32 {
    fn into_js(self, _: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        rv.set_uint32(self);
        Ok(())
    }
}
impl IntoJs for f64 {
    fn into_js(self, _: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        rv.set_double(self);
        Ok(())
    }
}
impl IntoJs for f32 {
    fn into_js(self, _: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        rv.set_double(self as f64);
        Ok(())
    }
}
impl IntoJs for bool {
    fn into_js(self, _: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        rv.set_bool(self);
        Ok(())
    }
}
impl IntoJs for String {
    fn into_js(self, scope: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        let s = v8::String::new(scope, &self).ok_or_else(|| S2Error::new("alloc", "string too long"))?;
        rv.set(s.into());
        Ok(())
    }
}
impl<T: IntoJs> IntoJs for Option<T> {
    fn into_js(self, scope: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        match self {
            Some(v) => v.into_js(scope, rv),
            None => {
                rv.set_null();
                Ok(())
            }
        }
    }
}
impl<T: IntoJs> IntoJs for Result<T, S2Error> {
    fn into_js(self, scope: &mut v8::PinScope, rv: &mut v8::ReturnValue) -> Result<(), S2Error> {
        self?.into_js(scope, rv)
    }
}

/// The TypeScript type a Rust parameter or result appears as.
pub(crate) trait JsType {
    fn ts() -> String;
}
macro_rules! js_type {
    ($($t:ty => $ts:literal),* $(,)?) => { $(impl JsType for $t { fn ts() -> String { $ts.into() } })* };
}
js_type!(() => "void", i32 => "number", u32 => "number", f64 => "number", f32 => "number",
         bool => "boolean", String => "string");
impl<T: JsType> JsType for Option<T> {
    fn ts() -> String {
        format!("{} | null", T::ts())
    }
}
impl<T: JsType> JsType for Result<T, S2Error> {
    fn ts() -> String {
        T::ts()
    }
}

/// A fast-API-eligible parameter type.
pub(crate) trait FastArg {
    const INFO: CTypeInfo;
}
/// A fast-API-eligible result type, with what the fast entry returns if its body panics.
pub(crate) trait FastRet {
    const INFO: CTypeInfo;
    const FALLBACK: Self;
}
macro_rules! fast_types {
    ($($t:ty => $ft:ident, $fallback:expr);* $(;)?) => {
        $(
            impl FastArg for $t { const INFO: CTypeInfo = FastType::$ft.as_info(); }
            impl FastRet for $t { const INFO: CTypeInfo = FastType::$ft.as_info(); const FALLBACK: Self = $fallback; }
        )*
    };
}
fast_types!(i32 => Int32, 0; u32 => Uint32, 0; f64 => Float64, f64::NAN; f32 => Float32, f32::NAN; bool => Bool, false);
impl FastRet for () {
    const INFO: CTypeInfo = FastType::Void.as_info();
    const FALLBACK: Self = ();
}

/// A native's registration and description.
pub(crate) struct Meta {
    pub name: &'static str,
    pub doc: &'static [&'static str],
    pub params: &'static [(&'static str, fn() -> String)],
    pub ret: fn() -> String,
    pub fast: bool,
    pub install: fn(&mut v8::PinScope, v8::Local<v8::Object>),
}

/// Register `natives` on a context's global object.
pub(crate) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>, natives: &[&Meta]) {
    for n in natives {
        (n.install)(scope, global);
    }
}

#[doc(hidden)]
pub(crate) const fn one(_: &str) -> usize {
    1
}

/// The shared slow-callback body every `native!` expands to.
#[doc(hidden)]
pub(crate) fn slow_body<R: IntoJs>(
    scope: &mut v8::PinScope,
    args: &v8::FunctionCallbackArguments,
    rv: &mut v8::ReturnValue,
    name: &str,
    arity: usize,
    body: impl FnOnce(&mut v8::PinScope, &v8::FunctionCallbackArguments) -> Result<R, S2Error>,
) {
    if (args.length().max(0) as usize) < arity {
        let s = if arity == 1 { "" } else { "s" };
        throw(scope, S2Error::new("arity", format!("{name} expects {arity} argument{s}")));
        return;
    }
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(scope, args)));
    let result = match ran {
        Ok(Ok(value)) => value.into_js(scope, rv),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(S2Error::new("panic", format!("{name} panicked"))),
    };
    if let Err(e) = result {
        throw(scope, e);
    }
}

/// Define a native. See the module docs.
macro_rules! native {
    ($(#[doc = $doc:literal])* fast fn $name:ident($($arg:ident : $ty:ty),* $(,)?) -> $ret:ty $body:block) => {
        $crate::native::native!(@gen fast [$($doc)*] $name ($($arg : $ty),*) $ret $body);
    };
    ($(#[doc = $doc:literal])* fn $name:ident($($arg:ident : $ty:ty),* $(,)?) -> $ret:ty $body:block) => {
        $crate::native::native!(@gen slow [$($doc)*] $name ($($arg : $ty),*) $ret $body);
    };
    (@gen $mode:ident [$($doc:literal)*] $name:ident ($($arg:ident : $ty:ty),*) $ret:ty $body:block) => {
        #[allow(non_snake_case, unused_imports, unused_mut, unused_variables, unused_assignments, dead_code)]
        pub(crate) mod $name {
            use super::*;

            const ARITY: usize = 0 $(+ $crate::native::one(stringify!($arg)))*;

            pub(crate) fn call($($arg: $ty),*) -> $ret $body

            pub(crate) fn slow(
                scope: &mut v8::PinScope,
                args: v8::FunctionCallbackArguments,
                mut rv: v8::ReturnValue,
            ) {
                $crate::native::slow_body(scope, &args, &mut rv, stringify!($name), ARITY, |scope, args| {
                    let mut i = 0i32;
                    $(
                        let $arg = <$ty as $crate::native::FromJs>::from_js(scope, args, i)?;
                        i += 1;
                    )*
                    Ok(call($($arg),*))
                })
            }

            $crate::native::native!(@entry $mode $name ($($arg : $ty),*) $ret);

            pub(crate) const META: $crate::native::Meta = $crate::native::Meta {
                name: stringify!($name),
                doc: &[$($doc),*],
                params: &[$((stringify!($arg), <$ty as $crate::native::JsType>::ts)),*],
                ret: <$ret as $crate::native::JsType>::ts,
                fast: $crate::native::native!(@is_fast $mode),
                install,
            };
        }
    };
    (@entry fast $name:ident ($($arg:ident : $ty:ty),*) $ret:ty) => {
        /// The fast-API entry: optimised JS calls this directly. No V8 heap, no panics escape.
        #[allow(improper_ctypes_definitions)]
        pub(crate) extern "C" fn fast(_recv: v8::Local<v8::Object>, $($arg: $ty),*) -> $ret {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call($($arg),*)))
                .unwrap_or(<$ret as $crate::native::FastRet>::FALLBACK)
        }

        pub(crate) const CFN: v8::fast_api::CFunction = v8::fast_api::CFunction::new(
            fast as *const std::ffi::c_void,
            &v8::fast_api::CFunctionInfo::new(
                <$ret as $crate::native::FastRet>::INFO,
                &[v8::fast_api::Type::V8Value.as_info(), $(<$ty as $crate::native::FastArg>::INFO),*],
                v8::fast_api::Int64Representation::Number,
            ),
        );

        fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
            $crate::v8host::set_fast_native(scope, global, stringify!($name), slow, &CFN);
        }
    };
    (@entry slow $name:ident ($($arg:ident : $ty:ty),*) $ret:ty) => {
        fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
            $crate::v8host::set_native(scope, global, stringify!($name), slow);
        }
    };
    (@is_fast fast) => { true };
    (@is_fast slow) => { false };
}
pub(crate) use native;

/// Every `native!`-defined native, by feature. The `.d.ts` is rendered from this list.
pub(crate) fn all() -> Vec<&'static Meta> {
    let mut v: Vec<&'static Meta> = Vec::new();
    v.extend_from_slice(crate::events::NATIVES);
    v
}

/// The declaration file for `natives`, sorted by name.
pub(crate) fn render_dts(natives: &[&Meta]) -> String {
    let mut sorted: Vec<&&Meta> = natives.iter().collect();
    sorted.sort_by_key(|n| n.name);
    let mut out = String::from(
        "// GENERATED by core/src/native.rs from the `native!` definitions — do not edit.\n\
         // Regenerate: S2_BLESS=1 cargo test -p s2script-core natives_dts_is_fresh\n\
         //\n\
         // Internal natives behind the prelude. Failures follow one convention: `T | null` means\n\
         // \"none\", and anything else that fails throws an `S2Error` with a `code`.\n",
    );
    for n in sorted {
        out.push('\n');
        if !n.doc.is_empty() || n.fast {
            out.push_str("/**\n");
            for line in n.doc {
                let line = line.strip_prefix(' ').unwrap_or(line);
                out.push_str(&format!(" *{}{}\n", if line.is_empty() { "" } else { " " }, line));
            }
            if n.fast {
                if !n.doc.is_empty() {
                    out.push_str(" *\n");
                }
                out.push_str(" * Fast-API native: optimised callers skip the V8 callback.\n");
            }
            out.push_str(" */\n");
        }
        let params: Vec<String> = n.params.iter().map(|(p, ts)| format!("{}: {}", p, ts())).collect();
        out.push_str(&format!("declare function {}({}): {};\n", n.name, params.join(", "), (n.ret)()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v8host::frame_tests::{dummy_logger, eval_in_context_string};
    use crate::v8host::{clone_plugin_context, create_plugin_context, init, shutdown, with_host_isolate};

    native! {
        /// Some(x) for a positive x.
        fn __s2_test_opt(x: i32) -> Option<i32> { (x > 0).then_some(x) }
    }
    native! {
        fn __s2_test_res(x: i32) -> Result<String, S2Error> {
            if x > 0 { Ok(format!("ok{x}")) } else { Err(S2Error::new("bad_input", "x must be positive")) }
        }
    }
    native! {
        fn __s2_test_panic() -> i32 { panic!("deliberate test panic") }
    }
    native! {
        fast fn __s2_test_add(a: i32, b: i32) -> i32 { a.wrapping_add(b) }
    }
    const TEST_NATIVES: &[&Meta] =
        &[&__s2_test_opt::META, &__s2_test_res::META, &__s2_test_panic::META, &__s2_test_add::META];

    fn install_into(id: &str, natives: &[&Meta]) {
        let ctx = clone_plugin_context(id).expect("plugin context");
        with_host_isolate(|isolate| {
            let mut hs_storage = v8::HandleScope::new(isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let c = v8::Local::new(hs, &ctx);
            let cs = &mut v8::ContextScope::new(hs, c);
            let global = c.global(cs);
            install(cs, global, natives);
        })
        .ok()
        .expect("host isolate");
    }

    /// The failure convention, end to end: `Option` → `null`, `Result::Err` → `S2Error` with its
    /// code, too few arguments → code `arity`, a panic → code `panic` (and the isolate lives on).
    #[test]
    fn natives_follow_the_one_failure_convention() {
        let _ = init(dummy_logger());
        create_plugin_context("n");
        install_into("n", TEST_NATIVES);
        let probe = |src: &str| {
            eval_in_context_string("n", &format!(
                "(function () {{ try {{ return String({src}); }} catch (e) {{ return e.name + ':' + e.code + ':' + e.message; }} }})()"
            ))
        };
        assert_eq!(probe("__s2_test_opt(3)"), "3");
        assert_eq!(probe("__s2_test_opt(-1)"), "null");
        assert_eq!(probe("__s2_test_res(2)"), "ok2");
        assert_eq!(probe("__s2_test_res(0)"), "S2Error:bad_input:x must be positive");
        assert_eq!(probe("__s2_test_opt()"), "S2Error:arity:__s2_test_opt expects 1 argument");
        assert_eq!(probe("__s2_test_add(1)"), "S2Error:arity:__s2_test_add expects 2 arguments");
        assert_eq!(probe("__s2_test_panic()"), "S2Error:panic:__s2_test_panic panicked");
        assert_eq!(probe("__s2_test_add(2, 3)"), "5", "the isolate survives a native's panic");
        shutdown();
    }

    /// A `fast` native answers the same once V8 optimises its caller onto the fast path.
    #[test]
    fn fast_natives_match_their_slow_path_once_optimized() {
        let _ = init(dummy_logger());
        create_plugin_context("n");
        install_into("n", TEST_NATIVES);
        let total = eval_in_context_string("n", r#"
            (function () { var t = 0; for (var i = 0; i < 200000; i++) t = (t + __s2_test_add(i, 1)) | 0; return t; })()
        "#);
        let mut want: i32 = 0;
        for i in 0..200000i32 { want = want.wrapping_add(i.wrapping_add(1)); }
        assert_eq!(total, want.to_string());
        shutdown();
    }

    #[test]
    fn metadata_renders_typed_declarations() {
        let dts = render_dts(TEST_NATIVES);
        assert!(dts.contains("/**\n * Some(x) for a positive x.\n */\ndeclare function __s2_test_opt(x: number): number | null;"), "{dts}");
        assert!(dts.contains("declare function __s2_test_res(x: number): string;"));
        assert!(dts.contains(" * Fast-API native: optimised callers skip the V8 callback.\n */\ndeclare function __s2_test_add(a: number, b: number): number;"));
    }

    /// `core/js/natives.generated.d.ts` must match the `native!` definitions. Set `S2_BLESS=1` to
    /// rewrite it after adding or changing a native.
    #[test]
    fn natives_dts_is_fresh() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/js/natives.generated.d.ts");
        let rendered = render_dts(&all());
        if std::env::var_os("S2_BLESS").is_some() {
            std::fs::write(path, &rendered).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            committed == rendered,
            "core/js/natives.generated.d.ts is stale; run `S2_BLESS=1 cargo test -p s2script-core natives_dts_is_fresh`"
        );
    }

    #[test]
    fn native_names_are_unique() {
        let mut names: Vec<&str> = all().iter().map(|n| n.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "two natives share a name");
    }
}

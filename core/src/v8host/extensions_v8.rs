//! Binding native extensions into a plugin context (see `crate::extensions`).
//!
//! `module` builds a frozen, prototype-less object with one function per registered native. A
//! fast-eligible native gets V8's fast API pointed straight at the extension's function, so
//! optimised code calls it with no host code in between. Every native also has the generic slow
//! callback below, which V8 uses for unoptimised callers and which is the only path for strings,
//! entities and natives that can fail. The two agree on every answer: the slow path converts
//! numbers and booleans exactly as V8's fast path does (`ToInt32`, `ToUint32`, `ToNumber`,
//! `ToBoolean`).

use std::cell::RefCell;
use std::ffi::{c_void, CStr};

use crate::extensions::{self, Arg, Native, Type};
use crate::native::{throw, throw_code, S2Error};

/// The module object for loaded extension `ext`, built in the current context.
pub(crate) fn module<'s>(scope: &mut v8::PinScope<'s, '_>, ext: usize) -> Option<v8::Local<'s, v8::Object>> {
    let null = v8::null(scope).into();
    let obj = v8::Object::with_prototype_and_properties(scope, null, &[], &[]);
    for native in extensions::natives_of(ext) {
        let data = v8::External::new(scope, native as *const Native as *mut c_void);
        let builder = v8::FunctionTemplate::builder(slow).data(data.into()).length(native.args.len() as i32);
        let template = match native.fast {
            Some(info) => {
                let cfn = v8::fast_api::CFunction::new(native.func, info);
                builder.build_fast(scope, &[cfn])
            }
            None => builder.build(scope),
        };
        let func = template.get_function(scope)?;
        let name = v8::String::new(scope, &native.name)?;
        func.set_name(name);
        obj.create_data_property(scope, name.into(), func.into())?;
    }
    obj.set_integrity_level(scope, v8::IntegrityLevel::Frozen)?;
    Some(obj)
}

thread_local! {
    /// The string arguments of the call in progress, reused across calls.
    static STRINGS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn native_of(args: &v8::FunctionCallbackArguments) -> &'static Native {
    let data = v8::Local::<v8::External>::try_from(args.data()).expect("extension native without data");
    // Set by `module` to a leaked, never-freed `Native`.
    unsafe { &*(data.value() as *const Native) }
}

fn get<'s>(scope: &mut v8::PinScope<'s, '_>, obj: v8::Local<v8::Object>, key: &str) -> Option<v8::Local<'s, v8::Value>> {
    let k = v8::String::new(scope, key)?;
    obj.get(scope, k.into())
}

/// The generic path. Returns `None` when a JS exception is already pending (a throwing getter or
/// `toString`), `Some(Err)` for a failure to throw as `S2Error`.
fn call_slow(
    scope: &mut v8::PinScope,
    args: &v8::FunctionCallbackArguments,
    rv: &mut v8::ReturnValue,
    native: &'static Native,
) -> Option<Result<(), S2Error>> {
    let n = native.args.len();
    if (args.length().max(0) as usize) < n {
        let s = if n == 1 { "" } else { "s" };
        return Some(Err(S2Error::new("arity", format!("{} expects {n} argument{s}", native.name))));
    }
    // Phase one converts every argument; this is the only place plugin code can run (a getter, a
    // `toString`). Phase two then resolves entities and encodes strings with no plugin code in
    // between, so nothing can free an entity or reuse the string buffer before the call.
    enum Pending<'s> {
        Ready(Arg),
        Str(v8::Local<'s, v8::String>),
        Entity(i32, u64),
    }
    let mut pending: Vec<Pending> = Vec::with_capacity(n);
    for (i, ty) in native.args.iter().enumerate() {
        let v = args.get(i as i32);
        pending.push(match ty {
            Type::Bool => Pending::Ready(Arg::Int(v.boolean_value(scope) as u64)),
            Type::I32 => Pending::Ready(Arg::Int(v.int32_value(scope)? as i64 as u64)),
            Type::U32 => Pending::Ready(Arg::Int(v.uint32_value(scope)? as u64)),
            Type::F32 => Pending::Ready(Arg::F32(v.number_value(scope)? as f32)),
            Type::F64 => Pending::Ready(Arg::F64(v.number_value(scope)?)),
            Type::String => Pending::Str(v.to_string(scope)?),
            Type::Entity => {
                let stale = Pending::Entity(-1, 0);
                match v8::Local::<v8::Object>::try_from(v) {
                    Err(_) => stale,
                    Ok(obj) => {
                        let index = get(scope, obj, "index")?;
                        let id = get(scope, obj, "id")?;
                        match (index.int32_value(scope), id.number_value(scope)) {
                            (Some(index), Some(id)) if id >= 0.0 && id.fract() == 0.0 && id < 9_007_199_254_740_992.0 => {
                                Pending::Entity(index, id as u64)
                            }
                            _ => stale,
                        }
                    }
                }
            }
            Type::Void => unreachable!("registration refuses void arguments"),
        });
    }
    // Every string goes into one reused buffer, NUL-terminated; pointers are taken once it is full.
    let mut buf = STRINGS.with(|b| std::mem::take(&mut *b.borrow_mut()));
    buf.clear();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for p in &pending {
        if let Pending::Str(s) = p {
            let len = s.utf8_length(scope);
            let at = buf.len();
            buf.resize(at + len + 1, 0);
            let written = s.write_utf8_v2(scope, &mut buf[at..at + len], v8::WriteFlags::kReplaceInvalidUtf8, None);
            spans.push((at, written));
        }
    }
    let mut raw: Vec<Arg> = Vec::with_capacity(n);
    let mut next_span = 0;
    for (i, p) in pending.iter().enumerate() {
        raw.push(match *p {
            Pending::Ready(a) => a,
            Pending::Str(_) => {
                let (at, len) = spans[next_span];
                next_span += 1;
                if buf[at..at + len].contains(&0) {
                    STRINGS.with(|b| *b.borrow_mut() = buf);
                    return Some(Err(S2Error::new("argument", format!("{}: argument {i} contains a NUL character", native.name))));
                }
                buf[at + len] = 0;
                Arg::Int(buf[at..].as_ptr() as u64)
            }
            Pending::Entity(index, id) => {
                let ptr = if index < 0 { std::ptr::null_mut() } else { crate::entity::entity_resolve_ptr(index, id) };
                if ptr.is_null() {
                    STRINGS.with(|b| *b.borrow_mut() = buf);
                    return Some(Err(S2Error::new("stale_entity", format!("{}: argument {i} is not a live entity", native.name))));
                }
                Arg::Int(ptr as u64)
            }
        });
    }
    let (out, failed) = extensions::with_fail_scope(native.flags, || unsafe { extensions::call(native.func, &raw) });
    STRINGS.with(|b| *b.borrow_mut() = buf);
    if let Some((code, message)) = failed {
        throw_code(scope, &code, &message);
        return None;
    }
    match native.ret {
        Type::Void => rv.set_undefined(),
        Type::Bool => rv.set_bool(out.rax as u8 != 0),
        Type::I32 => rv.set_int32(out.rax as u32 as i32),
        Type::U32 => rv.set_uint32(out.rax as u32),
        Type::F32 => rv.set_double(f32::from_bits(out.xmm0.to_bits() as u32) as f64),
        Type::F64 => rv.set_double(out.xmm0),
        Type::String => {
            let p = out.rax as *const std::ffi::c_char;
            if p.is_null() {
                rv.set_null();
            } else {
                let s = unsafe { CStr::from_ptr(p) }.to_string_lossy();
                let Some(s) = v8::String::new(scope, &s) else {
                    return Some(Err(S2Error::new("alloc", "string too long")));
                };
                rv.set(s.into());
            }
        }
        Type::Entity => unreachable!("registration refuses entity results"),
    }
    Some(Ok(()))
}

#[cfg(test)]
thread_local! {
    /// Calls that took the slow path (tests prove optimised code takes the fast one).
    pub(crate) static SLOW_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn slow(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    #[cfg(test)]
    SLOW_CALLS.with(|c| c.set(c.get() + 1));
    let native = native_of(&args);
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call_slow(scope, &args, &mut rv, native)));
    match ran {
        Ok(Some(Ok(()))) | Ok(None) => {}
        Ok(Some(Err(e))) => throw(scope, e),
        Err(_) => throw(scope, S2Error::new("panic", format!("{} panicked", native.name))),
    }
}

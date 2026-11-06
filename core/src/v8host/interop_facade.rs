//! Protocol 2 interface facades: the fast path for consumer → producer method calls.
//!
//! `__s2_iface_call` re-resolves the interface by name on every call, copies the arguments into
//! Rust and back out, and does the same for the result, which costs about 170 ns for a call that
//! takes and returns a number. A consumer's typed handle is now a facade the prelude builds from the
//! consumer's own contract: one function per method that copies arguments and results field by
//! field in JavaScript and calls the producer's function directly (every context shares one
//! isolate). The strict-copy rules of `docs/PLUGIN_INTEROP.md` still hold, enforced by code that
//! uses only intrinsics captured before any plugin code runs.
//!
//! The host keeps the facade honest in three ways:
//! - **Binding.** A facade holds the producer's functions in a private cell that only
//!   `__s2_iface_bind` fills, after the same checks a `__s2_iface_call` makes. The bootstrap-only
//!   natives here are deleted before plugin code runs, so a plugin can't fill a cell itself.
//! - **Revocation.** Every change that could affect a binding (a publish, a plugin loading or
//!   unloading, imports or contracts changing) bumps an epoch in memory shared with every context.
//!   A facade whose cell is from an older epoch rebinds before calling, so a retired producer is
//!   never called. `sweep` later drops stale cells' references so they don't keep a dead context.
//! - **Shared counters.** The same memory (`crate::interop_slots`) holds the interop depth counter (the 32-call limit) and
//!   the profiler flag. While profiling, facades take the slow native path, which records spans.

use super::*;
use std::cell::RefCell;

use crate::interop_slots::{self as slots, DEPTH, EPOCH, PROFILING};

unsafe extern "C" fn keep(_: *mut std::ffi::c_void, _: usize, _: *mut std::ffi::c_void) {}

thread_local! {
    /// The shared slots as a V8 backing store. The memory is leaked for the thread, so nothing frees it.
    static STORE: v8::SharedRef<v8::BackingStore> = {
        let (ptr, len) = slots::memory();
        // SAFETY: the memory lives for the thread and is only touched on it.
        unsafe { v8::ArrayBuffer::new_backing_store_from_ptr(ptr, len, keep, std::ptr::null_mut()) }.make_shared()
    };
    /// Bound cells, for `sweep`. Weak: a cell must never keep its consumer's context alive.
    static CELLS: RefCell<Vec<v8::Weak<v8::Object>>> = const { RefCell::new(Vec::new()) };
}

fn read(index: usize) -> i32 {
    slots::get(index)
}
fn write(index: usize, value: i32) {
    slots::set(index, value)
}

/// Active interop calls, shared by facades (in JS) and the native paths (in Rust).
pub(super) fn depth() -> u32 {
    read(DEPTH).max(0) as u32
}
pub(super) fn set_depth(value: u32) {
    write(DEPTH, value as i32);
}

/// Invalidate every facade binding. The registries already do this on each change they make;
/// call it for changes made outside them (contracts, publish declarations).
pub(crate) fn revoke() {
    slots::bump_epoch();
}

/// Route facade calls through the native path (which records profiler spans) while profiling.
pub(crate) fn set_profiling(active: bool) {
    write(PROFILING, active as i32);
}

/// Drop the references stale cells hold, so a facade nobody calls doesn't keep a retired
/// producer's context alive. Needs a scope; run at lifecycle boundaries.
pub(super) fn sweep(scope: &mut v8::PinScope) {
    let epoch = read(EPOCH);
    let cells = CELLS.with(|c| std::mem::take(&mut *c.borrow_mut()));
    let mut kept = Vec::with_capacity(cells.len());
    for weak in cells {
        let Some(cell) = weak.to_local(scope) else { continue };
        let current = get(scope, cell, "epoch").and_then(|v| v.int32_value(scope)) == Some(epoch);
        if current {
            kept.push(weak);
            continue;
        }
        let null = v8::null(scope).into();
        for key in ["fns", "copy", "fail", "src"] {
            put(scope, cell, key, null);
        }
        let no = v8::Boolean::new(scope, false).into();
        put(scope, cell, "tracked", no);
    }
    CELLS.with(|c| c.borrow_mut().extend(kept));
}

pub(crate) fn reset() {
    CELLS.with(|c| c.borrow_mut().clear());
    set_depth(0);
    set_profiling(false);
}

fn get<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    key: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let key = v8::String::new(scope, key)?;
    object.get(scope, key.into())
}
/// Cells are objects the prelude made with a prototype chain no plugin can reach, so a plain set
/// can't reach a setter.
fn put(scope: &mut v8::PinScope, object: v8::Local<v8::Object>, key: &str, value: v8::Local<v8::Value>) {
    if let Some(key) = v8::String::new(scope, key) {
        object.set(scope, key.into(), value);
    }
}

const KIT_KEY: &str = "s2script.interop.kit";

/// `__s2_interop_slots() -> Int32Array` — the shared slots. Bootstrap-only.
pub(super) fn s2_interop_slots(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let store = STORE.with(|s| s.clone());
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    if let Some(view) = v8::Int32Array::new(scope, buffer, 0, slots::COUNT) {
        rv.set(view.into());
    }
}

/// `__s2_interop_kit(kit)` — remember this context's copy kit, so a binding can compile the
/// producer's own copier. Bootstrap-only.
pub(super) fn s2_interop_kit(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let kit = args.get(0);
    if !kit.is_object() {
        return;
    }
    let global = scope.get_current_context().global(scope);
    let Some(name) = v8::String::new(scope, KIT_KEY) else { return };
    let key = v8::Private::for_api(scope, Some(name));
    global.set_private(scope, key, kit);
}

/// The plan the prelude compiles a copier from: a compact form of the contract's schema.
fn plan(schema: &crate::interop::Schema) -> serde_json::Value {
    use crate::interop::Schema;
    use serde_json::json;
    match schema {
        Schema::Null => json!(0),
        Schema::Boolean => json!(1),
        Schema::Number => json!(2),
        Schema::String => json!(3),
        Schema::Void => json!(4),
        Schema::EntityRef => json!(5),
        Schema::Literal { value } => json!([6, value]),
        Schema::Array { item } => json!([7, plan(item)]),
        Schema::Object { fields } => json!([
            8,
            fields
                .iter()
                .map(|(key, field)| json!([key, plan(&field.schema), field.optional]))
                .collect::<Vec<_>>()
        ]),
        Schema::Union { variants } => json!([9, variants.iter().map(plan).collect::<Vec<_>>()]),
    }
}

/// `[sha256, [[method, [[argPlan, optional], ...], resultPlan], ...]]`, methods in contract order.
fn contract_plan(contract: &crate::interop::Contract) -> String {
    let methods: Vec<serde_json::Value> = contract
        .metadata
        .methods
        .iter()
        .map(|(name, method)| {
            let args: Vec<serde_json::Value> = method
                .args
                .iter()
                .map(|f| serde_json::json!([plan(&f.schema), f.optional]))
                .collect();
            serde_json::json!([name, args, plan(&method.result)])
        })
        .collect();
    serde_json::json!([contract.sha256, methods]).to_string()
}

/// `__s2_iface_plan(name) -> string | null` — the current plugin's own contract for an import it
/// declared with protocol 2, as a plan; null for anything else. Bootstrap-only.
pub(super) fn s2_iface_plan(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let name = args.get(0).to_rust_string_lossy(scope);
        let Some(consumer) = current_plugin(scope) else { return };
        let contract = PLUGIN_INTEROP.with(|p| p.borrow().get(&consumer).and_then(|m| m.get(&name)).cloned());
        if let Some(contract) = contract {
            if let Some(s) = v8::String::new(scope, &contract_plan(&contract)) {
                rv.set(s.into());
            }
        }
    }));
}

/// `__s2_iface_bind(name, sha256, cell, list)` — run a call's checks once and fill the facade's
/// cell with the producer's functions (in `list`) and copier, or throw the error the call would have thrown.
/// Bootstrap-only.
pub(super) fn s2_iface_bind(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        let sha = args.get(1).to_rust_string_lossy(scope);
        let Ok(cell) = v8::Local::<v8::Object>::try_from(args.get(2)) else { return };
        let Some(consumer) = current_plugin(scope) else {
            throw_named(scope, "InterfaceUnavailable", &name);
            return;
        };
        match IFACES.with(|r| r.borrow().bind_target(&consumer, &name)) {
            crate::interfaces::CallTarget::Unavailable => return throw_named(scope, "InterfaceUnavailable", &name),
            crate::interfaces::CallTarget::VersionMismatch => return throw_named(scope, "InterfaceVersionMismatch", &name),
            crate::interfaces::CallTarget::TypesMismatch => return throw_named(scope, "InterfaceTypesMismatch", &name),
            crate::interfaces::CallTarget::Ok => {}
        }
        let contract = match checked_contract(&consumer, &name) {
            Ok(Some(contract)) => contract,
            Ok(None) => return throw_named(scope, "InterfaceTypesMismatch", &name),
            Err(e) => return throw_named(scope, e, &name),
        };
        // The facade compiled its copiers from this contract; a different one can't be trusted.
        if contract.sha256 != sha {
            return throw_named(scope, "InterfaceTypesMismatch", &name);
        }
        if !live_interop_context(scope, &consumer) {
            return throw_named(scope, "InterfaceUnavailable", &consumer);
        }
        let Some((producer, generation)) = IFACES.with(|r| r.borrow().producer_of(&name)) else {
            return throw_named(scope, "InterfaceUnavailable", &name);
        };
        if !REGISTRY.with(|r| r.borrow().is_live(&producer, generation)) {
            return throw_named(scope, "InterfaceUnavailable", &name);
        }
        let Some(producer_ctx) = PLUGINS.with(|p| p.borrow().get(&producer).map(|pi| pi.context.clone())) else {
            return throw_named(scope, "InterfaceUnavailable", &name);
        };
        let methods: Vec<Option<v8::Global<v8::Function>>> = contract
            .metadata
            .methods
            .keys()
            .map(|m| IFACE_METHODS.with(|mm| mm.borrow().get(&(name.clone(), m.clone())).cloned()))
            .collect();
        let epoch = read(EPOCH);

        // The producer's own kit compiles the copier for arguments, from the producer's own copy of
        // the contract (identical, by hash), so values are checked and built in its realm.
        let Some(kit) = ({
            let ctx = v8::Local::new(scope, &producer_ctx);
            let global = ctx.global(scope);
            v8::String::new(scope, KIT_KEY).and_then(|n| {
                let key = v8::Private::for_api(scope, Some(n));
                global.get_private(scope, key)
            })
        })
        .and_then(|k| v8::Local::<v8::Object>::try_from(k).ok()) else {
            return throw_named(scope, "InterfaceUnavailable", &name);
        };
        let compiled = {
            let ctx = v8::Local::new(scope, &producer_ctx);
            let cs = &mut v8::ContextScope::new(scope, ctx);
            let mut storage = v8::TryCatch::new(cs);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let tc = &mut tc;
            (|| {
                let compile = v8::Local::<v8::Function>::try_from(get(tc, kit, "compile")?).ok()?;
                let plan = v8::String::new(tc, &contract_plan(&contract))?;
                let out = compile.call(tc, kit.into(), &[plan.into()])?;
                let out = v8::Local::<v8::Object>::try_from(out).ok()?;
                let copy = get(tc, out, "copy")?;
                let fail = get(tc, out, "fail")?;
                let src = get(tc, out, "src")?;
                Some((v8::Global::new(tc, copy), v8::Global::new(tc, fail), v8::Global::new(tc, src)))
            })()
        };
        let Some((copy, fail, src)) = compiled else {
            return throw_named(scope, "InterfaceUnavailable", &name);
        };

        // The facade passes the list to fill: one of its own objects, whose prototype chain no plugin
        // can reach, so a missing method reads as undefined.
        let Ok(fns) = v8::Local::<v8::Object>::try_from(args.get(3)) else { return };
        for (i, method) in methods.iter().enumerate() {
            if let Some(method) = method {
                let f = v8::Local::new(scope, method);
                fns.set_index(scope, i as u32, f.into());
            }
        }
        put(scope, cell, "fns", fns.into());
        let copy = v8::Local::new(scope, &copy);
        put(scope, cell, "copy", copy);
        let fail = v8::Local::new(scope, &fail);
        put(scope, cell, "fail", fail);
        let src = v8::Local::new(scope, &src);
        put(scope, cell, "src", src);
        if let Some(p) = v8::String::new(scope, &producer) {
            put(scope, cell, "producer", p.into());
        }
        let g = v8::Number::new(scope, generation as f64);
        put(scope, cell, "generation", g.into());
        let e = v8::Integer::new(scope, epoch);
        put(scope, cell, "epoch", e.into());
        // Track each cell once, however often it rebinds.
        if !get(scope, cell, "tracked").is_some_and(|t| t.is_true()) {
            let yes = v8::Boolean::new(scope, true).into();
            put(scope, cell, "tracked", yes);
            let weak = v8::Weak::new(scope, cell);
            CELLS.with(|c| c.borrow_mut().push(weak));
        }
    }));
}

/// `__s2_iface_check(name, producer, generation) -> bool` — after a call during which something
/// changed: are the caller, and the producer it called, still the live participants? The same
/// condition `__s2_iface_call` checks at its return boundary. Bootstrap-only.
pub(super) fn s2_iface_check(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let name = args.get(0).to_rust_string_lossy(scope);
        let producer = args.get(1).to_rust_string_lossy(scope);
        let Some(generation) = args.get(2).number_value(scope).map(|g| g as u64) else { return };
        let Some(consumer) = current_plugin(scope) else { return };
        rv.set_bool(
            live_interop_context(scope, &consumer)
                && REGISTRY.with(|r| r.borrow().is_live(&producer, generation))
                && IFACES.with(|r| r.borrow().producer_of(&name)) == Some((producer, generation)),
        );
    }));
}

/// `__s2_iface_observe(value)` — mark a thenable result handled before the facade refuses it.
/// Bootstrap-only.
pub(super) fn s2_iface_observe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        observe_thenable(&mut tc, args.get(0));
    }));
}

/// `__s2_is_proxy(value) -> bool`. A strict copy refuses proxies, which only the host can see.
fn s2_is_proxy(
    _scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    rv.set_bool(args.get(0).is_proxy());
}
extern "C" fn s2_is_proxy_fast(_recv: v8::Local<v8::Object>, value: v8::Local<v8::Value>) -> bool {
    value.is_proxy()
}
const IS_PROXY: v8::fast_api::CFunction = v8::fast_api::CFunction::new(
    s2_is_proxy_fast as *const std::ffi::c_void,
    &v8::fast_api::CFunctionInfo::new(
        v8::fast_api::Type::Bool.as_info(),
        &[v8::fast_api::Type::V8Value.as_info(), v8::fast_api::Type::V8Value.as_info()],
        v8::fast_api::Int64Representation::Number,
    ),
);

/// Bootstrap-only globals this module installs; `close_bootstrap_globals` deletes them.
pub(super) const BOOTSTRAP_ONLY: [&str; 7] = [
    "__s2_interop_slots",
    "__s2_interop_kit",
    "__s2_iface_plan",
    "__s2_iface_bind",
    "__s2_iface_check",
    "__s2_iface_observe",
    "__s2_is_proxy",
];

pub(super) fn install(scope: &mut v8::PinScope, global: v8::Local<v8::Object>) {
    set_native(scope, global, "__s2_interop_slots", s2_interop_slots);
    set_native(scope, global, "__s2_interop_kit", s2_interop_kit);
    set_native(scope, global, "__s2_iface_plan", s2_iface_plan);
    set_native(scope, global, "__s2_iface_bind", s2_iface_bind);
    set_native(scope, global, "__s2_iface_check", s2_iface_check);
    set_native(scope, global, "__s2_iface_observe", s2_iface_observe);
    set_fast_native(scope, global, "__s2_is_proxy", s2_is_proxy, &IS_PROXY);
    #[cfg(test)]
    set_native(scope, global, "__s2_test_sweep", |s: &mut v8::PinScope, _a: v8::FunctionCallbackArguments, _r: v8::ReturnValue| {
        crate::interop_slots::bump_epoch();
        sweep(s);
    });
}

/// How many cells `sweep` is tracking.
#[cfg(test)]
pub(crate) fn tracked_cells() -> usize {
    CELLS.with(|c| c.borrow().len())
}

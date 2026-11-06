//! The shared framework realm: one frozen context that holds the stateless parts of a game package
//! (its generated schema and nav accessors) once, for every plugin.
//!
//! Each plugin context used to evaluate the whole package itself, which cost about 11 ms and 1.4 MB
//! per plugin for the schema accessors alone. A package now wraps such a module in
//! `__s2_shared_module(key, factory)`. The first call compiles the factory's source in this realm,
//! runs it once and freezes what it returns; every later call, from any plugin, gets that same
//! object. Plugins call its functions directly, which works because every context lives on one
//! isolate.
//!
//! What may go in a shared module: code with no per-plugin state and no engine side effects. The
//! realm has exactly one native, `__s2_schema_offset`, and no plugin identity, so anything that
//! subscribes, ledgers, or reads `current_plugin` stays in the per-plugin part of the package.
//! Objects a shared module creates belong to this realm, so its accessors build values (such as
//! vectors) through the receiver's own methods to keep them in the plugin's realm.
//!
//! The key names the module and its content hash, which the generator embeds, so a regenerated
//! module gets a fresh instance rather than the old one.

use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// Deep-freeze every builtin reachable from the realm's global object (the global itself stays
/// extensible, but nothing on it is writable afterwards).
const LOCKDOWN: &str = r#"(function () {
  var seen = new Set();
  function freeze(o) {
    if (o === null || (typeof o !== "object" && typeof o !== "function") || seen.has(o)) return;
    seen.add(o);
    Object.freeze(o);
    Reflect.ownKeys(o).forEach(function (k) {
      var d = Object.getOwnPropertyDescriptor(o, k);
      if (!d) return;
      if ("value" in d) freeze(d.value); else { freeze(d.get); freeze(d.set); }
    });
    freeze(Object.getPrototypeOf(o));
  }
  Reflect.ownKeys(globalThis).forEach(function (k) {
    if (k === "globalThis") return;
    var d = Object.getOwnPropertyDescriptor(globalThis, k);
    if (d && "value" in d) freeze(d.value);
    if (d && "value" in d && d.configurable) Object.defineProperty(globalThis, k, { writable: false, configurable: false });
  });
})();"#;

struct Framework {
    context: v8::Global<v8::Context>,
    modules: HashMap<String, v8::Global<v8::Object>>,
}

thread_local! {
    static FRAMEWORK: RefCell<Option<Framework>> = const { RefCell::new(None) };
    /// Context-independent compiled scripts, keyed by source identity (see `compiled`).
    static SCRIPTS: RefCell<HashMap<(usize, usize, u64), v8::Global<v8::UnboundScript>>> =
        RefCell::new(HashMap::new());
}

/// Drop the realm, every shared module, and the compiled-script cache. Runs before the isolate
/// drops.
pub(crate) fn reset() {
    FRAMEWORK.with(|f| f.borrow_mut().take());
    SCRIPTS.with(|s| s.borrow_mut().clear());
}

/// `src` compiled once per isolate and bound to the current context.
///
/// Every plugin context evaluates the same engine prelude and package bootstrap, so they're
/// compiled once and bound to each new context, instead of being copied into a fresh V8 string and
/// parsed again for every plugin. The key is the source's address and length plus `generation`, so
/// callers pass only text that stays alive and unchanged at that address while it is cached: a
/// `&'static str`, or a registered package's source with its generation.
pub(super) fn compiled<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    src: &str,
    generation: u64,
) -> Option<v8::Local<'s, v8::Script>> {
    let key = (src.as_ptr() as usize, src.len(), generation);
    if let Some(unbound) = SCRIPTS.with(|s| s.borrow().get(&key).cloned()) {
        return Some(v8::Local::new(scope, &unbound).bind_to_current_context(scope));
    }
    let code = v8::String::new(scope, src)?;
    let mut source = v8::script_compiler::Source::new(code, None);
    let unbound = v8::script_compiler::compile_unbound_script(
        scope,
        &mut source,
        v8::script_compiler::CompileOptions::NoCompileOptions,
        v8::script_compiler::NoCacheReason::NoReason,
    )?;
    SCRIPTS.with(|s| s.borrow_mut().insert(key, v8::Global::new(scope, unbound)));
    Some(unbound.bind_to_current_context(scope))
}

/// Forget the compiled scripts of a package generation that is being released.
pub(crate) fn forget_generation(generation: u64) {
    SCRIPTS.with(|s| s.borrow_mut().retain(|k, _| k.2 != generation));
}

/// How many compiled scripts are cached.
#[cfg(test)]
pub(crate) fn script_count() -> usize {
    SCRIPTS.with(|s| s.borrow().len())
}

/// How many shared modules are loaded.
#[cfg(test)]
pub(crate) fn module_count() -> usize {
    FRAMEWORK.with(|f| f.borrow().as_ref().map_or(0, |fw| fw.modules.len()))
}

fn framework_context<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Context>> {
    if let Some(ctx) = FRAMEWORK.with(|f| f.borrow().as_ref().map(|fw| fw.context.clone())) {
        return Some(v8::Local::new(scope, &ctx));
    }
    let context = v8::Context::new(scope, Default::default());
    {
        let cs = &mut v8::ContextScope::new(scope, context);
        let global = context.global(cs);
        set_native(cs, global, "__s2_schema_offset", s2_schema_offset);
        // Freeze the realm's intrinsics before any module loads. Shared objects lead back to them
        // (a view's prototype chain ends at this realm's Object.prototype), so without this one
        // plugin could patch a builtin under every other plugin.
        let src = v8::String::new(cs, LOCKDOWN)?;
        v8::Script::compile(cs, src, None)?.run(cs)?;
    }
    FRAMEWORK.with(|f| {
        *f.borrow_mut() = Some(Framework {
            context: v8::Global::new(scope, context),
            modules: HashMap::new(),
        })
    });
    Some(context)
}

/// Compile `factory`'s source in the framework realm, run it, and freeze its result.
fn instantiate<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    factory: v8::Local<v8::Function>,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let source = factory
        .to_string(scope)
        .ok_or("shared module factory has no source")?
        .to_rust_string_lossy(scope);
    let context = framework_context(scope).ok_or("framework realm allocation")?;
    let cs = &mut v8::ContextScope::new(scope, context);
    let mut storage = v8::TryCatch::new(cs);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let thrown = |tc: &mut v8::PinnedRef<v8::TryCatch<v8::HandleScope>>| {
        tc.exception()
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "shared module threw".into())
    };
    let wrapped = v8::String::new(&mut tc, &format!("({source})")).ok_or("source allocation")?;
    let Some(compiled) = v8::Script::compile(&mut tc, wrapped, None).and_then(|s| s.run(&mut tc)) else {
        return Err(thrown(&mut tc));
    };
    let compiled = v8::Local::<v8::Function>::try_from(compiled)
        .map_err(|_| "shared module factory must be a function")?;
    let recv = v8::undefined(&mut tc).into();
    let Some(result) = compiled.call(&mut tc, recv, &[]) else {
        return Err(thrown(&mut tc));
    };
    let object = v8::Local::<v8::Object>::try_from(result)
        .map_err(|_| "shared module must return an object")?;
    object.set_integrity_level(&mut tc, v8::IntegrityLevel::Frozen);
    Ok(v8::Local::new(&mut tc, object))
}

/// `__s2_shared_module(key, factory) -> object` — the framework realm's instance of a shared
/// package module, created on first use. Available only while a package bootstraps.
pub(super) fn s2_shared_module(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let key = args.get(0);
        let Ok(factory) = v8::Local::<v8::Function>::try_from(args.get(1)) else {
            throw_type_error(scope, "__s2_shared_module(key, factory): factory must be a function");
            return;
        };
        if !key.is_string() {
            throw_type_error(scope, "__s2_shared_module(key, factory): key must be a string");
            return;
        }
        let key = key.to_rust_string_lossy(scope);
        let cached = FRAMEWORK.with(|f| {
            f.borrow().as_ref().and_then(|fw| fw.modules.get(&key).cloned())
        });
        if let Some(module) = cached {
            rv.set(v8::Local::new(scope, &module).into());
            return;
        }
        match instantiate(scope, factory) {
            Ok(module) => {
                let global = v8::Global::new(scope, module);
                FRAMEWORK.with(|f| {
                    if let Some(fw) = f.borrow_mut().as_mut() {
                        // A regenerated module replaces the instance of the same name.
                        let name = key.split(':').next().unwrap_or("");
                        fw.modules.retain(|k, _| k.split(':').next().unwrap_or("") != name);
                        fw.modules.insert(key, global);
                    }
                });
                rv.set(module.into());
            }
            Err(error) => {
                let message = format!("shared module '{key}': {error}");
                log_warn(&message);
                throw_error(scope, &message);
            }
        }
    }));
}

fn throw_type_error(scope: &mut v8::PinScope, message: &str) {
    if let Some(m) = v8::String::new(scope, message) {
        let e = v8::Exception::type_error(scope, m);
        scope.throw_exception(e);
    }
}

fn throw_error(scope: &mut v8::PinScope, message: &str) {
    if let Some(m) = v8::String::new(scope, message) {
        let e = v8::Exception::error(scope, m);
        scope.throw_exception(e);
    }
}

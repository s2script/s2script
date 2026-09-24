//! Host-authorized package bootstrap and synchronous scalar adapter dispatch.
//! The public function API and pointer projections remain later Task 6/7 work.
use super::*;
use crate::engine_functions::{
    contract::*,
    registry::{self, Binding},
    runtime::{self, Frame},
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::Arc,
};

struct PreparedPackage {
    owner: HostPackageOwner,
    source: Arc<str>,
    manifest: ImplementationManifestHash,
    #[allow(dead_code)] // Retained payload measurement for Task 7's loader accounting seam.
    retained_bytes: usize,
}
pub(crate) struct PreparedPackageReceipt {
    package: Rc<PreparedPackage>,
}
impl PreparedPackageReceipt {
    pub(crate) fn owner(&self) -> &OwnerKey {
        self.package.owner.key()
    }
}
thread_local! {
    static PACKAGES:RefCell<BTreeMap<u64,Rc<PreparedPackage>>>=const{RefCell::new(BTreeMap::new())};
    static BOOTSTRAP:RefCell<Option<PackageInstanceKey>>=const{RefCell::new(None)};
    static ADAPTERS:RefCell<BTreeMap<u64,Rc<Adapter>>>=const{RefCell::new(BTreeMap::new())};
    static SUBSCRIPTIONS:RefCell<BTreeMap<u64,Rc<Subscription>>>=const{RefCell::new(BTreeMap::new())};
    static SEMANTICS:RefCell<BTreeMap<String,String>>=const{RefCell::new(BTreeMap::new())};
    static AUTHORIZED:RefCell<BTreeMap<u64,(OwnerKey,String,String)>>=const{RefCell::new(BTreeMap::new())};
    static LEASES:RefCell<Vec<Lease>>=const{RefCell::new(Vec::new())};
    static INVOCATIONS:RefCell<BTreeMap<(i64,u64),InvocationState>>=const{RefCell::new(BTreeMap::new())};
}
/// Caller already selected/verified this source's actual manifest. No manifest parser lives here.
pub(crate) fn register_prepared_package(
    owner: HostPackageOwner,
    source: Arc<str>,
    manifest: ImplementationManifestHash,
) -> Result<PreparedPackageReceipt, String> {
    if source.is_empty() {
        return Err("empty prepared package source".into());
    }
    let package = Rc::new(PreparedPackage {
        retained_bytes: source.len() + manifest.as_str().len() + owner.key().id.len(),
        owner,
        source,
        manifest,
    });
    PACKAGES.with(|p| {
        p.borrow_mut()
            .insert(package.owner.key().generation, package.clone())
    });
    Ok(PreparedPackageReceipt { package })
}
impl Drop for PreparedPackageReceipt {
    fn drop(&mut self) {
        PACKAGES.with(|p| p.borrow_mut().remove(&self.owner().generation));
        let ids = ADAPTERS.with(|a| {
            a.borrow()
                .values()
                .filter(|a| a.instance.package_owner == *self.owner())
                .map(|a| a.id)
                .collect::<Vec<_>>()
        });
        for id in ids {
            drop_adapter(id);
        }
    }
}
struct Adapter {
    id: u64,
    instance: PackageInstanceKey,
    semantic: String,
    hash: String,
    package: Rc<PreparedPackage>,
    pre: Option<v8::Global<v8::Function>>,
    post: Option<v8::Global<v8::Function>>,
}
struct Subscription {
    id: u64,
    instance: PackageInstanceKey,
    adapter: String,
    phase: i32,
    binding: Rc<Binding>,
    wrapper: v8::Global<v8::Function>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(t) = self.binding.target {
            runtime::hook_release(t);
        }
    }
}
/// Sealed host-only binding authorization. Community archive validation remains strict.
/// S3's prepared internal input will call this seam; JS cannot grant it to itself.
pub(crate) fn authorize_binding(
    package: &PreparedPackageReceipt,
    owner: &OwnerKey,
    binding_id: u64,
    adapter: &str,
    hash: &str,
) -> Result<(), String> {
    let binding = registry::binding(binding_id, owner)?;
    if binding.target.is_none() {
        return Err("binding unavailable".into());
    }
    ImplementationManifestHash::new(hash.into())?;
    if adapter == "generic.v2" {
        return Err("package cannot replace public generic policy".into());
    }
    let authorization = (package.owner().clone(), adapter.into(), hash.into());
    if SUBSCRIPTIONS.with(|s| s.borrow().values().any(|s| s.binding.id == binding_id))
        && !AUTHORIZED.with(|a| a.borrow().get(&binding_id) == Some(&authorization))
    {
        return Err("cannot change authorization of a subscribed binding".into());
    }
    validate_subscription_domain(&binding, adapter)?;
    AUTHORIZED.with(|a| a.borrow_mut().insert(binding_id, authorization));
    Ok(())
}
fn validate_subscription_domain(binding: &Binding, adapter: &str) -> Result<(), String> {
    let conflict = SUBSCRIPTIONS.with(|s| {
        s.borrow().values().any(|s| {
            s.binding.target == binding.target
                && (s.adapter != adapter
                    || serde_json::to_string(&s.binding.function.abi).ok()
                        != serde_json::to_string(&binding.function.abi).ok())
        })
    });
    if conflict {
        return Err(
            "scalar proof requires one exact adapter/ABI projection domain per target".into(),
        );
    }
    Ok(())
}
fn current_owner(scope: &mut v8::PinScope) -> Result<OwnerKey, String> {
    let ctx = scope.get_current_context();
    let id = ctx
        .get_slot::<PluginId>()
        .ok_or("plugin context required")?
        .0
        .clone();
    let generation = ctx
        .get_slot::<InteropGeneration>()
        .ok_or("context generation required")?
        .0;
    if !owner_is_live(&id, generation) {
        return Err("owner generation unavailable".into());
    }
    if plugin_phase(&id) == Some(plugin::Phase::Unloading) {
        return Err("parent unloading".into());
    }
    Ok(OwnerKey::plugin(&id, generation))
}
fn throw(scope: &mut v8::PinScope, error: impl AsRef<str>) {
    if let Some(text) = v8::String::new(scope, error.as_ref()) {
        let e = v8::Exception::error(scope, text);
        scope.throw_exception(e);
    }
}
fn bigint(value: v8::Local<v8::Value>) -> Result<u64, String> {
    let value = v8::Local::<v8::BigInt>::try_from(value).map_err(|_| "opaque id must be bigint")?;
    let (id, lossless) = value.u64_value();
    if !lossless || id == 0 {
        Err("invalid opaque id".into())
    } else {
        Ok(id)
    }
}
fn get<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    key: &str,
) -> Result<v8::Local<'s, v8::Value>, String> {
    let key = v8::String::new(scope, key).ok_or("string allocation")?;
    object
        .get(scope, key.into())
        .ok_or("property read failed".into())
}
fn set<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    key: &str,
    value: v8::Local<'s, v8::Value>,
) -> Result<(), String> {
    let key = v8::String::new(scope, key).ok_or("string allocation")?;
    if object.set(scope, key.into(), value) == Some(true) {
        Ok(())
    } else {
        Err("property write failed".into())
    }
}
fn sync_function(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
) -> Result<Option<v8::Global<v8::Function>>, String> {
    if value.is_undefined() {
        return Ok(None);
    }
    if value.is_async_function() {
        return Err("adapter callbacks must be synchronous".into());
    }
    let function =
        v8::Local::<v8::Function>::try_from(value).map_err(|_| "callback must be function")?;
    Ok(Some(v8::Global::new(scope, function)))
}
pub(crate) fn bootstrap(scope: &mut v8::PinScope, id: &str, generation: u64) -> Result<(), String> {
    let packages = PACKAGES.with(|p| p.borrow().values().cloned().collect::<Vec<_>>());
    for package in packages {
        let instance = PackageInstanceKey {
            parent: OwnerKey::plugin(id, generation),
            package_owner: package.owner.key().clone(),
        };
        let prior = BOOTSTRAP.with(|b| b.replace(Some(instance)));
        struct Reset(Option<PackageInstanceKey>);
        impl Drop for Reset {
            fn drop(&mut self) {
                BOOTSTRAP.with(|b| b.replace(self.0.take()));
            }
        }
        let _reset = Reset(prior);
        let _busy = crate::dispatch::ParentBusy::enter(id, generation);
        let global = scope.get_current_context().global(scope);
        // Hidden callback data binds BOTH generations; retaining a native from
        // another context/reload cannot acquire that context's package authority.
        let data = v8::Array::new(scope, 2);
        let package_generation = v8::BigInt::new_from_u64(scope, package.owner.key().generation);
        let parent_generation = v8::BigInt::new_from_u64(scope, generation);
        data.set_index(scope, 0, package_generation.into());
        data.set_index(scope, 1, parent_generation.into());
        let register = v8::Function::builder(js_register)
            .data(data.into())
            .build(scope)
            .ok_or("register allocation")?;
        let subscribe = v8::Function::builder(js_subscribe)
            .data(data.into())
            .build(scope)
            .ok_or("subscribe allocation")?;
        set(
            scope,
            global,
            "__s2_function_adapter_register",
            register.into(),
        )?;
        set(
            scope,
            global,
            "__s2_function_adapter_subscribe",
            subscribe.into(),
        )?;
        let mut storage = v8::TryCatch::new(scope);
        let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let source = v8::String::new(&mut tc, &package.source).ok_or("source allocation")?;
        let result = v8::Script::compile(&mut tc, source, None).and_then(|s| s.run(&mut tc));
        let error = if result.is_none() {
            Some(
                tc.exception()
                    .map(|e| e.to_rust_string_lossy(&tc))
                    .unwrap_or("package bootstrap threw".into()),
            )
        } else {
            None
        };
        for name in [
            "__s2_function_adapter_register",
            "__s2_function_adapter_subscribe",
        ] {
            let key = v8::String::new(&mut tc, name).unwrap();
            global.delete(&mut tc, key.into());
        }
        if let Some(error) = error {
            return Err(error);
        }
    }
    Ok(())
}
fn instance(
    scope: &mut v8::PinScope,
    data: v8::Local<v8::Value>,
) -> Result<(PackageInstanceKey, Rc<PreparedPackage>), String> {
    let data =
        v8::Local::<v8::Array>::try_from(data).map_err(|_| "missing host bootstrap token")?;
    let generation = bigint(
        data.get_index(scope, 0)
            .ok_or("missing package generation")?,
    )?;
    let expected_parent = bigint(
        data.get_index(scope, 1)
            .ok_or("missing parent generation")?,
    )?;
    let parent = current_owner(scope)?;
    if parent.generation != expected_parent {
        return Err("package token belongs to another parent generation".into());
    }
    let package = PACKAGES
        .with(|p| p.borrow().get(&generation).cloned())
        .ok_or("package receipt revoked")?;
    Ok((
        PackageInstanceKey {
            parent,
            package_owner: package.owner.key().clone(),
        },
        package,
    ))
}
fn js_register(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| {
        let (instance, package) = instance(scope, args.data())?;
        if BOOTSTRAP.with(|b| b.borrow().as_ref() != Some(&instance)) {
            return Err("host bootstrap authority required".into());
        }
        if !args.get(0).is_string() || !args.get(1).is_string() {
            return Err("adapter id/hash required".into());
        }
        let semantic = args.get(0).to_rust_string_lossy(scope);
        let hash = args.get(1).to_rust_string_lossy(scope);
        ImplementationManifestHash::new(hash.clone())?;
        if semantic.is_empty() || semantic == "generic.v2" {
            return Err("reserved generic policy".into());
        }
        if ADAPTERS.with(|a| {
            a.borrow()
                .values()
                .any(|a| a.instance == instance && a.semantic == semantic)
        }) {
            return Err("duplicate package adapter".into());
        }
        if SEMANTICS.with(|s| s.borrow().get(&semantic).is_some_and(|h| h != &hash)) {
            return Err("semantic contract hash conflict".into());
        }
        let callbacks = v8::Local::<v8::Object>::try_from(args.get(2))
            .map_err(|_| "callbacks object required")?;
        let value = get(scope, callbacks, "pre")?;
        let pre = sync_function(scope, value)?;
        let value = get(scope, callbacks, "post")?;
        let post = sync_function(scope, value)?;
        if pre.is_none() && post.is_none() {
            return Err("adapter requires a synchronous callback".into());
        }
        let id = registry::next_id()?;
        if !record_resource(
            &instance.parent.id,
            instance.parent.generation,
            plugin::Resource::FunctionAdapter(id),
        ) {
            return Err("parent ledger unavailable".into());
        }
        SEMANTICS.with(|s| s.borrow_mut().insert(semantic.clone(), hash.clone()));
        ADAPTERS.with(|a| {
            a.borrow_mut().insert(
                id,
                Rc::new(Adapter {
                    id,
                    instance,
                    semantic,
                    hash,
                    package,
                    pre,
                    post,
                }),
            )
        });
        receipt(scope, id, false)
    })();
    match result {
        Ok(value) => rv.set(value.into()),
        Err(e) => throw(scope, e),
    }
}
fn js_subscribe(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| {
        let (instance, _) = instance(scope, args.data())?;
        let id = bigint(args.get(0))?;
        let binding = registry::binding(id, &instance.parent)?;
        let adapter = args.get(1).to_rust_string_lossy(scope);
        let phase = match args.get(2).to_rust_string_lossy(scope).as_str() {
            "pre" => 0,
            "post" => 1,
            _ => return Err("invalid subscription phase".into()),
        };
        if !binding
            .function
            .policy
            .surfaces
            .iter()
            .any(|s| s == if phase == 0 { "pre" } else { "post" })
        {
            return Err("undeclared subscription surface".into());
        }
        let row = ADAPTERS
            .with(|a| {
                a.borrow()
                    .values()
                    .find(|a| a.instance == instance && a.semantic == adapter)
                    .cloned()
            })
            .ok_or("current package adapter unavailable")?;
        if !AUTHORIZED.with(|a| {
            a.borrow().get(&id).is_some_and(|(p, name, hash)| {
                *p == instance.package_owner && *name == adapter && *hash == row.hash
            })
        }) {
            return Err("host binding authorization required".into());
        }
        if (phase == 0 && row.pre.is_none()) || (phase == 1 && row.post.is_none()) {
            return Err("adapter phase unimplemented".into());
        }
        let wrapper = sync_function(scope, args.get(3))?.ok_or("wrapper required")?;
        let target = binding.target.ok_or("binding unavailable")?;
        // Authorizations may precede all subscriptions. Admission runs on the
        // V8 owner thread with no JS call between this check and insertion;
        // HookAcquire registers the native binding without invoking the target.
        validate_subscription_domain(&binding, &adapter)?;
        runtime::hook_acquire(target)?;
        let id = match registry::next_id() {
            Ok(id) => id,
            Err(e) => {
                runtime::hook_release(target);
                return Err(e);
            }
        };
        let subscription = Rc::new(Subscription {
            id,
            instance: instance.clone(),
            adapter,
            phase,
            binding,
            wrapper,
        });
        if !record_resource(
            &instance.parent.id,
            instance.parent.generation,
            plugin::Resource::FunctionSubscription(id),
        ) {
            return Err("parent ledger unavailable".into());
        }
        SUBSCRIPTIONS.with(|s| s.borrow_mut().insert(id, subscription));
        receipt(scope, id, true)
    })();
    match result {
        Ok(value) => rv.set(value.into()),
        Err(e) => throw(scope, e),
    }
}
fn receipt<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    id: u64,
    subscription: bool,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let object = v8::Object::new(scope);
    let data = v8::Array::new(scope, 2);
    let id = v8::BigInt::new_from_u64(scope, id);
    let kind = v8::Boolean::new(scope, subscription);
    data.set_index(scope, 0, id.into());
    data.set_index(scope, 1, kind.into());
    let dispose = v8::Function::builder(js_dispose)
        .data(data.into())
        .build(scope)
        .ok_or("dispose allocation")?;
    set(scope, object, "dispose", dispose.into())?;
    let getter = v8::Function::builder(js_receipt_status)
        .data(data.into())
        .build(scope)
        .ok_or("status allocation")?;
    let undef = v8::undefined(scope);
    let desc = v8::PropertyDescriptor::new_from_get_set(getter.into(), undef.into());
    let key = v8::String::new(scope, "status").unwrap();
    object.define_property(scope, key.into(), &desc);
    object.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
    Ok(object)
}
fn receipt_data(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
) -> Result<(u64, bool), String> {
    let data = v8::Local::<v8::Array>::try_from(value).map_err(|_| "invalid receipt")?;
    let id = bigint(data.get_index(scope, 0).ok_or("receipt id")?)?;
    let sub = data
        .get_index(scope, 1)
        .ok_or("receipt type")?
        .boolean_value(scope);
    Ok((id, sub))
}
fn receipt_owner(id: u64, sub: bool) -> Option<OwnerKey> {
    if sub {
        SUBSCRIPTIONS.with(|s| s.borrow().get(&id).map(|s| s.instance.parent.clone()))
    } else {
        ADAPTERS.with(|s| s.borrow().get(&id).map(|s| s.instance.parent.clone()))
    }
}
fn js_dispose(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| {
        let (id, sub) = receipt_data(scope, args.data())?;
        let owner = current_owner(scope)?;
        if receipt_owner(id, sub).as_ref() != Some(&owner) {
            return Ok(false);
        }
        release_resource(
            &owner.id,
            owner.generation,
            &if sub {
                plugin::Resource::FunctionSubscription(id)
            } else {
                plugin::Resource::FunctionAdapter(id)
            },
        );
        if sub {
            drop_subscription(id)
        } else {
            drop_adapter(id)
        };
        Ok::<_, String>(true)
    })();
    match result {
        Ok(v) => rv.set_bool(v),
        Err(e) => throw(scope, e),
    }
}
fn js_receipt_status(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let state = receipt_data(scope, args.data())
        .ok()
        .map_or("disposed", |(id, sub)| {
            if !sub {
                return if receipt_owner(id, false).is_some() {
                    "active"
                } else {
                    "disposed"
                };
            }
            let target = SUBSCRIPTIONS.with(|s| s.borrow().get(&id).and_then(|s| s.binding.target));
            match target.map(runtime::status) {
                None => "disposed",
                Some(Err(_)) => "unavailable",
                Some(Ok(status)) => match status.state {
                    1 => "pending",
                    2 => "active",
                    3 => "removing",
                    4 => "disposed",
                    _ => "failed",
                },
            }
        });
    let text = v8::String::new(scope, state).unwrap();
    rv.set(text.into());
}
pub(crate) fn drop_subscription(id: u64) {
    let removed = SUBSCRIPTIONS.with(|s| s.borrow_mut().remove(&id));
    if let Some(target) = removed.as_ref().and_then(|s| s.binding.target) {
        let live_phases = SUBSCRIPTIONS.with(|s| {
            let rows = s.borrow();
            [0, 1].map(|phase| {
                rows.values()
                    .any(|s| s.binding.target == Some(target) && s.phase == phase)
            })
        });
        INVOCATIONS.with(|i| {
            i.borrow_mut().retain(|(t, _), state| {
                if *t != target {
                    return true;
                }
                // Abort even neutral rows after the last target subscription. If
                // only one phase disappears, release that phase's V8 hold while
                // preserving its copied scalar deliveries for the matched peer.
                if !live_phases.iter().any(|live| *live) {
                    return false;
                }
                for (phase, live) in live_phases.iter().enumerate() {
                    if !live {
                        state.adapters[phase] = None;
                    }
                }
                true
            })
        });
    }
    drop(removed);
}
pub(crate) fn drop_adapter(id: u64) {
    let removed = ADAPTERS.with(|a| a.borrow_mut().remove(&id));
    if let Some(adapter) = removed {
        let ids = SUBSCRIPTIONS.with(|s| {
            s.borrow()
                .values()
                .filter(|s| s.instance == adapter.instance && s.adapter == adapter.semantic)
                .map(|s| s.id)
                .collect::<Vec<_>>()
        });
        for id in ids {
            release_resource(
                &adapter.instance.parent.id,
                adapter.instance.parent.generation,
                &plugin::Resource::FunctionSubscription(id),
            );
            drop_subscription(id);
        }
        INVOCATIONS.with(|s| {
            s.borrow_mut().retain(|_, state| {
                let mut removed = false;
                for selected in &mut state.adapters {
                    if selected.as_ref().is_some_and(|a| a.id == adapter.id) {
                        *selected = None;
                        removed = true;
                    }
                }
                !removed || state.adapters.iter().any(Option::is_some)
            })
        });
    }
}
pub(crate) fn drop_owner(owner: &OwnerKey) {
    let adapters = ADAPTERS.with(|a| {
        a.borrow()
            .values()
            .filter(|a| a.instance.parent == *owner)
            .map(|a| a.id)
            .collect::<Vec<_>>()
    });
    for id in adapters {
        drop_adapter(id);
    }
    for id in registry::owner_bindings(owner) {
        AUTHORIZED.with(|a| a.borrow_mut().remove(&id));
    }
}

#[derive(Clone, Copy)]
struct Decision {
    action: i32,
    value: Option<S2FunctionValue>,
}
struct InvocationState {
    // Independently selected PRE/POST instances, pinned to one invocation ID.
    // Copied decisions carry no V8 values and survive either instance's removal.
    adapters: [Option<Rc<Adapter>>; 2],
    deliveries: Vec<Decision>,
    retained_bytes: usize,
}
struct Dispatch {
    frame: Frame,
    binding: Rc<Binding>,
    adapter: Rc<Adapter>,
    subscribers: Vec<Rc<Subscription>>,
    cursor: Cell<usize>,
    revision: Cell<u64>,
    deliveries: RefCell<Vec<Decision>>,
}
#[derive(Clone)]
struct Lease {
    id: u64,
    owner: OwnerKey,
    dispatch: Rc<Dispatch>,
    adapter: bool,
    enabled: bool,
}
struct LeaseGuard;
impl LeaseGuard {
    fn enter(
        owner: OwnerKey,
        dispatch: Rc<Dispatch>,
        adapter: bool,
    ) -> Result<(Self, u64), String> {
        let id = registry::next_id()?;
        LEASES.with(|s| {
            s.borrow_mut().push(Lease {
                id,
                owner,
                dispatch,
                adapter,
                enabled: true,
            })
        });
        Ok((Self, id))
    }
    fn close(&self) {
        LEASES.with(|s| {
            if let Some(top) = s.borrow_mut().last_mut() {
                top.enabled = false;
            }
        });
    }
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        LEASES.with(|s| {
            s.borrow_mut().pop();
        });
    }
}
fn lease(scope: &mut v8::PinScope, id: u64) -> Result<Lease, String> {
    let owner = current_owner(scope)?;
    LEASES
        .with(|s| {
            s.borrow()
                .last()
                .filter(|l| l.enabled && l.id == id && l.owner == owner)
                .cloned()
        })
        .ok_or("expired or suspended callback lease".into())
}
fn accessor_data(
    scope: &mut v8::PinScope,
    args: &v8::FunctionCallbackArguments,
) -> Result<(Lease, i32), String> {
    let a = v8::Local::<v8::Array>::try_from(args.data()).map_err(|_| "invalid accessor")?;
    let id = bigint(a.get_index(scope, 0).ok_or("missing lease")?)?;
    let selector = a
        .get_index(scope, 1)
        .and_then(|v| v.int32_value(scope))
        .ok_or("missing selector")?;
    Ok((lease(scope, id)?, selector))
}
fn scalar_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    v: S2FunctionValue,
) -> Result<v8::Local<'s, v8::Value>, String> {
    Ok(match v.kind {
        0 => v8::undefined(scope).into(),
        1 if v.bits <= 1 => v8::Boolean::new(scope, v.bits == 1).into(),
        2 => v8::Integer::new(scope, v.bits as i32).into(),
        3 => v8::Integer::new_from_unsigned(scope, v.bits as u32).into(),
        4 => v8::BigInt::new_from_i64(scope, v.bits as i64).into(),
        5 => v8::BigInt::new_from_u64(scope, v.bits).into(),
        6 => v8::Number::new(scope, f32::from_bits(v.bits as u32) as f64).into(),
        7 => v8::Number::new(scope, f64::from_bits(v.bits)).into(),
        _ => return Err("unsupported scalar value".into()),
    })
}
fn scalar_from_js(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    kind: u8,
) -> Result<S2FunctionValue, String> {
    let mut out = runtime::blank();
    out.kind = kind;
    out.bits = match kind {
        0 if value.is_undefined() => 0,
        1 if value.is_boolean() => u64::from(value.boolean_value(scope)),
        2 if value.is_int32() => value.int32_value(scope).unwrap() as u32 as u64,
        3 if value.is_uint32() => value.uint32_value(scope).unwrap() as u64,
        4 => {
            let n = v8::Local::<v8::BigInt>::try_from(value).map_err(|_| "i64 requires bigint")?;
            let (n, ok) = n.i64_value();
            if !ok {
                return Err("i64 out of range".into());
            }
            n as u64
        }
        5 => {
            let n = v8::Local::<v8::BigInt>::try_from(value).map_err(|_| "u64 requires bigint")?;
            let (n, ok) = n.u64_value();
            if !ok {
                return Err("u64 out of range".into());
            }
            n
        }
        6 | 7 if value.is_number() => {
            let n = value.number_value(scope).unwrap();
            if !n.is_finite() {
                return Err("finite scalar required".into());
            }
            if kind == 6 {
                let f = n as f32;
                if !f.is_finite() {
                    return Err("f32 out of range".into());
                }
                f.to_bits() as u64
            } else {
                n.to_bits()
            }
        }
        _ => return Err("typed scalar return/value mismatch".into()),
    };
    Ok(out)
}
fn field_kind(dispatch: &Dispatch, selector: i32) -> Result<u8, String> {
    if selector == -2 {
        runtime::kind(&dispatch.binding.function.abi.returns.native)
    } else {
        let p = dispatch
            .binding
            .function
            .abi
            .parameters
            .get(selector as usize)
            .ok_or("unknown field")?;
        runtime::kind(&p.native)
    }
}
fn js_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let result = (|| {
        let (l, i) = accessor_data(scope, &args)?;
        let v = l.dispatch.frame.read(i, field_kind(&l.dispatch, i)?)?;
        scalar_to_js(scope, v)
    })();
    match result {
        Ok(v) => rv.set(v),
        Err(e) => throw(scope, e),
    }
}
fn js_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _: v8::ReturnValue) {
    let result = (|| {
        let (l, i) = accessor_data(scope, &args)?;
        if l.dispatch.frame.phase != 0
            || i < 0
            || !l.dispatch.binding.function.abi.parameters[i as usize]
                .mutable
                .iter()
                .any(|p| p == "pre")
        {
            return Err("undeclared field mutation".into());
        }
        let value = scalar_from_js(scope, args.get(0), field_kind(&l.dispatch, i)?)?;
        l.dispatch.frame.write(i, &value)?;
        l.dispatch.revision.set(
            l.dispatch
                .revision
                .get()
                .checked_add(1)
                .ok_or("frame revision exhausted")?,
        );
        Ok::<_, String>(())
    })();
    if let Err(e) = result {
        throw(scope, e)
    }
}
fn view<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    dispatch: &Dispatch,
    lease: u64,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let object = v8::Object::new(scope);
    let mut fields = dispatch
        .binding
        .function
        .abi
        .parameters
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.clone(), i as i32))
        .collect::<Vec<_>>();
    if dispatch.frame.phase == 1 {
        fields.push(("returnValue".into(), -2));
    }
    for (name, index) in fields {
        let data = v8::Array::new(scope, 2);
        let l = v8::BigInt::new_from_u64(scope, lease);
        let i = v8::Integer::new(scope, index);
        data.set_index(scope, 0, l.into());
        data.set_index(scope, 1, i.into());
        let getter = v8::Function::builder(js_get)
            .data(data.into())
            .build(scope)
            .ok_or("getter allocation")?;
        let setter = v8::Function::builder(js_set)
            .data(data.into())
            .build(scope)
            .ok_or("setter allocation")?;
        let desc = v8::PropertyDescriptor::new_from_get_set(getter.into(), setter.into());
        let key = v8::String::new(scope, &name).unwrap();
        object.define_property(scope, key.into(), &desc);
    }
    if dispatch.frame.phase == 1 {
        let skipped = v8::Boolean::new(scope, dispatch.frame.info.flags & 1 != 0);
        set(scope, object, "skipped", skipped.into())?;
    }
    object.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
    Ok(object)
}
fn decision(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    kind: u8,
    phase: i32,
) -> Result<Decision, String> {
    if observe_thenable(scope, value) {
        return Err("callback returned a promise/thenable".into());
    }
    if phase == 1 {
        if !value.is_undefined() {
            return Err("POST callback must return void".into());
        }
        return Ok(Decision {
            action: 0,
            value: None,
        });
    }
    if value.is_undefined() {
        return Ok(Decision {
            action: 0,
            value: None,
        });
    }
    if value.is_int32() {
        let action = value.int32_value(scope).unwrap();
        if (0..=1).contains(&action) {
            return Ok(Decision {
                action,
                value: None,
            });
        }
        return Err("suppression requires typed decision object".into());
    }
    let object =
        v8::Local::<v8::Object>::try_from(value).map_err(|_| "invalid adapter decision")?;
    let action = get(scope, object, "action")?;
    if !action.is_int32() {
        return Err("suppression action must be an int32".into());
    }
    let action = action
        .int32_value(scope)
        .filter(|a| (2..=3).contains(a))
        .ok_or("invalid suppression action")?;
    let value = get(scope, object, "returnValue")?;
    let value = scalar_from_js(scope, value, kind)?;
    Ok(Decision {
        action,
        value: if kind == 0 { None } else { Some(value) },
    })
}
fn js_cursor(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| -> Result<v8::Local<v8::Value>, String> {
        let l = lease(scope, bigint(args.data())?)?;
        if !l.adapter {
            return Err("cursor requires adapter callback lease".into());
        }
        loop {
            let index = l.dispatch.cursor.get();
            let Some(sub) = l.dispatch.subscribers.get(index).cloned() else {
                return Ok(v8::null(scope).into());
            };
            l.dispatch.cursor.set(index + 1);
            if !SUBSCRIPTIONS.with(|s| s.borrow().contains_key(&sub.id))
                || !owner_is_live(&sub.instance.parent.id, sub.instance.parent.generation)
            {
                continue;
            }
            if sub.instance.parent != l.owner
                && crate::dispatch::parent_busy(
                    &sub.instance.parent.id,
                    sub.instance.parent.generation,
                )
            {
                continue;
            }
            let value = match invoke_wrapper(scope, &l.dispatch, &sub) {
                Ok(value) => value,
                Err(error) => {
                    log_warn(&format!("function subscriber decision: {error}"));
                    Decision {
                        action: 0,
                        value: None,
                    }
                }
            };
            l.dispatch.deliveries.borrow_mut().push(value);
            let out = v8::Object::new(scope);
            let action = v8::Integer::new(scope, value.action);
            set(scope, out, "action", action.into())?;
            if let Some(value) = value.value {
                let value = scalar_to_js(scope, value)?;
                set(scope, out, "returnValue", value)?;
            }
            let revision = v8::Number::new(scope, l.dispatch.revision.get() as f64);
            set(scope, out, "frameRevision", revision.into())?;
            out.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
            return Ok(out.into());
        }
    })();
    match result {
        Ok(v) => rv.set(v),
        Err(e) => throw(scope, e),
    }
}
fn invoke_wrapper(
    parent: &mut v8::PinScope,
    dispatch: &Rc<Dispatch>,
    sub: &Subscription,
) -> Result<Decision, String> {
    let context =
        clone_plugin_context(&sub.instance.parent.id).ok_or("subscriber context unavailable")?;
    let context = v8::Local::new(parent, &context);
    let scope = &mut v8::ContextScope::new(parent, context);
    let mut storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let _busy =
        crate::dispatch::ParentBusy::enter(&sub.instance.parent.id, sub.instance.parent.generation);
    let (guard, id) = LeaseGuard::enter(sub.instance.parent.clone(), dispatch.clone(), false)?;
    let view = view(&mut tc, dispatch, id)?;
    let function = v8::Local::new(&mut tc, &sub.wrapper);
    let recv = v8::undefined(&mut tc);
    let value = function.call(&mut tc, recv.into(), &[view.into()]);
    guard.close();
    let value = value.ok_or("subscriber wrapper threw")?;
    decision(
        &mut tc,
        value,
        runtime::kind(&dispatch.binding.function.abi.returns.native)?,
        dispatch.frame.phase,
    )
}
fn invoke_adapter(parent: &mut v8::PinScope, dispatch: Rc<Dispatch>) -> Result<Decision, String> {
    let adapter = &dispatch.adapter;
    let callback = if dispatch.frame.phase == 0 {
        &adapter.pre
    } else {
        &adapter.post
    };
    let Some(callback) = callback else {
        return Ok(Decision {
            action: 0,
            value: None,
        });
    };
    let context =
        clone_plugin_context(&adapter.instance.parent.id).ok_or("adapter context unavailable")?;
    let context = v8::Local::new(parent, &context);
    let scope = &mut v8::ContextScope::new(parent, context);
    let mut storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let _busy = crate::dispatch::ParentBusy::enter(
        &adapter.instance.parent.id,
        adapter.instance.parent.generation,
    );
    let (guard, id) = LeaseGuard::enter(adapter.instance.parent.clone(), dispatch.clone(), true)?;
    let facade = v8::Object::new(&mut tc);
    let frame = view(&mut tc, &dispatch, id)?;
    set(&mut tc, facade, "frame", frame.into())?;
    let phase = v8::String::new(
        &mut tc,
        if dispatch.frame.phase == 0 {
            "pre"
        } else {
            "post"
        },
    )
    .unwrap();
    set(&mut tc, facade, "phase", phase.into())?;
    let cursor = v8::Object::new(&mut tc);
    let data = v8::BigInt::new_from_u64(&mut tc, id);
    let next = v8::Function::builder(js_cursor)
        .data(data.into())
        .build(&mut tc)
        .ok_or("cursor allocation")?;
    set(&mut tc, cursor, "invokeNext", next.into())?;
    cursor.set_integrity_level(&mut tc, v8::IntegrityLevel::Frozen);
    set(&mut tc, facade, "cursor", cursor.into())?;
    facade.set_integrity_level(&mut tc, v8::IntegrityLevel::Frozen);
    let function = v8::Local::new(&mut tc, callback);
    let recv = v8::undefined(&mut tc);
    let value = function.call(&mut tc, recv.into(), &[facade.into()]);
    guard.close();
    decision(
        &mut tc,
        value.ok_or("adapter threw")?,
        runtime::kind(&dispatch.binding.function.abi.returns.native)?,
        dispatch.frame.phase,
    )
}
fn eligible(adapter: &Adapter, bypass: u64, phase: i32) -> bool {
    let owner = &adapter.instance.parent;
    let implements_phase = if phase == 0 {
        adapter.pre.is_some()
    } else {
        adapter.post.is_some()
    };
    implements_phase
        && owner.generation != bypass
        && owner_is_live(&owner.id, owner.generation)
        && plugin_phase(&owner.id) == Some(plugin::Phase::Active)
        && !crate::dispatch::parent_busy(&owner.id, owner.generation)
}
/// Synchronous, including nested CallbackScope entry. Never queues V8 work.
pub(crate) fn dispatch(target: i64, info: S2FunctionFrameInfo, phase: i32) -> Result<(), String> {
    if target <= 0
        || info.version != 1
        || info.struct_size != 48
        || info.frame_token == 0
        || info.native_epoch == 0
        || info.flags > 1
        || info.parameter_count > 32
        || info.invocation_id == 0
        || ![0, 1].contains(&phase)
    {
        return Err("invalid frame metadata".into());
    }
    let key = (target, info.invocation_id);
    let subscribers = SUBSCRIPTIONS.with(|s| {
        s.borrow()
            .values()
            .filter(|s| {
                s.binding.target == Some(target)
                    // PRE must reserve the matched adapter even for POST-only
                    // subscriptions. Actual delivery is phase-filtered below.
                    && (phase == 0 || s.phase == phase)
                    && s.instance.parent.generation != info.suppressed_owner
                    && !crate::dispatch::parent_busy(
                        &s.instance.parent.id,
                        s.instance.parent.generation,
                    )
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    // Take the exact paired PRE state into this POST stack, keeping its copied
    // deliveries and registration hold alive until POST actually completes.
    let post_state = if phase == 1 {
        INVOCATIONS.with(|i| i.borrow_mut().remove(&key))
    } else {
        None
    };
    let adapter = if phase == 0 {
        let selected = ADAPTERS.with(|a| {
            let rows = a.borrow();
            [0, 1].map(|selection_phase| {
                rows.values()
                    .find(|a| {
                        eligible(a, info.suppressed_owner, selection_phase)
                            && subscribers.iter().any(|s| {
                                s.phase == selection_phase
                                    && s.adapter == a.semantic
                                    && AUTHORIZED.with(|auth| {
                                        auth.borrow()
                                            .get(&s.binding.id)
                                            .is_some_and(|(_, _, hash)| *hash == a.hash)
                                    })
                            })
                    })
                    .cloned()
            })
        });
        let pre = selected[0].clone();
        if INVOCATIONS
            .with(|i| {
                i.borrow_mut().insert(
                    key,
                    InvocationState {
                        adapters: selected,
                        deliveries: Vec::new(),
                        retained_bytes: 0,
                    },
                )
            })
            .is_some()
        {
            return Err("duplicate PRE invocation".into());
        }
        pre
    } else {
        post_state
            .as_ref()
            .and_then(|state| state.adapters[1].clone())
            .filter(|a| {
                ADAPTERS.with(|rows| rows.borrow().contains_key(&a.id))
                    && eligible(a, info.suppressed_owner, 1)
            })
    };
    let Some(adapter) = adapter else {
        return Ok(());
    };
    let subscribers = subscribers
        .into_iter()
        .filter(|s| s.phase == phase)
        .collect::<Vec<_>>();
    let Some(binding) = subscribers
        .iter()
        .find(|s| s.adapter == adapter.semantic)
        .map(|s| s.binding.clone())
    else {
        return Ok(());
    };
    if binding.function.abi.parameters.len() != info.parameter_count as usize {
        return Err("frame parameter count mismatch".into());
    }
    let frame = Frame::validate(target, info, phase, &binding.function.abi.fingerprint)?;
    let dispatch = Rc::new(Dispatch {
        frame,
        binding,
        adapter: adapter.clone(),
        subscribers: subscribers
            .into_iter()
            .filter(|s| s.adapter == adapter.semantic)
            .collect(),
        cursor: Cell::new(0),
        revision: Cell::new(0),
        deliveries: RefCell::new(Vec::new()),
    });
    let result = if let Some(info) = crate::nest::top().filter(|p| !p.is_null()) {
        let mut storage = unsafe { v8::CallbackScope::new(&*info) };
        let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        invoke_adapter(&mut scope, dispatch.clone())
    } else {
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = clone_plugin_context(&adapter.instance.parent.id)
                .ok_or("adapter context unavailable")?;
            let context = v8::Local::new(&mut scope, &context);
            let scope = &mut v8::ContextScope::new(&mut scope, context);
            invoke_adapter(scope, dispatch.clone())
        })
        .map_err(|_| "synchronous host isolate unavailable")?
    };
    let result = result?; // Adapter failure leaves the staged native frame uncommitted.
    if phase == 0 {
        INVOCATIONS.with(|i| {
            if let Some(state) = i.borrow_mut().get_mut(&key) {
                state.deliveries = dispatch.deliveries.borrow().clone();
                state.retained_bytes =
                    state.deliveries.capacity() * std::mem::size_of::<Decision>();
            }
        });
        dispatch
            .frame
            .commit(result.action, result.value.as_ref())?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod proof {
    use super::*;
    pub fn pending_invocations() -> usize {
        INVOCATIONS.with(|i| i.borrow().len())
    }
    pub const SEMANTIC: &str = "proof.scalar.v1";
    pub const HASH: &str = "3e4f14eb33cba81079d21282c1abb09e06542218078a3959317b811f52a3cca9";
    pub fn package() -> PreparedPackageReceipt {
        let manifest = br#"{"name":"@proof/scalar","version":"1.0.0","entry":"adapter.js"}"#;
        let source = format!(
            r#"(()=>{{
            const register=__s2_function_adapter_register, subscribe=__s2_function_adapter_subscribe;
            globalThis.proofEvents=[];
            globalThis.proofReceipt=register('{SEMANTIC}','{HASH}',{{
              pre(d){{
                proofEvents.push('adapter');globalThis.savedAdapter=d.frame;globalThis.savedCursor=d.cursor;
                let best={{action:0}},delivery;
                while((delivery=d.cursor.invokeNext())!==null){{
                  if(delivery.action>best.action)best=delivery;
                  let refused=false;try{{globalThis.savedView.x;}}catch(_){{refused=true;}}
                  if(!refused)throw Error('wrapper lease survived return');
                }}
                return best.action>=2?{{action:best.action,returnValue:best.returnValue}}:best.action;
              }},
              post(d){{proofEvents.push('post');while(d.cursor.invokeNext()!==null){{}}}}
            }});
            globalThis.proofSubscribe=(binding)=>{{
              globalThis.proofBinding=binding;
              globalThis.proofSubscription=subscribe(binding,'{SEMANTIC}','pre',(view)=>{{
                proofEvents.push('wrapper');globalThis.savedView=view;
                let refused=false;try{{globalThis.savedAdapter.x;}}catch(_){{refused=true;}}
                if(!refused)throw Error('adapter lease active during subscriber');
                const inner=typeof __proofCall==='function'?__proofCall(3):3;
                return {{action:2,returnValue:view.x+70+inner}};
              }});
              globalThis.proofPostSubscription=subscribe(binding,'{SEMANTIC}','post',(view)=>{{
                proofEvents.push('post-wrapper:'+view.returnValue);
                let refused=0;try{{view.x=99;}}catch(_){{refused++;}}
                try{{view.returnValue=99;}}catch(_){{refused++;}}
                if(refused!==2)throw Error('POST became writable');
              }});
            }};
            globalThis.proofDuplicate=()=>register('{SEMANTIC}','{HASH}',{{pre(){{}}}});
        }})()"#
        );
        register_prepared_package(
            HostPackageOwner::mint("@proof/scalar").unwrap(),
            source.into(),
            ImplementationManifestHash::new(crate::engine_functions::contract::hash_bytes(
                manifest,
            ))
            .unwrap(),
        )
        .unwrap()
    }
    pub fn bind(package: &PreparedPackageReceipt, id: &str) -> u64 {
        let binding = prepared_binding(id, |_| {});
        let owner = OwnerKey::plugin(id, plugin_generation(id));
        authorize_binding(package, &owner, binding, SEMANTIC, HASH).unwrap();
        eval_in_context(id, &format!("proofSubscribe({binding}n);")).unwrap();
        binding
    }
    pub fn prepared_binding(id: &str, configure: impl FnOnce(&mut serde_json::Value)) -> u64 {
        use crate::engine_functions::{contract, overrides, tests};
        let owner = OwnerKey::plugin(id, plugin_generation(id));
        let mut value = tests::fixture();
        value["ownerId"] = id.into();
        let f = &mut value["functions"][0];
        f["canonicalId"] = format!("{id}::fire").into();
        f["requirement"] = "required".into();
        f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:i32(i32)".into();
        f["abi"]["parameters"] = serde_json::json!([{"name":"x","native":"i32","projection":{"id":"i32","version":1},"mutable":[]}]);
        f["abi"]["returns"] =
            serde_json::json!({"native":"i32","projection":{"id":"i32","version":1}});
        f["policy"]["surfaces"] = serde_json::json!(["call", "pre", "post"]);
        f["policy"]["suppression"] = "generic".into();
        configure(f);
        tests::seal(&mut value);
        let mut summary = tests::summary(&value);
        summary["functions"][0]["suppresses"] =
            (value["functions"][0]["policy"]["suppression"] == "generic").into();
        summary["functions"][0]["mutates"] = value["functions"][0]["abi"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| !p["mutable"].as_array().unwrap().is_empty())
            .into();
        let parsed = contract::parse(
            &value.to_string(),
            id,
            &summary,
            &["engine:calls".into(), "engine:hooks".into()],
        )
        .unwrap();
        let candidate = overrides::prepare(parsed, "actual-proof-archive", vec![]).unwrap();
        let receipt = registry::prepare_owner(owner.clone(), candidate).unwrap();
        registry::activate_owner(receipt).unwrap()[0]
    }
    pub fn counts(owner: &str, generation: u64) -> (usize, usize) {
        let matches = |i: &PackageInstanceKey| i.parent == OwnerKey::plugin(owner, generation);
        (
            ADAPTERS.with(|a| a.borrow().values().filter(|a| matches(&a.instance)).count()),
            SUBSCRIPTIONS.with(|s| s.borrow().values().filter(|s| matches(&s.instance)).count()),
        )
    }
    pub fn provenance(owner: &str) -> (PackageInstanceKey, String) {
        ADAPTERS.with(|a| {
            let a = a.borrow();
            let a = a.values().find(|a| a.instance.parent.id == owner).unwrap();
            (a.instance.clone(), a.package.manifest.as_str().into())
        })
    }
    thread_local! {static RELEASES:Cell<usize>=const{Cell::new(0)};}
    extern "C" fn prepare(
        _: *const i8,
        _: *const i8,
        _: *const i8,
        _: *const i8,
        _: *mut i8,
        _: i32,
    ) -> i64 {
        1
    }
    extern "C" fn acquire(_: i64, _: *mut i8, _: i32) -> i64 {
        2
    }
    extern "C" fn release(_: i64) -> i32 {
        RELEASES.with(|n| n.set(n.get() + 1));
        1
    }
    #[test]
    fn production_bootstrap_authority_and_ledger_teardown_host() {
        init(frame_tests::logger).unwrap();
        RELEASES.with(|n| n.set(0));
        let mut ops = S2EngineOps::default();
        ops.function_prepare = Some(prepare);
        ops.function_hook_acquire = Some(acquire);
        ops.function_hook_release = Some(release);
        ops.function_target_release = Some(release);
        set_engine_ops(Some(ops));
        let package = package();
        frame_tests::load_body("owner-a", "return {};", "{}");
        frame_tests::load_body("owner-b", "return {};", "{}");
        println!("bootstrap log: {:?}", frame_tests::LOG.lock().unwrap());
        let a = plugin_generation("owner-a");
        let b = plugin_generation("owner-b");
        let (pa, ha) = provenance("owner-a");
        let (pb, hb) = provenance("owner-b");
        assert_ne!(pa.parent, pb.parent);
        assert_eq!(pa.package_owner, pb.package_owner);
        assert_eq!(ha, hb);
        assert_eq!(
            ha,
            crate::engine_functions::contract::hash_bytes(
                br#"{"name":"@proof/scalar","version":"1.0.0","entry":"adapter.js"}"#
            )
        );
        assert!(eval_in_context("owner-a", "proofDuplicate()").is_err());
        bind(&package, "owner-a");
        bind(&package, "owner-b");
        assert_eq!(counts("owner-a", a), (1, 2));
        unload_plugin("owner-a");
        assert_eq!(counts("owner-a", a), (0, 0));
        assert_eq!(counts("owner-b", b), (1, 2));
        assert_eq!(RELEASES.with(Cell::get), 3);
        frame_tests::load_body("owner-a", "return {};", "{}");
        assert_ne!(plugin_generation("owner-a"), a);
        bind(&package, "owner-a");
        create_plugin_context("never-active");
        let n = plugin_generation("never-active");
        bind(&package, "never-active");
        unload_plugin("never-active");
        assert_eq!(counts("never-active", n), (0, 0));
        unload_plugin("owner-a");
        unload_plugin("owner-b");
        assert_eq!(RELEASES.with(Cell::get), 12);
        drop(package);
        set_engine_ops(None);
        shutdown();
    }
}

#[cfg(test)]
mod scalar_transport_tests {
    use super::*;
    #[derive(Clone)]
    struct MockFrame {
        id: u64,
        input: S2FunctionValue,
        output: S2FunctionValue,
        action: i32,
    }
    thread_local! {static STACK:RefCell<Vec<MockFrame>>=const{RefCell::new(Vec::new())};}
    extern "C" fn prepare(
        _: *const i8,
        _: *const i8,
        _: *const i8,
        _: *const i8,
        _: *mut i8,
        _: i32,
    ) -> i64 {
        1
    }
    extern "C" fn acquire(_: i64, _: *mut i8, _: i32) -> i64 {
        2
    }
    extern "C" fn release(_: i64) -> i32 {
        1
    }
    extern "C" fn read(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        selector: i32,
        kind: u8,
        out: *mut S2FunctionValue,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        STACK.with(|s| {
            let s = s.borrow();
            let Some(frame) = s.last().filter(|f| f.id == token) else {
                return 0;
            };
            let value = if selector == -2 {
                frame.output
            } else {
                frame.input
            };
            if value.kind != kind {
                return 0;
            }
            unsafe { *out = value };
            1
        })
    }
    extern "C" fn write(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        _: i32,
        value: *const S2FunctionValue,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            let Some(f) = s.last_mut().filter(|f| f.id == token) else {
                return 0;
            };
            f.input = unsafe { *value };
            1
        })
    }
    extern "C" fn commit(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        action: i32,
        value: *const S2FunctionValue,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            let Some(f) = s.last_mut().filter(|f| f.id == token) else {
                return 0;
            };
            f.action = action;
            if !value.is_null() {
                f.output = unsafe { *value };
            }
            1
        })
    }
    extern "C" fn call(
        target: i64,
        owner: u64,
        args: *const S2FunctionValue,
        _: i32,
        out: *mut S2FunctionValue,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        let id = registry::next_id().unwrap();
        let input = unsafe { *args };
        STACK.with(|s| {
            s.borrow_mut().push(MockFrame {
                id,
                input,
                output: input,
                action: 0,
            })
        });
        let mut info = S2FunctionFrameInfo {
            version: 1,
            struct_size: 48,
            frame_token: id,
            native_epoch: id,
            invocation_id: id,
            suppressed_owner: owner,
            parameter_count: 1,
            flags: 0,
        };
        let pre = crate::ffi::s2script_core_dispatch_function(target, &info, 0);
        info.flags = STACK.with(|s| u32::from(s.borrow().last().unwrap().action >= 2));
        let post = crate::ffi::s2script_core_dispatch_function(target, &info, 1);
        let value = STACK.with(|s| s.borrow_mut().pop().unwrap().output);
        unsafe { *out = value };
        i32::from(pre == 1 && post == 1)
    }
    fn js_call(
        scope: &mut v8::PinScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let owner = current_owner(scope).unwrap();
        let mut value = runtime::blank();
        value.kind = 2;
        value.bits = args.get(0).int32_value(scope).unwrap() as u32 as u64;
        let output =
            crate::nest::with_outbound(&args, || runtime::call(1, owner.generation, &[value]))
                .unwrap();
        rv.set_int32(output.bits as i32);
    }
    fn init_transport() {
        init(frame_tests::logger).unwrap();
        let mut ops = S2EngineOps::default();
        ops.function_prepare = Some(prepare);
        ops.function_hook_acquire = Some(acquire);
        ops.function_hook_release = Some(release);
        ops.function_target_release = Some(release);
        ops.function_frame_read = Some(read);
        ops.function_frame_write = Some(write);
        ops.function_frame_commit = Some(commit);
        set_engine_ops(Some(ops));
    }
    fn review_package(body: &str) -> PreparedPackageReceipt {
        let source = format!(
            r#"(()=>{{
            const register=__s2_function_adapter_register,subscribe=__s2_function_adapter_subscribe;
            globalThis.events=[];
            globalThis.subscribeProof=(id,semantic='proof.review.v1',phase='pre')=>subscribe(id,semantic,phase,globalThis.wrapper);
            {body}
        }})()"#
        );
        register_prepared_package(
            HostPackageOwner::mint("@proof/review").unwrap(),
            source.into(),
            ImplementationManifestHash::new(crate::engine_functions::contract::hash_bytes(
                br#"{"name":"@proof/review","entry":"review.js"}"#,
            ))
            .unwrap(),
        )
        .unwrap()
    }
    fn authorize(
        package: &PreparedPackageReceipt,
        id: &str,
        binding: u64,
        semantic: &str,
    ) -> Result<(), String> {
        authorize_binding(
            package,
            &OwnerKey::plugin(id, plugin_generation(id)),
            binding,
            semantic,
            proof::HASH,
        )
    }
    fn open_frame() -> S2FunctionFrameInfo {
        let id = registry::next_id().unwrap();
        let mut input = runtime::blank();
        input.kind = 2;
        input.bits = 7;
        STACK.with(|s| {
            s.borrow_mut().push(MockFrame {
                id,
                input,
                output: input,
                action: 0,
            })
        });
        S2FunctionFrameInfo {
            version: 1,
            struct_size: 48,
            frame_token: id,
            native_epoch: id,
            invocation_id: id,
            suppressed_owner: 0,
            parameter_count: 1,
            flags: 0,
        }
    }
    fn close_frame(info: &S2FunctionFrameInfo) -> MockFrame {
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, info, 1), 1);
        STACK.with(|s| s.borrow_mut().pop().unwrap())
    }
    #[test]
    fn post_only_subscription_delivers_without_pre_and_unload_clears_matched_state() {
        init_transport();
        for pre in ["", "pre(){throw Error('invented PRE delivery');},"] {
            let package = review_package(&format!(
                r#"
                register('proof.review.v1','{}',{{{pre}post(d){{events.push('post');while(d.cursor.invokeNext()!==null){{}}}}}});
                globalThis.wrapper=(view)=>{{events.push('wrapper:'+view.returnValue);}};
            "#,
                proof::HASH
            ));
            frame_tests::load_body("post-only", "return {};", "{}");
            let owner = OwnerKey::plugin("post-only", plugin_generation("post-only"));
            let binding = proof::prepared_binding("post-only", |f| {
                f["policy"]["surfaces"] = serde_json::json!(["call", "post"]);
                f["policy"]["suppression"] = "none".into();
            });
            authorize(&package, "post-only", binding, "proof.review.v1").unwrap();
            eval_in_context(
                "post-only",
                &format!("subscribeProof({binding}n,'proof.review.v1','post');"),
            )
            .unwrap();
            let info = open_frame();
            assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
            assert_eq!(proof::pending_invocations(), 1);
            assert!(INVOCATIONS.with(|s| s.borrow().values().all(|s| s.deliveries.is_empty())));
            eval_in_context(
                "post-only",
                "if(events.length)throw Error('PRE delivery invented');",
            )
            .unwrap();
            let result = close_frame(&info);
            assert_eq!(result.action, 0);
            let delivered=eval_in_context("post-only","if(events.join(',')!=='post,wrapper:7')throw Error('missing POST-only delivery: '+events);");
            assert_eq!(proof::pending_invocations(), 0);
            let unfinished = open_frame();
            assert_eq!(
                crate::ffi::s2script_core_dispatch_function(1, &unfinished, 0),
                1
            );
            unload_plugin("post-only");
            assert_eq!(proof::pending_invocations(), 0);
            assert_eq!(proof::counts("post-only", owner.generation), (0, 0));
            assert_eq!(registry::retained_bytes(&owner), 0);
            close_frame(&unfinished);
            drop(package);
            delivered.unwrap();
        }
        set_engine_ops(None);
        shutdown();
    }
    #[test]
    fn mixed_phase_instances_preserve_post_across_registration_and_retirement_order() {
        init_transport();
        let mut failures = Vec::new();
        for order in [
            [("mixed-a", "pre"), ("mixed-b", "post")],
            [("mixed-b", "post"), ("mixed-a", "pre")],
        ] {
            for retirement in [
                "none",
                "unload-pre",
                "unload-post",
                "dispose-pre",
                "dispose-post",
            ] {
                let package = review_package(&format!(
                    r#"
                    const phase=globalThis.fixturePhase;
                    const callbacks={{}};
                    callbacks[phase]=(d)=>{{
                      if(d.phase!==phase)throw Error('wrong-phase adapter');
                      events.push('adapter:'+phase);
                      while(d.cursor.invokeNext()!==null){{}}
                      return phase==='pre'?0:undefined;
                    }};
                    register('proof.review.v1','{}',callbacks);
                    globalThis.wrapper=(view)=>{{
                      events.push('wrapper:'+phase+(phase==='post'?':'+view.returnValue:''));
                      return phase==='pre'?0:undefined;
                    }};
                "#,
                    proof::HASH
                ));
                for (owner, phase) in order {
                    // Existing host prelude supplies fixture configuration before
                    // actual package bootstrap; it grants no adapter authority.
                    register_injected_package(
                        "@s2script/cs2",
                        &format!("globalThis.fixturePhase='{phase}';"),
                    );
                    frame_tests::load_body(owner, "return {};", "{}");
                    let binding = proof::prepared_binding(owner, |_| {});
                    authorize(&package, owner, binding, "proof.review.v1").unwrap();
                    eval_in_context(owner,&format!("globalThis.mixedSubscription=subscribeProof({binding}n,'proof.review.v1','{phase}');")).unwrap();
                }
                let a = OwnerKey::plugin("mixed-a", plugin_generation("mixed-a"));
                let b = OwnerKey::plugin("mixed-b", plugin_generation("mixed-b"));
                let weak_adapter = |owner: &OwnerKey| {
                    ADAPTERS.with(|rows| {
                        Rc::downgrade(
                            rows.borrow()
                                .values()
                                .find(|row| row.instance.parent == *owner)
                                .unwrap(),
                        )
                    })
                };
                let a_hold = weak_adapter(&a);
                let b_hold = weak_adapter(&b);
                let info = open_frame();
                assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
                eval_in_context("mixed-a","if(events.join(',')!=='adapter:pre,wrapper:pre')throw Error('PRE delivery mismatch');").unwrap();
                eval_in_context(
                    "mixed-b",
                    "if(events.length)throw Error('invented POST delivery during PRE');",
                )
                .unwrap();
                assert_eq!(proof::pending_invocations(), 1);
                assert!(INVOCATIONS
                    .with(|rows| rows.borrow().values().all(|row| row.deliveries.len() == 1)));
                let retained_before =
                    INVOCATIONS.with(|rows| rows.borrow()[&(1, info.invocation_id)].retained_bytes);
                assert!(
                    retained_before > 0,
                    "copied PRE delivery must remain measured"
                );
                match retirement {
                    "unload-pre" => {
                        unload_plugin("mixed-a");
                        assert!(a_hold.upgrade().is_none(), "unloaded PRE adapter retained");
                    }
                    "unload-post" => {
                        unload_plugin("mixed-b");
                        assert!(b_hold.upgrade().is_none(), "unloaded POST adapter retained");
                    }
                    "dispose-pre" => {
                        eval_in_context("mixed-a", "mixedSubscription.dispose();").unwrap()
                    }
                    "dispose-post" => {
                        eval_in_context("mixed-b", "mixedSubscription.dispose();").unwrap()
                    }
                    _ => (),
                }
                if retirement == "unload-pre" || retirement == "dispose-pre" {
                    assert_eq!(
                        a_hold.strong_count(),
                        usize::from(retirement == "dispose-pre"),
                        "PRE phase hold survived retirement"
                    );
                    INVOCATIONS.with(|rows| {
                        let rows = rows.borrow();
                        let row = &rows[&(1, info.invocation_id)];
                        assert!(row.adapters[0].is_none());
                        assert_eq!(row.adapters[1].as_ref().unwrap().instance.parent, b);
                        assert_eq!(row.deliveries.len(), 1);
                        assert_eq!(row.deliveries[0].action, 0);
                        assert_eq!(
                            row.retained_bytes, retained_before,
                            "copied data accounting lost with PRE V8 hold"
                        );
                    });
                }
                if retirement == "unload-post" || retirement == "dispose-post" {
                    assert_eq!(
                        b_hold.strong_count(),
                        usize::from(retirement == "dispose-post"),
                        "POST phase hold survived retirement"
                    );
                    INVOCATIONS.with(|rows| {
                        let rows = rows.borrow();
                        let row = &rows[&(1, info.invocation_id)];
                        assert!(row.adapters[1].is_none());
                        assert_eq!(row.adapters[0].as_ref().unwrap().instance.parent, a);
                    });
                }
                if retirement == "unload-pre" && proof::pending_invocations() != 1 {
                    failures.push(format!(
                        "{order:?}/{retirement}: lost B's matched POST state"
                    ));
                }
                close_frame(&info);
                assert_eq!(proof::pending_invocations(), 0);
                if retirement != "unload-post" {
                    let expected = if retirement == "dispose-post" {
                        ""
                    } else {
                        "adapter:post,wrapper:post:7"
                    };
                    if let Err(error)=eval_in_context("mixed-b",&format!("if(events.join(',')!=='{expected}')throw Error('POST delivery mismatch: '+events);")) {
                        failures.push(format!("{order:?}/{retirement}: {error}"));
                    }
                    unload_plugin("mixed-b");
                }
                if retirement != "unload-pre" {
                    eval_in_context("mixed-a","if(events.join(',')!=='adapter:pre,wrapper:pre')throw Error('wrong-phase PRE callback');").unwrap();
                    unload_plugin("mixed-a");
                }
                assert!(a_hold.upgrade().is_none() && b_hold.upgrade().is_none());
                assert_eq!(proof::counts(&a.id, a.generation), (0, 0));
                assert_eq!(proof::counts(&b.id, b.generation), (0, 0));
                drop(package);
            }
        }
        register_injected_package("@s2script/cs2", "");
        set_engine_ops(None);
        shutdown();
        assert!(
            failures.is_empty(),
            "mixed phase delivery failures: {failures:#?}"
        );
    }
    #[test]
    fn subscription_admission_refuses_preauthorized_conflicting_domains() {
        init_transport();
        let mut admitted = Vec::new();
        for conflict in ["semantic", "mutation", "name"] {
            let package = review_package(&format!(
                r#"
                for(const semantic of ['proof.review.v1','proof.review.other'])
                  register(semantic,'{}',{{pre(d){{while(d.cursor.invokeNext()!==null){{}}return 0;}}}});
                globalThis.wrapper=()=>{{events.push('wrapper');return 0;}};
            "#,
                proof::HASH
            ));
            for id in ["domain-a", "domain-b"] {
                frame_tests::load_body(id, "return {};", "{}");
            }
            let first = proof::prepared_binding("domain-a", |_| {});
            let second = proof::prepared_binding("domain-b", |f| match conflict {
                "mutation" => f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]),
                "name" => f["abi"]["parameters"][0]["name"] = "other".into(),
                _ => (),
            });
            let second_semantic = if conflict == "semantic" {
                "proof.review.other"
            } else {
                "proof.review.v1"
            };
            // Both authorizations precede either subscription; native fingerprints
            // are identical even when the copied wrapper contract differs.
            authorize(&package, "domain-a", first, "proof.review.v1").unwrap();
            authorize(&package, "domain-b", second, second_semantic).unwrap();
            eval_in_context("domain-a", &format!("subscribeProof({first}n);")).unwrap();
            let result = eval_in_context(
                "domain-b",
                &format!("subscribeProof({second}n,'{second_semantic}');"),
            );
            match result {
                Ok(()) => admitted.push(conflict),
                Err(error) => assert!(
                    error.contains("projection domain"),
                    "unexpected admission refusal: {error}"
                ),
            }
            let info = open_frame();
            assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
            close_frame(&info);
            eval_in_context(
                "domain-a",
                "if(events.join(',')!=='wrapper')throw Error('first domain lost');",
            )
            .unwrap();
            unload_plugin("domain-a");
            unload_plugin("domain-b");
            drop(package);
        }
        set_engine_ops(None);
        shutdown();
        assert!(
            admitted.is_empty(),
            "conflicting domains admitted after prior authorization: {admitted:?}"
        );
    }
    #[test]
    fn live_binding_authorization_cannot_change_package_or_contract() {
        init_transport();
        let package = review_package(&format!(
            r#"
            register('proof.review.v1','{}',{{pre(d){{d.cursor.invokeNext();return 0;}}}});
            globalThis.wrapper=()=>{{events.push('wrapper');return 0;}};
        "#,
            proof::HASH
        ));
        frame_tests::load_body("domain-live", "return {};", "{}");
        let binding = proof::prepared_binding("domain-live", |_| {});
        let owner = OwnerKey::plugin("domain-live", plugin_generation("domain-live"));
        authorize(&package, "domain-live", binding, "proof.review.v1").unwrap();
        eval_in_context("domain-live", &format!("subscribeProof({binding}n);")).unwrap();
        let other = review_package("// intentionally no adapters");
        let mut accepted = Vec::new();
        for (label, pkg, semantic, hash) in [
            (
                "semantic",
                &package,
                "proof.review.other",
                proof::HASH.to_string(),
            ),
            ("hash", &package, "proof.review.v1", "a".repeat(64)),
            (
                "package",
                &other,
                "proof.review.v1",
                proof::HASH.to_string(),
            ),
        ] {
            if authorize_binding(pkg, &owner, binding, semantic, &hash).is_ok() {
                accepted.push(label);
            }
        }
        authorize(&package, "domain-live", binding, "proof.review.v1").unwrap(); // Idempotent identity.
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        close_frame(&info);
        eval_in_context(
            "domain-live",
            "if(events.join(',')!=='wrapper')throw Error('authorization changed live delivery');",
        )
        .unwrap();
        unload_plugin("domain-live");
        drop(other);
        drop(package);
        set_engine_ops(None);
        shutdown();
        assert!(
            accepted.is_empty(),
            "live authorization changed: {accepted:?}"
        );
    }
    #[test]
    fn malformed_object_actions_never_suppress_through_subscriber_or_adapter() {
        init_transport();
        let package = review_package(&format!(
            r#"
            register('proof.review.v1','{}',{{pre(d){{
                const delivery=d.cursor.invokeNext();events.push('delivery:'+delivery.action);
                if(mode==='adapter')return {{action:invalidAction,returnValue:73}};
                return delivery.action>=2?{{action:delivery.action,returnValue:delivery.returnValue}}:delivery.action;
            }}}});
            globalThis.wrapper=()=>mode==='subscriber'?{{action:invalidAction,returnValue:73}}:0;
        "#,
            proof::HASH
        ));
        frame_tests::load_body("typed-actions", "return {};", "{}");
        let binding = proof::prepared_binding("typed-actions", |_| {});
        authorize(&package, "typed-actions", binding, "proof.review.v1").unwrap();
        eval_in_context("typed-actions", &format!("subscribeProof({binding}n);")).unwrap();
        let mut failures = Vec::new();
        for mode in ["subscriber", "adapter"] {
            for (label, action) in [
                ("fractional", "2.5"),
                ("string", "'2'"),
                ("valueOf", "({valueOf(){coercions++;return 2;}})"),
            ] {
                eval_in_context("typed-actions",&format!("globalThis.mode='{mode}';globalThis.coercions=0;globalThis.invalidAction={action};events.length=0;")).unwrap();
                let info = open_frame();
                let pre = crate::ffi::s2script_core_dispatch_function(1, &info, 0);
                let result = close_frame(&info);
                let expected_pre = if mode == "subscriber" { 1 } else { 0 };
                if pre != expected_pre || result.action != 0 || result.output.bits != 7 {
                    failures.push(format!(
                        "{mode}/{label}: PRE={pre}, committed action={}, return={}",
                        result.action, result.output.bits
                    ));
                }
                if let Err(e)=eval_in_context("typed-actions","if(coercions!==0)throw Error('action coercion ran');if(events.join(',')!=='delivery:0')throw Error('invalid subscriber decision escaped: '+events);") {
                    failures.push(format!("{mode}/{label}: {e}"));
                }
                assert_eq!(proof::pending_invocations(), 0);
            }
            for action in [2, 3] {
                eval_in_context("typed-actions",&format!("globalThis.mode='{mode}';globalThis.invalidAction={action};events.length=0;")).unwrap();
                let info = open_frame();
                assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
                let result = close_frame(&info);
                assert_eq!(
                    (result.action, result.output.bits),
                    (action, 73),
                    "valid typed {mode} action"
                );
            }
        }
        unload_plugin("typed-actions");
        drop(package);
        set_engine_ops(None);
        shutdown();
        assert!(
            failures.is_empty(),
            "malformed decisions crossed the typed boundary: {failures:#?}"
        );
    }
    #[test]
    fn scalar_transport_mock_proves_real_v8_cursor_leases_and_nested_busy_selection() {
        init(frame_tests::logger).unwrap();
        let mut ops = S2EngineOps::default();
        ops.function_prepare = Some(prepare);
        ops.function_hook_acquire = Some(acquire);
        ops.function_hook_release = Some(release);
        ops.function_target_release = Some(release);
        ops.function_frame_read = Some(read);
        ops.function_frame_write = Some(write);
        ops.function_frame_commit = Some(commit);
        ops.function_call = Some(call);
        set_engine_ops(Some(ops));
        let package = proof::package();
        for id in ["owner-a", "owner-b"] {
            frame_tests::load_body(id, "return {};", "{}");
            proof::bind(&package, id);
            HOST.with(|h| {
                let mut host = h.borrow_mut();
                let host = host.as_mut().unwrap();
                let context = clone_plugin_context(id).unwrap();
                let mut storage = v8::HandleScope::new(&mut host.isolate);
                let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
                let context = v8::Local::new(&mut hs, &context);
                let scope = &mut v8::ContextScope::new(&mut hs, context);
                let function = v8::Function::new(scope, js_call).unwrap();
                let global = context.global(scope);
                set(scope, global, "__proofCall", function.into()).unwrap();
            });
        }
        eval_in_context("owner-a","if(__proofCall(7)!==80)throw Error('wrong typed nested result');if(proofEvents.length!==0)throw Error('busy A invoked');").unwrap();
        eval_in_context("owner-b","if(proofEvents.join(',')!=='adapter,wrapper,post,post-wrapper:80')throw Error('selection order '+proofEvents);let failed=0;try{savedView.x}catch(_){failed++}try{savedCursor.invokeNext()}catch(_){failed++}if(failed!==2)throw Error('stale callback lease');").unwrap();
        assert_eq!(proof::pending_invocations(), 0);
        unload_plugin("owner-a");
        unload_plugin("owner-b");
        drop(package);
        set_engine_ops(None);
        shutdown();
    }
}

//! Host-authorized bootstrap and synchronous scalar/entity policy fan-out.
//! Public activation and the remaining codecs stay in later Task 6/7 work.
use super::*;
use crate::engine_functions::{
    contract::*,
    package_adapter::{self, DispatchAdapter, SubscriberCursor},
    policy::{AdapterContract, SubscriptionMode},
    projection::{self, EntityProjection, ProjectedValue},
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
    owner: OwnerKey,
    instance: Option<PackageInstanceKey>,
    adapter: String,
    contract: AdapterContract,
    mode: SubscriptionMode,
    generic: bool,
    builtin: Option<&'static dyn DispatchAdapter>,
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
    let contract = AdapterContract {
        id: adapter.into(),
        version: 1,
        contract_hash: hash.into(),
    };
    validate_subscription_domain(&binding, &contract, false, SubscriptionMode::Mutating)?;
    AUTHORIZED.with(|a| a.borrow_mut().insert(binding_id, authorization));
    Ok(())
}
fn validate_subscription_domain(
    binding: &Binding,
    contract: &AdapterContract,
    generic: bool,
    mode: SubscriptionMode,
) -> Result<(), String> {
    let conflict = SUBSCRIPTIONS.with(|s| {
        s.borrow()
            .values()
            .find(|s| {
                s.binding.target == binding.target
                    && (!crate::engine_functions::projection::compatible(
                        &s.binding.function.abi,
                        &binding.function.abi,
                    ) || (!(generic && mode == SubscriptionMode::Observe)
                        && !(s.generic && s.mode == SubscriptionMode::Observe)
                        && s.contract != *contract))
            })
            .map(|s| (s.binding.function.canonical_id.clone(), s.contract.clone()))
    });
    if let Some((name, other)) = conflict {
        return Err(format!("incompatible adapter contract or projection domain: {} [{} v{} {}] conflicts with {} [{} v{} {}]", binding.function.canonical_id, contract.id,contract.version,contract.contract_hash,name,other.id,other.version,other.contract_hash));
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
        let mode = SubscriptionMode::for_phase(phase, false)?;
        let contract = AdapterContract {
            id: adapter.clone(),
            version: 1,
            contract_hash: row.hash.clone(),
        };
        let id = insert_subscription(
            instance.parent.clone(),
            Some(instance),
            binding,
            adapter,
            contract,
            mode,
            false,
            None,
            phase,
            wrapper,
        )?;
        receipt(scope, id, true)
    })();
    match result {
        Ok(value) => rv.set(value.into()),
        Err(e) => throw(scope, e),
    }
}
fn insert_subscription(
    owner: OwnerKey,
    instance: Option<PackageInstanceKey>,
    binding: Rc<Binding>,
    adapter: String,
    contract: AdapterContract,
    mode: SubscriptionMode,
    generic: bool,
    builtin: Option<&'static dyn DispatchAdapter>,
    phase: i32,
    wrapper: v8::Global<v8::Function>,
) -> Result<u64, String> {
    let target = binding.target.ok_or("binding unavailable")?;
    // No JavaScript runs between domain validation and admission.
    validate_subscription_domain(&binding, &contract, generic, mode)?;
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
        owner: owner.clone(),
        instance,
        binding,
        adapter,
        contract,
        mode,
        generic,
        builtin,
        phase,
        wrapper,
    });
    if !record_resource(
        &owner.id,
        owner.generation,
        plugin::Resource::FunctionSubscription(id),
    ) {
        return Err("parent ledger unavailable".into());
    }
    SUBSCRIPTIONS.with(|s| s.borrow_mut().insert(id, subscription));
    Ok(id)
}
/// Internal host entry consumed by Task 7; no public global is installed here.
/// Generic subscriptions require only their exact plugin owner, never package authority.
pub(crate) fn subscribe_generic(
    scope: &mut v8::PinScope,
    owner: OwnerKey,
    binding_id: u64,
    phase: i32,
    observe_only: bool,
    wrapper: v8::Global<v8::Function>,
) -> Result<u64, String> {
    if current_owner(scope)? != owner {
        return Err("subscription context owner mismatch".into());
    }
    let binding = registry::binding(binding_id, &owner)?;
    let mode = SubscriptionMode::for_phase(phase, observe_only)?;
    let surface = if phase == 0 { "pre" } else { "post" };
    if !binding
        .function
        .policy
        .surfaces
        .iter()
        .any(|s| s == surface)
    {
        return Err("undeclared subscription surface".into());
    }
    let function = v8::Local::new(scope, &wrapper);
    if function.is_async_function() {
        return Err("synchronous wrapper required".into());
    }
    let contract = AdapterContract::generic(&binding.function.policy)?;
    let implementation = crate::engine_functions::policy::public_adapter(&binding.function.policy)?;
    insert_subscription(
        owner,
        None,
        binding,
        contract.id.clone(),
        contract,
        mode,
        true,
        Some(implementation),
        phase,
        wrapper,
    )
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
        SUBSCRIPTIONS.with(|s| s.borrow().get(&id).map(|s| s.owner.clone()))
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
                    .any(|s| s.binding.target == Some(target) && s.phase == phase && !s.generic)
            })
        });
        let any_live = SUBSCRIPTIONS.with(|s| {
            s.borrow()
                .values()
                .any(|s| s.binding.target == Some(target))
        });
        INVOCATIONS.with(|i| {
            i.borrow_mut().retain(|(t, _), state| {
                if *t != target {
                    return true;
                }
                // Abort even neutral rows after the last target subscription. If
                // only one phase disappears, release that phase's V8 hold while
                // preserving its copied scalar deliveries for the matched peer.
                if !any_live {
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
                .filter(|s| {
                    s.instance.as_ref() == Some(&adapter.instance) && s.adapter == adapter.semantic
                })
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
            s.borrow_mut().retain(|(target, _), state| {
                let mut removed = false;
                for selected in &mut state.adapters {
                    if selected.as_ref().is_some_and(|a| a.id == adapter.id) {
                        *selected = None;
                        removed = true;
                    }
                }
                !removed
                    || state.adapters.iter().any(Option::is_some)
                    || SUBSCRIPTIONS.with(|s| {
                        s.borrow()
                            .values()
                            .any(|s| s.binding.target == Some(*target))
                    })
            })
        });
    }
}
pub(crate) fn drop_owner(owner: &OwnerKey) {
    let subscriptions = SUBSCRIPTIONS.with(|s| {
        s.borrow()
            .values()
            .filter(|s| s.owner == *owner)
            .map(|s| s.id)
            .collect::<Vec<_>>()
    });
    for id in subscriptions {
        drop_subscription(id);
    }
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
    value: Option<ProjectedValue>,
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
    adapter: Option<Rc<Adapter>>,
    subscribers: Vec<Rc<Subscription>>,
    cursor: Cell<usize>,
    revision: Rc<Cell<u64>>,
    edits: Rc<RefCell<std::collections::BTreeMap<i32, (ProjectedValue, String)>>>,
    deliveries: RefCell<Vec<Decision>>,
}
#[derive(Clone)]
struct Lease {
    id: u64,
    owner: OwnerKey,
    dispatch: Rc<Dispatch>,
    binding: Rc<Binding>,
    adapter: bool,
    mode: SubscriptionMode,
    enabled: bool,
}
struct LeaseGuard;
impl LeaseGuard {
    fn enter(
        owner: OwnerKey,
        dispatch: Rc<Dispatch>,
        binding: Rc<Binding>,
        adapter: bool,
        mode: SubscriptionMode,
    ) -> Result<(Self, u64), String> {
        let id = registry::next_id()?;
        LEASES.with(|s| {
            s.borrow_mut().push(Lease {
                id,
                owner,
                dispatch,
                binding,
                adapter,
                mode,
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
fn field_type(binding: &Binding, selector: i32) -> Result<(&str, &str), String> {
    match selector {
        -1 if binding.function.abi.receiver == "entity" => Ok(("ptr", "entity")),
        -2 => {
            let r = &binding.function.abi.returns;
            Ok((&r.native, &r.projection.id))
        }
        i if i >= 0 => {
            let p = binding
                .function
                .abi
                .parameters
                .get(i as usize)
                .ok_or("unknown field")?;
            Ok((&p.native, &p.projection.id))
        }
        _ => Err("unknown field".into()),
    }
}
fn projected_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: ProjectedValue,
) -> Result<v8::Local<'s, v8::Value>, String> {
    match value {
        ProjectedValue::Scalar(value) => scalar_to_js(scope, value),
        ProjectedValue::Entity {
            reference: None, ..
        } => Ok(v8::null(scope).into()),
        ProjectedValue::Entity {
            reference: Some(reference),
            ..
        } => interop_wire::projected_entity_ref(scope, reference)
            .ok_or("captured EntityRef prototype unavailable".into()),
    }
}
fn projected_from_js(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    native: &str,
    projection: &str,
) -> Result<ProjectedValue, String> {
    if let Some(entity) = EntityProjection::parse(projection) {
        let reference = if value.is_null() {
            None
        } else {
            Some(
                interop_wire::strict_entity_reference(scope, value)
                    .ok_or("genuine current-context EntityRef required")?,
            )
        };
        entity.value(reference)
    } else {
        Ok(ProjectedValue::Scalar(scalar_from_js(
            scope,
            value,
            runtime::kind(native)?,
        )?))
    }
}
fn js_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let result = (|| {
        let (l, i) = accessor_data(scope, &args)?;
        let (native, projection) = field_type(&l.binding, i)?;
        let v = l
            .dispatch
            .frame
            .read_requested(i, projection::request(native, projection)?)
            .and_then(|v| projection::decode(v, native, projection))
            .map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))?;
        projected_to_js(scope, v)
    })();
    match result {
        Ok(v) => rv.set(v),
        Err(e) => throw(scope, e),
    }
}
fn js_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _: v8::ReturnValue) {
    let result = (|| {
        let (l, i) = accessor_data(scope, &args)?;
        if !l.mode.writable()
            || l.dispatch.frame.phase != 0
            || i < 0
            || !l.binding.function.abi.parameters[i as usize]
                .mutable
                .iter()
                .any(|p| p == "pre")
        {
            return Err("undeclared field mutation".into());
        }
        let (native, projection) = field_type(&l.binding, i)?;
        let value = projected_from_js(scope, args.get(0), native, projection)
            .map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))?;
        let wire = projection::encode(value)
            .map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))?;
        l.dispatch
            .frame
            .write(i, &wire)
            .map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))?;
        l.dispatch
            .edits
            .borrow_mut()
            .insert(i, (value, l.binding.function.canonical_id.clone()));
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
    binding: &Binding,
    lease: u64,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let object = v8::Object::new(scope);
    let mut fields = binding
        .function
        .abi
        .parameters
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.clone(), i as i32))
        .collect::<Vec<_>>();
    if binding.function.abi.receiver == "entity" {
        fields.push(("self".into(), -1));
    }
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
    native: &str,
    projection: &str,
    phase: i32,
) -> Result<Decision, String> {
    let kind = projection::request(native, projection)?.kind;
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
        if (0..=1).contains(&action) || (kind == 0 && (2..=3).contains(&action)) {
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
    let value = projected_from_js(scope, value, native, projection)?;
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
                || !owner_is_live(&sub.owner.id, sub.owner.generation)
            {
                continue;
            }
            if sub.owner != l.owner
                && crate::dispatch::parent_busy(&sub.owner.id, sub.owner.generation)
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
            if value.action == 3 {
                l.dispatch.cursor.set(l.dispatch.subscribers.len());
            }
            l.dispatch.deliveries.borrow_mut().push(value);
            let out = v8::Object::new(scope);
            let action = v8::Integer::new(scope, value.action);
            set(scope, out, "action", action.into())?;
            if let Some(value) = value.value {
                let value = projected_to_js(scope, value)?;
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
    let context = clone_plugin_context(&sub.owner.id).ok_or("subscriber context unavailable")?;
    let context = v8::Local::new(parent, &context);
    let scope = &mut v8::ContextScope::new(parent, context);
    let mut storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let _busy = crate::dispatch::ParentBusy::enter(&sub.owner.id, sub.owner.generation);
    let (guard, id) = LeaseGuard::enter(
        sub.owner.clone(),
        dispatch.clone(),
        sub.binding.clone(),
        false,
        sub.mode,
    )?;
    let view = view(&mut tc, dispatch, &sub.binding, id)?;
    let function = v8::Local::new(&mut tc, &sub.wrapper);
    let recv = v8::undefined(&mut tc);
    let value = function.call(&mut tc, recv.into(), &[view.into()]);
    guard.close();
    let value = value.ok_or("subscriber wrapper threw")?;
    let decision = decision(
        &mut tc,
        value,
        &sub.binding.function.abi.returns.native,
        &sub.binding.function.abi.returns.projection.id,
        dispatch.frame.phase,
    )?;
    if sub.mode == SubscriptionMode::Observe && decision.action != 0 {
        return Err("observe-only subscriber cannot change the decision".into());
    }
    Ok(decision)
}
fn invoke_adapter(parent: &mut v8::PinScope, dispatch: Rc<Dispatch>) -> Result<Decision, String> {
    let adapter = dispatch
        .adapter
        .as_ref()
        .ok_or("package implementation required")?;
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
    let (guard, id) = LeaseGuard::enter(
        adapter.instance.parent.clone(),
        dispatch.clone(),
        dispatch.binding.clone(),
        true,
        SubscriptionMode::for_phase(dispatch.frame.phase, false)?,
    )?;
    let facade = v8::Object::new(&mut tc);
    let frame = view(&mut tc, &dispatch, &dispatch.binding, id)?;
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
        &dispatch.binding.function.abi.returns.native,
        &dispatch.binding.function.abi.returns.projection.id,
        dispatch.frame.phase,
    )
}
struct GenericCursor<'a, 's, 'i> {
    scope: &'a mut v8::PinScope<'s, 'i>,
    dispatch: Rc<Dispatch>,
}
impl SubscriberCursor for GenericCursor<'_, '_, '_> {
    fn invoke_next(&mut self) -> Result<Option<package_adapter::SubscriberDelivery>, String> {
        loop {
            let index = self.dispatch.cursor.get();
            let Some(sub) = self.dispatch.subscribers.get(index) else {
                return Ok(None);
            };
            self.dispatch.cursor.set(index + 1);
            if !SUBSCRIPTIONS.with(|s| s.borrow().contains_key(&sub.id))
                || !owner_is_live(&sub.owner.id, sub.owner.generation)
                || crate::dispatch::parent_busy(&sub.owner.id, sub.owner.generation)
            {
                continue;
            }
            let decision = match invoke_wrapper(self.scope, &self.dispatch, sub) {
                Ok(value) => value,
                Err(error) => {
                    log_warn(&format!("function subscriber decision: {error}"));
                    Decision {
                        action: 0,
                        value: None,
                    }
                }
            };
            self.dispatch.deliveries.borrow_mut().push(decision);
            let action = match decision.action {
                0 => crate::multiplexer::HookResult::Continue,
                1 => crate::multiplexer::HookResult::Changed,
                2 => crate::multiplexer::HookResult::Handled,
                3 => crate::multiplexer::HookResult::Stop,
                _ => unreachable!("checked callback decision"),
            };
            return Ok(Some(package_adapter::SubscriberDelivery {
                action,
                return_value: decision.value,
                frame_revision: self.dispatch.revision.get(),
            }));
        }
    }
}
fn invoke_domains(scope: &mut v8::PinScope, dispatch: Rc<Dispatch>) -> Result<Decision, String> {
    let mut result = Decision {
        action: 0,
        value: None,
    };
    // One admitted mutating semantic domain, then generic observers. Package
    // POST runs before generic POST, which sees its provider-effective snapshot.
    for group in 0..3 {
        let subscribers = dispatch
            .subscribers
            .iter()
            .filter(|s| match group {
                0 => !s.generic,
                1 => s.generic && s.mode == SubscriptionMode::Mutating,
                _ => s.generic && s.mode == SubscriptionMode::Observe,
            })
            .cloned()
            .collect::<Vec<_>>();
        if subscribers.is_empty() {
            continue;
        }
        let binding = subscribers.first().unwrap().binding.clone();
        let part = Rc::new(Dispatch {
            frame: dispatch.frame.clone(),
            binding,
            adapter: dispatch.adapter.clone(),
            subscribers,
            cursor: Cell::new(0),
            revision: dispatch.revision.clone(),
            edits: dispatch.edits.clone(),
            deliveries: RefCell::new(Vec::new()),
        });
        let decision = if group == 0 {
            invoke_adapter(scope, part.clone())?
        } else {
            let mut cursor = GenericCursor {
                scope,
                dispatch: part.clone(),
            };
            let mut args = package_adapter::AdapterDispatch {
                cursor: &mut cursor,
            };
            if group == 2 || dispatch.frame.phase == 1 {
                let implementation = part.subscribers[0]
                    .builtin
                    .ok_or("missing builtin implementation")?;
                implementation.post(&mut args)?;
                Decision {
                    action: 0,
                    value: None,
                }
            } else {
                let implementation = part.subscribers[0]
                    .builtin
                    .ok_or("missing builtin implementation")?;
                match implementation.pre(&mut args)? {
                    package_adapter::PreDecision::Continue => Decision {
                        action: 0,
                        value: None,
                    },
                    package_adapter::PreDecision::Changed => Decision {
                        action: 1,
                        value: None,
                    },
                    package_adapter::PreDecision::Suppress {
                        action,
                        return_value,
                    } => Decision {
                        action: match action {
                            package_adapter::SuppressAction::Handled => 2,
                            package_adapter::SuppressAction::Stop => 3,
                        },
                        value: return_value,
                    },
                }
            }
        };
        if group != 2 {
            result = decision;
        }
        dispatch
            .deliveries
            .borrow_mut()
            .extend(part.deliveries.borrow().iter().copied());
    }
    Ok(result)
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
    let result = dispatch_inner(target, info, phase);
    #[cfg(test)]
    if let Err(error) = &result {
        proof::record_dispatch_error(error);
    }
    result
}
fn dispatch_inner(target: i64, info: S2FunctionFrameInfo, phase: i32) -> Result<(), String> {
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
                    && plugin_phase(&s.owner.id) == Some(plugin::Phase::Active)
                    && s.owner.generation != info.suppressed_owner
                    && !crate::dispatch::parent_busy(
                        &s.owner.id,
                        s.owner.generation,
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
    let subscribers = subscribers
        .into_iter()
        .filter(|s| s.phase == phase)
        .collect::<Vec<_>>();
    if subscribers.is_empty() {
        return Ok(());
    }
    if subscribers.iter().any(|s| !s.generic) && adapter.is_none() {
        return Err("no eligible synchronous package adapter instance".into());
    }
    // A matching native PRE is required even for generic POST-only subscribers.
    if phase == 1 && post_state.is_none() {
        return Ok(());
    }
    let binding = subscribers[0].binding.clone();
    if binding.function.abi.parameters.len() != info.parameter_count as usize {
        return Err("frame parameter count mismatch".into());
    }
    let frame = Frame::validate(target, info, phase, &binding.function.abi.fingerprint)?;
    let dispatch = Rc::new(Dispatch {
        frame,
        binding,
        adapter: adapter.clone(),
        subscribers,
        cursor: Cell::new(0),
        revision: Rc::new(Cell::new(0)),
        edits: Rc::new(RefCell::new(std::collections::BTreeMap::new())),
        deliveries: RefCell::new(Vec::new()),
    });
    let result = if let Some(info) = crate::nest::top().filter(|p| !p.is_null()) {
        let mut storage = unsafe { v8::CallbackScope::new(&*info) };
        let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        invoke_domains(&mut scope, dispatch.clone())
    } else {
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = clone_plugin_context(&dispatch.binding.owner.id)
                .ok_or("dispatch context unavailable")?;
            let context = v8::Local::new(&mut scope, &context);
            let scope = &mut v8::ContextScope::new(&mut scope, context);
            invoke_domains(scope, dispatch.clone())
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
        // All callbacks have returned. Revalidate copied host identities, then
        // let the native atomic commit independently revalidate current slots.
        let edits = dispatch
            .edits
            .borrow()
            .iter()
            .map(|(selector, (value, name))| {
                projection::encode(*value)
                    .map(|value| (*selector, value, name.clone()))
                    .map_err(|e| format!("{name}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (selector, value, name) in edits {
            dispatch
                .frame
                .write(selector, &value)
                .map_err(|e| format!("{name}: {e}"))?;
        }
        let name = &dispatch.binding.function.canonical_id;
        let value = result
            .value
            .map(projection::encode)
            .transpose()
            .map_err(|e| format!("{name}: {e}"))?;
        dispatch
            .frame
            .commit(result.action, value.as_ref())
            .map_err(|e| format!("{name}: {e}"))?;
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
    fn js_subscribe_generic_test(
        scope: &mut v8::PinScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let result = (|| {
            let owner = current_owner(scope)?;
            if owner.generation != bigint(args.data())? {
                return Err("test owner generation mismatch".into());
            }
            let binding = bigint(args.get(0))?;
            let phase = match args.get(1).to_rust_string_lossy(scope).as_str() {
                "pre" => 0,
                "post" => 1,
                _ => return Err("invalid phase".into()),
            };
            if !args.get(2).is_boolean() {
                return Err("observeOnly must be boolean".into());
            }
            let observe_only = args.get(2).boolean_value(scope);
            let wrapper = sync_function(scope, args.get(3))?.ok_or("wrapper required")?;
            let id = subscribe_generic(scope, owner, binding, phase, observe_only, wrapper)?;
            receipt(scope, id, true)
        })();
        match result {
            Ok(value) => rv.set(value.into()),
            Err(error) => throw(scope, error),
        }
    }
    pub fn install_generic_test_native(id: &str) {
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = clone_plugin_context(id).unwrap();
            let context = v8::Local::new(&mut scope, &context);
            let scope = &mut v8::ContextScope::new(&mut scope, context);
            let data = v8::BigInt::new_from_u64(scope, plugin_generation(id));
            let callback = v8::Function::builder(js_subscribe_generic_test)
                .data(data.into())
                .build(scope)
                .unwrap();
            let global = context.global(scope);
            set(scope, global, "__proofSubscribeGeneric", callback.into()).unwrap();
        })
        .unwrap();
    }
    thread_local! {static DISPATCH_ERRORS:RefCell<Vec<String>>=const{RefCell::new(Vec::new())};}
    pub(super) fn record_dispatch_error(error: &str) {
        DISPATCH_ERRORS.with(|s| s.borrow_mut().push(error.into()));
    }
    fn expect_entity_failure(fragment: &str) {
        let errors = DISPATCH_ERRORS.with(|s| s.borrow_mut().drain(..).collect::<Vec<_>>());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("::fire:") && e.contains(fragment)),
            "expected named {fragment}: {errors:?}"
        );
        println!("PASS expected entity refusal: {}", errors.join("; "));
    }
    pub type EntitySlot = unsafe extern "C" fn(i32, u32, i32) -> i32;
    thread_local! { static ENTITY_SLOT: Cell<Option<EntitySlot>> = const {Cell::new(None)}; }
    fn js_entity_call(
        scope: &mut v8::PinScope,
        args: v8::FunctionCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let result = (|| {
            let owner = current_owner(scope)?;
            let binding = registry::binding(bigint(args.get(0))?, &owner)?;
            let abi = &binding.function.abi;
            let receiver = usize::from(abi.receiver == "entity");
            if args.length() as usize != 1 + receiver + abi.parameters.len() {
                return Err("argument count mismatch".into());
            }
            let mut values = Vec::new();
            for i in 0..receiver + abi.parameters.len() {
                let (native, projection) = if receiver == 1 && i == 0 {
                    ("ptr", "entity")
                } else {
                    let p = &abi.parameters[i - receiver];
                    (p.native.as_str(), p.projection.id.as_str())
                };
                values.push(
                    projected_from_js(scope, args.get((i + 1) as i32), native, projection)
                        .map_err(|e| format!("{}: {e}", binding.function.canonical_id))?,
                );
            }
            let value =
                crate::nest::with_outbound(&args, || runtime::call_binding(&binding, &values))?;
            projected_to_js(scope, value)
        })();
        match result {
            Ok(v) => rv.set(v),
            Err(e) => throw(scope, e),
        }
    }
    fn js_entity_delete(
        scope: &mut v8::PinScope,
        args: v8::FunctionCallbackArguments,
        _: v8::ReturnValue,
    ) {
        let result: Result<(), String> = (|| {
            if !args.get(0).is_int32() || !args.get(1).is_uint32() {
                return Err("fixture identity required".into());
            }
            let index = args.get(0).int32_value(scope).unwrap();
            let serial = args.get(1).uint32_value(scope).unwrap();
            if !matches!(index, 901 | 902) {
                return Err("fixture slot refused".into());
            }
            let mode = args.get(2).to_rust_string_lossy(scope);
            if !matches!(mode.as_str(), "native" | "books" | "both") {
                return Err("fixture deletion mode refused".into());
            }
            if mode != "native" {
                crate::entity_live::on_deleted(index, serial as i32);
            }
            if mode != "books" {
                assert_eq!(
                    unsafe { ENTITY_SLOT.with(Cell::get).unwrap()(index, serial, 0) },
                    1
                );
            }
            Ok(())
        })();
        if let Err(e) = result {
            throw(scope, e)
        }
    }
    fn entity_native(id: &str) {
        install_generic_test_native(id);
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = clone_plugin_context(id).unwrap();
            let context = v8::Local::new(&mut hs, &context);
            let scope = &mut v8::ContextScope::new(&mut hs, context);
            let call = v8::Function::new(scope, js_entity_call).unwrap();
            let delete = v8::Function::new(scope, js_entity_delete).unwrap();
            let global = context.global(scope);
            set(scope, global, "__proofEntityCall", call.into()).unwrap();
            set(scope, global, "__proofEntityDelete", delete.into()).unwrap();
        })
        .unwrap();
    }
    fn entity_binding(id: &str, nullable: bool, writable: bool, receiver: bool) -> u64 {
        prepared_binding(id, |f| {
            f["target"]["pattern"] = if receiver { "51" } else { "50" }.into();
            f["abi"]["receiver"] = if receiver { "entity" } else { "none" }.into();
            f["abi"]["fingerprint"] = if receiver {
                "linux-x86_64-sysv:entity:i32(i32)"
            } else {
                "linux-x86_64-sysv:none:ptr(ptr)"
            }
            .into();
            let projection = if nullable { "entity?" } else { "entity" };
            f["abi"]["parameters"][0]["name"] =
                if nullable { "optional" } else { "required" }.into();
            f["abi"]["parameters"][0]["native"] = if receiver { "i32" } else { "ptr" }.into();
            f["abi"]["parameters"][0]["projection"]["id"] =
                if receiver { "i32" } else { projection }.into();
            f["abi"]["parameters"][0]["mutable"] = if writable {
                serde_json::json!(["pre"])
            } else {
                serde_json::json!([])
            };
            f["abi"]["returns"]["native"] = if receiver { "i32" } else { "ptr" }.into();
            f["abi"]["returns"]["projection"]["id"] =
                if receiver { "i32" } else { projection }.into();
        })
    }
    /// Nullable writes/decisions must not lend their nullability to strict siblings.
    /// Run in the shared proof so these assertions also exercise the real Service.
    fn entity_null_conformance(reverse_readers: bool) {
        const WRITER: &str = "entity-null-writer";
        frame_tests::load_body(WRITER, "return {};", "{}");
        entity_native(WRITER);
        let writer = entity_binding(WRITER, true, true, false);
        eval_in_context(
            WRITER,
            &format!(
                r#"
            globalThis.nullMode='read';globalThis.writeAttempts=0;
            globalThis.nullWriter=__proofSubscribeGeneric({writer}n,'pre',false,v=>{{
                if(nullMode==='edit'){{writeAttempts++;v.optional=null;}}
                if(nullMode==='suppress'){{writeAttempts++;return {{action:2,returnValue:null}};}}
                return 0;
            }});
        "#
            ),
        )
        .unwrap();
        // The new writer holds the existing native target while readers change.
        for id in ["entity-strict", "entity-nullable"] {
            eval_in_context(id, "pre.dispose();post.dispose();globalThis.nullSeen=[];").unwrap();
        }
        eval_in_context("entity-strict", r#"
            globalThis.strictNullMode='read';globalThis.strictNullAttempts=0;globalThis.strictNullRefused=0;
            globalThis.strictNullVote=__proofSubscribeGeneric(binding,'pre',false,v=>{
                if(strictNullMode==='write'){
                    strictNullAttempts++;
                    try{v.required=null}catch(e){
                        if(!String(e).includes('entity-strict::fire')||!String(e).includes('strict entity'))throw e;
                        strictNullRefused++;
                    }
                    if(v.required.id!==a.id)throw Error('strict null write changed field');
                }
                if(strictNullMode==='suppress'){strictNullAttempts++;return {action:2,returnValue:null};}
                return 0;
            });
        "#).unwrap();
        let reader_order = if reverse_readers {
            ["entity-nullable", "entity-strict"]
        } else {
            ["entity-strict", "entity-nullable"]
        };
        for id in reader_order {
            let source = if id == "entity-strict" {
                r#"
                globalThis.pre=__proofSubscribeGeneric(binding,'pre',true,v=>{
                    try{nullSeen.push('pre:'+v.required.id)}catch(e){
                        if(!String(e).includes('entity-strict::fire')||!String(e).includes('strict entity'))throw e;
                        nullSeen.push('pre:error');
                    }
                    let denied=false;try{v.required=a}catch(_){denied=true}
                    if(!denied)throw Error('strict observer acquired mutation rights');
                    nullSeen.push('readonly');
                });
                globalThis.post=__proofSubscribeGeneric(binding,'post',true,v=>{
                    try{nullSeen.push('post:'+v.returnValue.id+':'+v.skipped)}catch(e){
                        if(!String(e).includes('entity-strict::fire')||!String(e).includes('strict entity'))throw e;
                        nullSeen.push('post:error:'+v.skipped);
                    }
                });
            "#
            } else {
                r#"
                globalThis.pre=__proofSubscribeGeneric(binding,'pre',true,v=>{
                    const value=v.optional;nullSeen.push(value===null?'pre:null':'pre:'+value.id);
                    let denied=false;try{v.optional=a}catch(_){denied=true}
                    if(!denied)throw Error('nullable observer acquired mutation rights');
                    nullSeen.push('readonly');
                });
                globalThis.post=__proofSubscribeGeneric(binding,'post',true,v=>{
                    const value=v.returnValue;nullSeen.push('post:'+(value===null?'null':value.id)+':'+v.skipped);
                });
            "#
            };
            eval_in_context(id, source).unwrap();
        }
        for (writer_mode, strict_mode, expected_strict, expected_nullable, returns_null) in [
            (
                "edit",
                "read",
                "['pre:error','readonly','post:error:false']",
                "['pre:null','readonly','post:null:false']",
                true,
            ),
            (
                "suppress",
                "read",
                "['pre:'+a.id,'readonly','post:error:true']",
                "['pre:'+a.id,'readonly','post:null:true']",
                true,
            ),
            (
                "read",
                "write",
                "['pre:'+a.id,'readonly','post:'+a.id+':false']",
                "['pre:'+a.id,'readonly','post:'+a.id+':false']",
                false,
            ),
            (
                "read",
                "suppress",
                "['pre:'+a.id,'readonly','post:'+a.id+':false']",
                "['pre:'+a.id,'readonly','post:'+a.id+':false']",
                false,
            ),
            (
                "read",
                "read",
                "['pre:'+a.id,'readonly','post:'+a.id+':false']",
                "['pre:'+a.id,'readonly','post:'+a.id+':false']",
                false,
            ),
        ] {
            eval_in_context(
                WRITER,
                &format!("nullMode='{writer_mode}';writeAttempts=0;"),
            )
            .unwrap();
            eval_in_context("entity-strict", &format!("strictNullMode='{strict_mode}';strictNullAttempts=0;strictNullRefused=0;nullSeen.length=0;")).unwrap();
            eval_in_context("entity-nullable", "nullSeen.length=0;").unwrap();
            let check = if returns_null {
                "if(call(a)!==null)throw Error('nullable null result lost');"
            } else {
                "if(call(a)?.id!==a.id)throw Error('strict refusal or live recovery failed');"
            };
            eval_in_context("entity-caller", check).unwrap();
            eval_in_context(
                WRITER,
                &format!(
                    "if(writeAttempts!=={})throw Error('nullable writer not exercised');",
                    u8::from(writer_mode != "read")
                ),
            )
            .unwrap();
            for (id, expected) in [
                ("entity-strict", expected_strict),
                ("entity-nullable", expected_nullable),
            ] {
                eval_in_context(id, &format!("if(JSON.stringify(nullSeen)!==JSON.stringify({expected}))throw Error('null projection sequence: '+nullSeen);")).unwrap();
            }
            eval_in_context("entity-strict", &format!("if(strictNullAttempts!=={}||strictNullRefused!=={})throw Error('strict null refusal not exercised');",u8::from(strict_mode!="read"),u8::from(strict_mode=="write"))).unwrap();
            println!("PASS shared V8 null rights writer={writer_mode} strict={strict_mode} reader-order={reader_order:?}");
        }
        eval_in_context("entity-strict", "strictNullVote.dispose();").unwrap();
        unload_plugin(WRITER);
    }
    /// Same real V8/projection path for host mock and native Service fixture.
    /// Slot controls are fixture authority, never public EntityRef identities.
    pub fn entity_conformance(nullable_first: bool, reverse_sub: bool, slot: EntitySlot) {
        ENTITY_SLOT.with(|s| s.set(Some(slot)));
        DISPATCH_ERRORS.with(|s| s.borrow_mut().clear());
        let seed = |index, serial| {
            assert_eq!(unsafe { slot(index, serial, 1) }, 1);
            crate::entity_live::on_created(index, serial as i32)
        };
        let a = seed(901, 71);
        let b = seed(902, 72);
        for id in [
            "entity-strict",
            "entity-nullable",
            "entity-caller",
            "entity-member",
        ] {
            frame_tests::load_body(id, "return {};", "{}");
            entity_native(id);
            eval_in_context(id,&format!("globalThis.E=__s2pkg_entity.EntityRef;globalThis.a=new E(901,{a});globalThis.b=new E(902,{b});globalThis.mode='read';globalThis.seen=[];globalThis.phaseCounts={{pre:0,post:0}};")).unwrap();
        }
        let order = if nullable_first {
            ["entity-nullable", "entity-strict"]
        } else {
            ["entity-strict", "entity-nullable"]
        };
        let mut bindings = std::collections::BTreeMap::new();
        for id in order {
            bindings.insert(
                id,
                entity_binding(id, id == "entity-nullable", id == "entity-strict", false),
            );
        }
        let strict = bindings["entity-strict"];
        let nullable = bindings["entity-nullable"];
        let lookup = |id: &str, binding| {
            registry::binding(binding, &OwnerKey::plugin(id, plugin_generation(id))).unwrap()
        };
        assert_eq!(
            lookup("entity-strict", strict).target,
            lookup("entity-nullable", nullable).target
        );
        for (id, binding) in [("entity-strict", strict), ("entity-nullable", nullable)] {
            eval_in_context(id,&format!("globalThis.binding={binding}n;globalThis.call=(v)=>__proofEntityCall(binding,v);if(call(a).id!==a.id)throw Error('call identity');")).unwrap();
        }
        eval_in_context("entity-strict",r#"{
            let hits=0;const getter=Object.create(E.prototype,{index:{get(){hits++;return 901}},id:{value:a.id}});
            const proxy=new Proxy(a,{get(){hits++;throw Error('proxy trap')}});
            for(const bad of [null,{}, {index:901,id:a.id},getter,proxy,3,'x']) {
                let refused=false;try{call(bad)}catch(e){refused=String(e).includes('entity-strict::fire')}
                if(!refused)throw Error('strict input accepted');
            }
            if(hits)throw Error('getter/proxy executed');
            __s2pkg_entity.EntityRef=function(){throw Error('mutable constructor used')};
            if(!(call(a) instanceof E))throw Error('captured prototype lost');
        }"#).unwrap();
        eval_in_context(
            "entity-nullable",
            "if(call(null)!==null)throw Error('nullable null');",
        )
        .unwrap();
        // Native/Rust books diverge in both directions; later valid calls recover.
        assert_eq!(unsafe { slot(901, 71, 0) }, 1);
        eval_in_context("entity-strict","let stale=false;try{call(a)}catch(e){stale=String(e).includes('entity-strict::fire')}if(!stale)throw Error('native stale strict');").unwrap();
        eval_in_context(
            "entity-nullable",
            "if(call(a)!==null)throw Error('native stale nullable');",
        )
        .unwrap();
        assert_eq!(unsafe { slot(901, 71, 1) }, 1);
        crate::entity_live::on_deleted(901, 71);
        eval_in_context("entity-strict","let books=false;try{call(a)}catch(e){books=String(e).includes('entity-strict::fire')}if(!books)throw Error('book stale strict');").unwrap();
        eval_in_context(
            "entity-nullable",
            "if(call(a)!==null)throw Error('book stale nullable');",
        )
        .unwrap();
        let a = seed(901, 71);
        for id in [
            "entity-strict",
            "entity-nullable",
            "entity-caller",
            "entity-member",
        ] {
            eval_in_context(id, &format!("globalThis.a=new E(901,{a});")).unwrap();
        }
        let caller = entity_binding("entity-caller", true, false, false);
        eval_in_context(
            "entity-caller",
            &format!(
                "globalThis.binding={caller}n;globalThis.call=v=>__proofEntityCall(binding,v);"
            ),
        )
        .unwrap();
        let suborder = if reverse_sub {
            ["entity-nullable", "entity-strict"]
        } else {
            ["entity-strict", "entity-nullable"]
        };
        for id in suborder {
            let code = if id == "entity-strict" {
                r#"
                globalThis.pre=__proofSubscribeGeneric(binding,'pre',false,v=>{
                    phaseCounts.pre++;globalThis.saved=v;let value;try{value=v.required}catch(e){if(!String(e).includes('entity-strict::fire'))throw e;seen.push('strict-error');return 0;}
                    seen.push(value.id);globalThis.copy=value;
                    if(mode==='edit')v.required=b;
                    if(mode==='stale-edit'){v.required=b;__proofEntityDelete(902,72,'native');}
                    if(mode==='stale-books-edit'){v.required=b;__proofEntityDelete(902,72,'books');}
                    if(mode==='unadoptable')__proofEntityDelete(901,71,'books');
                    if(mode==='suppress')return {action:2,returnValue:b};
                    if(mode==='bad-return')return {action:2,returnValue:17};
                    return 0;
                });
                globalThis.post=__proofSubscribeGeneric(binding,'post',true,v=>{phaseCounts.post++;try{
                    seen.push('post:'+v.returnValue.id);
                    if(globalThis.capturePost)postSnapshots.push([v.required.index,v.required.id,v.returnValue.index,v.returnValue.id,v.skipped]);
                }catch(e){if(!String(e).includes('entity-strict::fire'))throw e;seen.push('post-error')}});
            "#
            } else {
                r#"
                globalThis.pre=__proofSubscribeGeneric(binding,'pre',true,v=>{
                    phaseCounts.pre++;globalThis.saved=v;seen.push(v.optional===null?'null':v.optional.id);
                    if(mode==='unadoptable')__proofEntityDelete(901,71,'books');
                    let refused=false;try{v.optional=b}catch(_){refused=true}if(!refused)throw Error('observer mutated');return 0;
                });
                globalThis.post=__proofSubscribeGeneric(binding,'post',true,v=>{
                    phaseCounts.post++;seen.push(v.returnValue===null?'post:null':'post:'+v.returnValue.id);
                    if(globalThis.capturePost)postSnapshots.push([v.optional.index,v.optional.id,v.returnValue.index,v.returnValue.id,v.skipped]);
                });
            "#
            };
            eval_in_context(id, code).unwrap();
        }
        // Diagnose registration observation without changing the Pending lifecycle.
        let target = lookup("entity-strict", strict)
            .target
            .expect("prepared entity target");
        let diagnostic = |stage| {
            let status = match runtime::status(target) {
                Ok(status) => format!(
                    "state={} receipt={} reserved={}",
                    status.state, status.receipt, status.reserved
                ),
                Err(error) => format!("status unavailable: {error}"),
            };
            let snapshot = |id| {
                frame_tests::eval_in_context_string(
                    id,
                    "JSON.stringify({seen,pre:phaseCounts.pre,post:phaseCounts.post})",
                )
            };
            let detail = format!(
                "entity order nullable-first={nullable_first} reverse-sub={reverse_sub} stage={stage} target={target} {status} strict={} nullable={}",
                snapshot("entity-strict"), snapshot("entity-nullable")
            );
            println!("DIAG {detail}");
            detail
        };
        diagnostic("subscribed");
        for (stage, code) in [
            ("call(a)", "if(call(a).id!==a.id)throw Error('callback identity');"),
            ("call(null)", "if(call(null)!==null)throw Error('null callback');"),
        ] {
            let result = eval_in_context("entity-caller", code);
            let detail = diagnostic(stage);
            result.unwrap_or_else(|error| panic!("{error}; {detail}"));
        }
        let detail = diagnostic("before strict assertions");
        eval_in_context("entity-strict","if(!seen.includes('strict-error')||!seen.includes('post-error'))throw Error('strict null local errors');let lease=false;try{saved.required}catch(_){lease=true}if(!lease)throw Error('lease survived');if(copy.id!==a.id)throw Error('copy lost');seen.length=0;mode='edit';").unwrap_or_else(|error| panic!("{error}; {detail}"));
        eval_in_context("entity-nullable", "seen.length=0;").unwrap();
        eval_in_context(
            "entity-caller",
            "if(call(a).id!==b.id)throw Error('edit not recalled by original');",
        )
        .unwrap();
        eval_in_context("entity-nullable",&format!("if(seen[0]!=={})throw Error('per-wrapper edit order');if(seen[1]!=='post:'+b.id)throw Error('final observer return');",b)).unwrap();
        eval_in_context("entity-strict", "mode='suppress';").unwrap();
        eval_in_context(
            "entity-caller",
            "if(call(a).id!==b.id)throw Error('typed entity suppression');",
        )
        .unwrap();
        eval_in_context("entity-strict", "mode='bad-return';").unwrap();
        eval_in_context(
            "entity-caller",
            "if(call(a).id!==a.id)throw Error('invalid entity suppression committed');",
        )
        .unwrap();
        let assert_stale_call = |mode: &str| {
            eval_in_context("entity-strict", &format!("mode='{mode}';")).unwrap();
            for id in ["entity-strict", "entity-nullable"] {
                eval_in_context(
                    id,
                    "globalThis.capturePost=true;globalThis.postSnapshots=[];",
                )
                .unwrap();
            }
            eval_in_context("entity-caller",r#"{
                let failure;
                try{call(a)}catch(e){failure=String(e)}
                if(failure!=='Error: entity-caller::fire: synchronous core function dispatch failed')
                    throw Error('expected exact invocation-local caller error, got: '+failure);
            }"#).unwrap();
            expect_entity_failure("entity-strict::fire: strict entity projection is null or stale");
            for id in ["entity-strict", "entity-nullable"] {
                eval_in_context(id,r#"
                    if(JSON.stringify(postSnapshots)!==JSON.stringify([[a.index,a.id,a.index,a.id,false]]))
                        throw Error('failed edit changed original/POST or skipped delivery: '+JSON.stringify(postSnapshots));
                    capturePost=false;
                "#).unwrap();
            }
            assert_eq!(
                pending_invocations(),
                0,
                "failed call must drain its matched invocation"
            );
            println!("PASS {mode}: exact caller error, independent strict refusal, original POST argument/result and skipped=false, paired state drained");
        };
        let assert_recovered_call = |mode: &str| {
            eval_in_context("entity-strict", "mode='read';").unwrap();
            eval_in_context(
                "entity-caller",
                "if(call(a)?.id!==a.id)throw Error('failed invocation poisoned later valid call');",
            )
            .unwrap();
            assert_eq!(pending_invocations(), 0);
            println!("PASS {mode}: later valid call on the same bindings recovered");
        };
        assert_stale_call("stale-edit");
        assert_eq!(unsafe { slot(902, 72, 1) }, 1);
        assert_recovered_call("native-slot-only");
        assert_stale_call("stale-books-edit");
        let b = seed(902, 72);
        for id in ["entity-strict", "entity-nullable", "entity-caller"] {
            eval_in_context(id, &format!("globalThis.b=new E(902,{b});")).unwrap();
        }
        assert_recovered_call("host-books-only");
        eval_in_context("entity-strict", "mode='unadoptable';").unwrap();
        eval_in_context(
            "entity-caller",
            "if(call(a)!==null)throw Error('unadoptable nullable return');",
        )
        .unwrap();
        let a = seed(901, 71);
        for id in [
            "entity-strict",
            "entity-nullable",
            "entity-caller",
            "entity-member",
        ] {
            eval_in_context(id, &format!("globalThis.a=new E(901,{a});mode='read';")).unwrap();
        }
        eval_in_context(
            "entity-caller",
            "if(call(a).id!==a.id)throw Error('adoption did not recover');",
        )
        .unwrap();
        eval_in_context("entity-nullable", "mode='unadoptable';").unwrap();
        eval_in_context("entity-strict","{let failed=false;try{call(a)}catch(e){failed=String(e).includes('entity-strict::fire')}if(!failed)throw Error('strict unadoptable return accepted');}").unwrap();
        let a = seed(901, 71);
        for id in [
            "entity-strict",
            "entity-nullable",
            "entity-caller",
            "entity-member",
        ] {
            eval_in_context(id, &format!("globalThis.a=new E(901,{a});mode='read';")).unwrap();
        }
        eval_in_context(
            "entity-strict",
            "if(call(a).id!==a.id)throw Error('strict output failure poisoned binding');",
        )
        .unwrap();
        entity_null_conformance(reverse_sub);
        // Reused engine index/serial produces a fresh host identity. Old refs stay stale.
        crate::entity_live::on_deleted(902, 72);
        let replacement = seed(902, 73);
        eval_in_context("entity-strict","{let failed=false;try{call(b)}catch(_){failed=true}if(!failed)throw Error('old ref survived reuse');}").unwrap();
        eval_in_context(
            "entity-nullable",
            "if(call(b)!==null)throw Error('nullable reused ref');",
        )
        .unwrap();
        for id in ["entity-strict", "entity-nullable", "entity-caller"] {
            eval_in_context(id, &format!("globalThis.b=new E(902,{replacement});")).unwrap();
        }
        eval_in_context(
            "entity-strict",
            "if(call(b).id!==b.id)throw Error('new serial did not adopt');",
        )
        .unwrap();
        let member = entity_binding("entity-member", false, false, true);
        eval_in_context("entity-member",&format!("if(__proofEntityCall({member}n,a,3)!==13)throw Error('receiver convention');let refused=false;try{{__proofEntityCall({member}n,null,3)}}catch(_){{refused=true}}if(!refused)throw Error('nullable receiver');")).unwrap();
        eval_in_context("entity-member",&format!(r#"
            globalThis.memberPre=__proofSubscribeGeneric({member}n,'pre',true,v=>{{
                globalThis.memberView=v;if(v.self.id!==a.id||v.required!==3)throw Error('receiver view');
                let refused=false;try{{v.self=b}}catch(_){{refused=true}}if(!refused)throw Error('receiver writable');seen.push('receiver');
            }});
            globalThis.memberPost=__proofSubscribeGeneric({member}n,'post',true,v=>{{if(v.self.id!==a.id||v.returnValue!==13)throw Error('receiver POST');seen.push('receiver-post')}});
        "#)).unwrap();
        let member_binding = lookup("entity-member", member);
        let receiver = projection::encode(
            EntityProjection::Strict
                .value(Some(projection::EntityReference { index: 901, id: a }))
                .unwrap(),
        )
        .unwrap();
        let mut arg = runtime::blank();
        arg.kind = 2;
        arg.bits = 3;
        assert_eq!(
            runtime::call(member_binding.target.unwrap(), 0, &[receiver, arg])
                .unwrap()
                .bits,
            13
        );
        eval_in_context("entity-member","if(seen.join(',')!=='receiver,receiver-post')throw Error('receiver callback not entered');let expired=false;try{memberView.self}catch(_){expired=true}if(!expired)throw Error('receiver lease survived');").unwrap();
        for id in [
            "entity-strict",
            "entity-nullable",
            "entity-caller",
            "entity-member",
        ] {
            unload_plugin(id);
        }
        crate::entity_live::on_deleted(901, 71);
        crate::entity_live::on_deleted(902, 73);
        assert_eq!(pending_invocations(), 0);
        ENTITY_SLOT.with(|s| s.set(None));
        println!("PASS entity projection orders nullable-first={nullable_first} reverse-sub={reverse_sub}: calls, V8 authenticity, per-wrapper rights, recall, suppression, stale commit and adoption recovery");
    }
    /// Run with the caller's transport: host tests install the explicit scalar
    /// mock; the Linux outer-frame fixture supplies the real Service/strong export.
    pub fn policy_conformance(after_observer: impl FnOnce()) {
        const POLICY: &str = "proof.policy.v1";
        let source = format!(
            r#"(()=>{{
            const subscribe=__s2_function_adapter_subscribe;
            __s2_function_adapter_register('{POLICY}','{HASH}',{{
                pre(d){{
                    let best={{action:0}},next;
                    while((next=d.cursor.invokeNext())!==null){{if(next.action>best.action)best=next;}}
                    return best.action>=2?{{action:best.action,returnValue:best.returnValue}}:best.action;
                }},
                post(d){{while(d.cursor.invokeNext()!==null){{}}}}
            }});
            globalThis.policySubscribe=(binding,phase,wrapper)=>subscribe(binding,'{POLICY}',phase,wrapper);
            globalThis.policyEvents=[];
        }})()"#
        );
        let package = register_prepared_package(
            HostPackageOwner::mint("@proof/policy").unwrap(),
            source.into(),
            ImplementationManifestHash::new(crate::engine_functions::contract::hash_bytes(
                br#"{"name":"@proof/policy","entry":"policy.js","version":"1.0.0"}"#,
            ))
            .unwrap(),
        )
        .unwrap();
        frame_tests::load_body("policy-observer", "return {};", "{}");
        install_generic_test_native("policy-observer");
        let observer = prepared_binding("policy-observer", |f| {
            f["abi"]["parameters"][0]["name"] = "observed".into();
            f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
        });
        eval_in_context(
            "policy-observer",
            &format!(
                r#"
            __proofSubscribeGeneric({observer}n,'pre',true,v=>{{
                policyEvents.push('pre:'+v.observed);
                if('x' in v)throw Error('mutator field leaked');
                let denied=false;try{{v.observed=999;}}catch(_){{denied=true;}}
                if(!denied)throw Error('observer acquired mutation authority');
                globalThis.expired=v;
                return {{action:3,returnValue:999}};
            }});
            __proofSubscribeGeneric({observer}n,'post',false,v=>{{
                policyEvents.push('post:'+v.returnValue);
                let denied=false;try{{v.observed=999;}}catch(_){{denied=true;}}
                if(!denied)throw Error('POST acquired mutation authority');
            }});
        "#
            ),
        )
        .unwrap();
        after_observer();
        for id in ["policy-writer", "policy-reader"] {
            frame_tests::load_body(id, "return {};", "{}");
        }
        let writer = prepared_binding("policy-writer", |f| {
            f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
        });
        let reader = prepared_binding("policy-reader", |f| {
            f["abi"]["parameters"][0]["name"] = "renamed".into();
        });
        for (id, binding) in [("policy-writer", writer), ("policy-reader", reader)] {
            authorize_binding(
                &package,
                &OwnerKey::plugin(id, plugin_generation(id)),
                binding,
                POLICY,
                HASH,
            )
            .unwrap();
        }
        eval_in_context("policy-writer", &format!(r#"
            policySubscribe({writer}n,'pre',v=>{{v.x=41;policyEvents.push('write:'+v.x);return 1;}});
        "#)).unwrap();
        eval_in_context("policy-reader", &format!(r#"
            policySubscribe({reader}n,'pre',v=>{{
                policyEvents.push('read:'+v.renamed);
                if('x' in v)throw Error('writer field leaked');
                let denied=false;try{{v.renamed=999;}}catch(_){{denied=true;}}
                if(!denied)throw Error('writer rights leaked');
                return {{action:3,returnValue:73}};
            }});
            policySubscribe({reader}n,'pre',()=>{{policyEvents.push('after-stop');return 0;}});
            policySubscribe({reader}n,'post',v=>{{policyEvents.push('post:'+v.renamed+':'+v.returnValue);}});
        "#)).unwrap();
        let binding = registry::binding(
            observer,
            &OwnerKey::plugin("policy-observer", plugin_generation("policy-observer")),
        )
        .unwrap();
        let mut input = runtime::blank();
        input.kind = 2;
        input.bits = 7;
        assert_eq!(
            runtime::call(binding.target.unwrap(), 0, &[input])
                .unwrap()
                .bits,
            73
        );
        eval_in_context(
            "policy-writer",
            "if(policyEvents.join(',')!=='write:41')throw Error(policyEvents);",
        )
        .unwrap();
        eval_in_context(
            "policy-reader",
            "if(policyEvents.join(',')!=='read:41,post:41:73')throw Error(policyEvents);",
        )
        .unwrap();
        eval_in_context("policy-observer","if(policyEvents.join(',')!=='pre:41,post:73')throw Error(policyEvents);let denied=false;try{expired.observed}catch(_){denied=true}if(!denied)throw Error('expired observer lease');policyEvents.length=0;").unwrap();
        for id in ["policy-writer", "policy-reader"] {
            unload_plugin(id);
        }
        assert_eq!(pending_invocations(), 0);
        // Keep the physical target subscribed while switching its semantic domain.
        frame_tests::load_body("policy-generic", "return {};", "{}");
        install_generic_test_native("policy-generic");
        let generic = prepared_binding("policy-generic", |f| {
            f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
        });
        eval_in_context("policy-generic", &format!(r#"
            globalThis.stopEnabled=false;
            __proofSubscribeGeneric({generic}n,'pre',false,v=>{{v.x=12;return {{action:2,returnValue:31}};}});
            __proofSubscribeGeneric({generic}n,'pre',false,v=>{{if(v.x!==12)throw Error('edit lost');return {{action:2,returnValue:32}};}});
            __proofSubscribeGeneric({generic}n,'pre',false,v=>stopEnabled?{{action:3,returnValue:33}}:0);
            __proofSubscribeGeneric({generic}n,'pre',false,v=>{{policyEvents.push('tail');return 0;}});
        "#)).unwrap();
        assert_eq!(
            runtime::call(binding.target.unwrap(), 0, &[input])
                .unwrap()
                .bits,
            31
        );
        eval_in_context("policy-observer","if(policyEvents.join(',')!=='pre:12,post:31')throw Error(policyEvents);policyEvents.length=0;").unwrap();
        eval_in_context("policy-generic","if(policyEvents.join(',')!=='tail')throw Error(policyEvents);policyEvents.length=0;stopEnabled=true;").unwrap();
        assert_eq!(
            runtime::call(binding.target.unwrap(), 0, &[input])
                .unwrap()
                .bits,
            33
        );
        eval_in_context(
            "policy-observer",
            "if(policyEvents.join(',')!=='pre:12,post:33')throw Error(policyEvents);",
        )
        .unwrap();
        eval_in_context(
            "policy-generic",
            "if(policyEvents.length)throw Error('Stop did not end domain');",
        )
        .unwrap();
        unload_plugin("policy-generic");
        unload_plugin("policy-observer");
        drop(binding);
        drop(package);
        assert_eq!(pending_invocations(), 0);
        println!("PASS production policy fanout: per-wrapper names/rights, shared edits, named Stop plus generic observers, first typed return at strength, generic Stop and independent teardown");
    }
    pub fn counts(owner: &str, generation: u64) -> (usize, usize) {
        let matches = |i: &PackageInstanceKey| i.parent == OwnerKey::plugin(owner, generation);
        (
            ADAPTERS.with(|a| a.borrow().values().filter(|a| matches(&a.instance)).count()),
            SUBSCRIPTIONS.with(|s| {
                s.borrow()
                    .values()
                    .filter(|s| s.instance.as_ref().is_some_and(&matches))
                    .count()
            }),
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
        ops.function_call = Some(call);
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
        for conflict in ["semantic", "projection"] {
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
                "projection" => {
                    f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:i32(u32)".into();
                    f["abi"]["parameters"][0]["native"] = "u32".into();
                    f["abi"]["parameters"][0]["projection"]["id"] = "u32".into();
                }
                _ => (),
            });
            let second_semantic = if conflict == "semantic" {
                "proof.review.other"
            } else {
                "proof.review.v1"
            };
            // Both authorizations precede either subscription. The mock target id
            // also exercises a conflicting native fingerprint at admission.
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
                    error.contains("projection domain")
                        && error.contains("domain-a::fire")
                        && error.contains("domain-b::fire"),
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
    fn policy_conformance_real_v8_mock_transport() {
        init_transport();
        proof::policy_conformance(|| {});
        set_engine_ops(None);
        shutdown();
    }
    #[test]
    fn generic_invalid_votes_continue_and_expired_views_fail_after_await() {
        init_transport();
        frame_tests::load_body("generic-invalid", "return {};", "{}");
        proof::install_generic_test_native("generic-invalid");
        let binding = proof::prepared_binding("generic-invalid", |f| {
            f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
        });
        eval_in_context("generic-invalid", &format!(r#"
            globalThis.expiredReads=0;globalThis.expiredWrites=0;globalThis.coercions=0;
            __proofSubscribeGeneric({binding}n,'pre',false,v=>{{
                globalThis.expired=v;
                (async()=>{{await 0;
                    try{{v.x;}}catch(_){{expiredReads++;}}
                    try{{v.x=88;}}catch(_){{expiredWrites++;}}
                }})();
                return invalidVote;
            }});
            __proofSubscribeGeneric({binding}n,'pre',false,v=>{{return {{action:2,returnValue:37}};}});
        "#)).unwrap();
        for vote in [
            "2",
            "{action:2}",
            "{action:3,returnValue:'37'}",
            "{action:2.5,returnValue:90}",
            "{action:'2',returnValue:90}",
            "{action:{valueOf(){coercions++;return 2;}},returnValue:90}",
            "Promise.resolve(0)",
        ] {
            eval_in_context(
                "generic-invalid",
                &format!("globalThis.invalidVote={vote};"),
            )
            .unwrap();
            let info = open_frame();
            assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
            let result = close_frame(&info);
            assert_eq!(
                (result.action, result.output.bits, result.input.bits),
                (2, 37, 7),
                "{vote}"
            );
        }
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            scope.perform_microtask_checkpoint();
        })
        .unwrap();
        eval_in_context("generic-invalid","if(expiredReads!==7||expiredWrites!==7||coercions!==0)throw Error([expiredReads,expiredWrites,coercions]);").unwrap();
        assert!(eval_in_context(
            "generic-invalid",
            &format!("__proofSubscribeGeneric({binding}n,'pre',false,async()=>0);")
        )
        .is_err());
        unload_plugin("generic-invalid");
        set_engine_ops(None);
        shutdown();
    }
    #[test]
    fn generic_observer_survives_named_pre_owner_unload_between_phases() {
        init_transport();
        let package = review_package(&format!(
            r#"
            register('proof.review.v1','{}',{{pre(d){{d.cursor.invokeNext();return 0;}}}});
            globalThis.wrapper=()=>0;
        "#,
            proof::HASH
        ));
        frame_tests::load_body("retire-named", "return {};", "{}");
        frame_tests::load_body("retire-observer", "return {};", "{}");
        let named = proof::prepared_binding("retire-named", |_| {});
        authorize(&package, "retire-named", named, "proof.review.v1").unwrap();
        eval_in_context("retire-named", &format!("subscribeProof({named}n);")).unwrap();
        let observer = proof::prepared_binding("retire-observer", |_| {});
        proof::install_generic_test_native("retire-observer");
        eval_in_context("retire-observer",&format!("__proofSubscribeGeneric({observer}n,'post',false,v=>events.push(v.returnValue) && undefined);")).unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        unload_plugin("retire-named");
        assert_eq!(proof::pending_invocations(), 1);
        close_frame(&info);
        eval_in_context(
            "retire-observer",
            "if(events.join(',')!=='7')throw Error(events);",
        )
        .unwrap();
        assert_eq!(proof::pending_invocations(), 0);
        unload_plugin("retire-observer");
        drop(package);
        set_engine_ops(None);
        shutdown();
    }
    #[test]
    fn generic_fanout_folds_ties_stops_its_domain_and_observes_final_edits() {
        init_transport();
        frame_tests::load_body("generic-owner", "return {};", "{}");
        proof::install_generic_test_native("generic-owner");
        let binding = proof::prepared_binding("generic-owner", |f| {
            f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
        });
        eval_in_context("generic-owner", &format!(r#"
            globalThis.events=[];
            __proofSubscribeGeneric({binding}n,'pre',true,v=>{{
                events.push('observer:'+v.x);
                let denied=false;try{{v.x=99;}}catch(_){{denied=true;}}
                if(!denied)throw Error('observer inherited mutation rights');
                return {{action:3,returnValue:999}};
            }});
            __proofSubscribeGeneric({binding}n,'pre',false,v=>{{v.x=12;events.push('first');return {{action:2,returnValue:31}};}});
            __proofSubscribeGeneric({binding}n,'pre',false,v=>{{events.push('second:'+v.x);return {{action:2,returnValue:32}};}});
            __proofSubscribeGeneric({binding}n,'pre',false,v=>{{events.push('third');return {{action:3,returnValue:33}};}});
            __proofSubscribeGeneric({binding}n,'pre',false,v=>{{throw Error('ran after domain Stop');}});
            __proofSubscribeGeneric({binding}n,'post',false,v=>{{events.push('post:'+v.returnValue);}});
        "#)).unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        let result = close_frame(&info);
        assert_eq!(
            (result.action, result.output.bits, result.input.bits),
            (3, 33, 12)
        );
        eval_in_context("generic-owner", "if(events.join(',')!=='first,second:12,third,observer:12,post:33')throw Error(events);").unwrap();
        unload_plugin("generic-owner");
        set_engine_ops(None);
        shutdown();
    }
    #[test]
    fn compatible_scalar_bindings_keep_each_wrappers_fields_and_rights() {
        init_transport();
        let package = review_package(&format!(
            r#"
            register('proof.review.v1','{}',{{pre(d){{
                while(d.cursor.invokeNext()!==null){{}}
                return 1;
            }}}});
        "#,
            proof::HASH
        ));
        for name in ["rights-writer", "rights-reader"] {
            frame_tests::load_body(name, "return {};", "{}");
        }
        let writer = proof::prepared_binding("rights-writer", |f| {
            f["abi"]["parameters"][0]["mutable"] = serde_json::json!(["pre"]);
        });
        let reader = proof::prepared_binding("rights-reader", |f| {
            f["abi"]["parameters"][0]["name"] = "renamed".into();
        });
        authorize(&package, "rights-writer", writer, "proof.review.v1").unwrap();
        authorize(&package, "rights-reader", reader, "proof.review.v1").unwrap();
        eval_in_context(
            "rights-writer",
            &format!(
                r#"
            globalThis.wrapper=v=>{{v.x=41;events.push('wrote:'+v.x);return 1;}};
            subscribeProof({writer}n);
        "#
            ),
        )
        .unwrap();
        eval_in_context(
            "rights-reader",
            &format!(
                r#"
            globalThis.wrapper=v=>{{
                if('x' in v)throw Error('another binding field leaked');
                events.push('read:'+v.renamed);
                let denied=false;try{{v.renamed=99;}}catch(_){{denied=true;}}
                if(!denied)throw Error('another binding mutation right leaked');
                return 0;
            }};
            subscribeProof({reader}n);
        "#
            ),
        )
        .unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        let result = close_frame(&info);
        assert_eq!(result.input.bits, 41);
        eval_in_context(
            "rights-reader",
            "if(events.join(',')!=='read:41')throw Error(events);",
        )
        .unwrap();
        for name in ["rights-writer", "rights-reader"] {
            unload_plugin(name);
        }
        drop(package);
        set_engine_ops(None);
        shutdown();
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

#[cfg(test)]
mod entity_transport_tests {
    use super::*;
    // This host transport models identities/staged commits, not native pointers.
    // The shared conformance body also runs against the real Linux fixture.
    struct MockFrame {
        id: u64,
        input: S2FunctionValue,
        output: S2FunctionValue,
        receiver: Option<S2FunctionValue>,
        edit: Option<S2FunctionValue>,
        action: i32,
    }
    thread_local! {
        static SLOTS:RefCell<std::collections::BTreeMap<i32,u32>>=const{RefCell::new(std::collections::BTreeMap::new())};
        static STACK:RefCell<Vec<MockFrame>>=const{RefCell::new(Vec::new())};
    }
    unsafe extern "C" fn slot(index: i32, serial: u32, live: i32) -> i32 {
        if !matches!(index, 901 | 902) {
            return 0;
        }
        SLOTS.with(|s| {
            if live == 1 {
                s.borrow_mut().insert(index, serial);
            } else {
                s.borrow_mut().remove(&index);
            }
        });
        1
    }
    fn project(mut v: S2FunctionValue, flags: u8) -> Option<S2FunctionValue> {
        let live = v.aux != u32::MAX
            && SLOTS.with(|s| s.borrow().get(&(v.aux as i32)).copied() == Some(v.bits as u32));
        v.flags = flags;
        if !live {
            if flags == 1 {
                return None;
            }
            v.aux = u32::MAX;
            v.bits = 0;
        }
        Some(v)
    }
    extern "C" fn prepare(
        _: *const i8,
        _: *const i8,
        abi: *const i8,
        _: *const i8,
        _: *mut i8,
        _: i32,
    ) -> i64 {
        if unsafe { std::ffi::CStr::from_ptr(abi) }
            .to_str()
            .unwrap()
            .contains("\"receiver\":\"entity\"")
        {
            12
        } else {
            11
        }
    }
    extern "C" fn acquire(_: i64, _: *mut i8, _: i32) -> i64 {
        1
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
        _: u8,
        out: *mut S2FunctionValue,
        why: *mut i8,
        cap: i32,
    ) -> i32 {
        STACK.with(|s| {
            let s = s.borrow();
            let Some(f) = s.last().filter(|f| f.id == token) else {
                return 0;
            };
            let v = if selector == -2 {
                f.output
            } else if selector == -1 {
                f.receiver.unwrap()
            } else {
                f.edit.unwrap_or(f.input)
            };
            if v.kind != 8 {
                unsafe { *out = v };
                return 1;
            }
            let Some(v) = project(v, unsafe { (*out).flags }) else {
                return refusal(why, cap);
            };
            unsafe { *out = v };
            1
        })
    }
    fn refusal(why: *mut i8, cap: i32) -> i32 {
        let message = b"strict entity projection is null or stale\0";
        if !why.is_null() && cap >= message.len() as i32 {
            unsafe {
                std::ptr::copy_nonoverlapping(message.as_ptr(), why.cast(), message.len());
            }
        }
        0
    }
    extern "C" fn write(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        _: i32,
        v: *const S2FunctionValue,
        why: *mut i8,
        cap: i32,
    ) -> i32 {
        let v = unsafe { *v };
        if project(v, v.flags).is_none() {
            return refusal(why, cap);
        }
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            let Some(f) = s.last_mut().filter(|f| f.id == token) else {
                return 0;
            };
            f.edit = Some(v);
            1
        })
    }
    extern "C" fn commit(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        action: i32,
        v: *const S2FunctionValue,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            let Some(f) = s.last_mut().filter(|f| f.id == token) else {
                return 0;
            };
            let edit = match f.edit {
                Some(e) => match project(e, e.flags) {
                    Some(e) => Some(e),
                    None => return 0,
                },
                None => None,
            };
            let output = if v.is_null() {
                None
            } else {
                let v = unsafe { *v };
                match project(v, v.flags) {
                    Some(v) => Some(v),
                    None => return 0,
                }
            };
            if let Some(e) = edit {
                f.input = e;
            }
            if let Some(v) = output {
                f.output = v;
            }
            f.action = action;
            1
        })
    }
    extern "C" fn call(
        target: i64,
        owner: u64,
        args: *const S2FunctionValue,
        _: i32,
        out: *mut S2FunctionValue,
        why: *mut i8,
        cap: i32,
    ) -> i32 {
        let input = unsafe { *args };
        let Some(input) = project(input, input.flags) else {
            return 0;
        };
        let receiver = if target == 12 { Some(input) } else { None };
        let input = if target == 12 {
            unsafe { *args.add(1) }
        } else {
            input
        };
        let request = unsafe { *out };
        let id = registry::next_id().unwrap();
        STACK.with(|s| {
            s.borrow_mut().push(MockFrame {
                id,
                input,
                output: input,
                receiver,
                edit: None,
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
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            let f = s.last_mut().unwrap();
            if f.action < 2 {
                f.output = f.input;
                if f.receiver.is_some() {
                    f.output.bits += 10;
                }
            }
            info.flags = u32::from(f.action >= 2);
            f.edit = None;
        });
        let post = crate::ffi::s2script_core_dispatch_function(target, &info, 1);
        let value = STACK.with(|s| s.borrow_mut().pop().unwrap().output);
        // RuntimeBinding::Call records callback failure, lets native original/POST
        // finish, then propagates the invocation-local error rather than output.
        if pre != 1 || post != 1 {
            let message = b"synchronous core function dispatch failed";
            if !why.is_null() && cap > 0 {
                let count = message.len().min(cap as usize - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(message.as_ptr(), why.cast(), count);
                    *why.add(count) = 0;
                }
            }
            return 0;
        }
        if value.kind != 8 {
            unsafe { *out = value };
            return 1;
        }
        let Some(value) = project(value, request.flags) else {
            return 0;
        };
        unsafe { *out = value };
        1
    }
    #[test]
    fn entity_projections_real_v8_all_preparation_subscription_orders() {
        init(frame_tests::logger).unwrap();
        let mut ops = S2EngineOps::default();
        ops.function_prepare = Some(prepare);
        ops.function_call = Some(call);
        ops.function_hook_acquire = Some(acquire);
        ops.function_hook_release = Some(release);
        ops.function_target_release = Some(release);
        ops.function_frame_read = Some(read);
        ops.function_frame_write = Some(write);
        ops.function_frame_commit = Some(commit);
        set_engine_ops(Some(ops));
        for first in [false, true] {
            for reverse in [false, true] {
                proof::entity_conformance(first, reverse, slot);
            }
        }
        set_engine_ops(None);
        shutdown();
    }
}

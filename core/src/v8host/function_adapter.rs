//! Host-authorized bootstrap and synchronous projected-value policy fan-out.
//! The public facade shares this service; additional projection codecs remain separate work.
use super::*;
use crate::engine_functions::copied;
use crate::engine_functions::{
    contract::*,
    package_adapter::{self, DispatchAdapter, SubscriberCursor},
    policy::{AdapterContract, HostAdapterGrant, PostReturnAuthority, SubscriptionMode},
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

pub(super) struct PreparedPackage {
    owner: HostPackageOwner,
    source: Arc<str>,
    manifest: ImplementationManifestHash,
    grants: Vec<HostAdapterGrant>,
    exports_required: bool,
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
    register_prepared_package_with_authorities(owner, source, manifest, Vec::new())
}
pub(crate) fn register_prepared_package_with_authorities(
    owner: HostPackageOwner,
    source: Arc<str>,
    manifest: ImplementationManifestHash,
    grants: Vec<HostAdapterGrant>,
) -> Result<PreparedPackageReceipt, String> {
    if grants.iter().any(|g| !g.belongs_to(&owner)) {
        return Err("adapter grant belongs to another package generation".into());
    }
    register_package(owner, source, manifest, grants, false)
}
pub(crate) fn register_selected_package(
    owner: HostPackageOwner,
    source: Arc<str>,
    manifest: ImplementationManifestHash,
) -> Result<PreparedPackageReceipt, String> {
    register_package(owner, source, manifest, Vec::new(), true)
}
fn register_package(
    owner: HostPackageOwner,
    source: Arc<str>,
    manifest: ImplementationManifestHash,
    grants: Vec<HostAdapterGrant>,
    exports_required: bool,
) -> Result<PreparedPackageReceipt, String> {
    if PACKAGES.with(|p| p.borrow().contains_key(&owner.key().generation)) {
        return Err("package source already registered".into());
    }
    if source.is_empty() {
        return Err("empty prepared package source".into());
    }
    owner.claim_source()?;
    let package = Rc::new(PreparedPackage {
        exports_required,
        retained_bytes: source.len()
            + manifest.as_str().len()
            + owner.key().id.len()
            + grants
                .iter()
                .map(HostAdapterGrant::retained_bytes)
                .sum::<usize>(),
        grants,
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
        drop_package(self.owner());
    }
}
struct Adapter {
    id: u64,
    post_authority: PostReturnAuthority,
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
    if owner.kind == OwnerKind::GamePackage && owner != package.owner() {
        return Err("binding belongs to another package".into());
    }
    if !PACKAGES.with(|p| {
        p.borrow()
            .get(&package.owner().generation)
            .is_some_and(|p| Rc::ptr_eq(p, &package.package))
    }) {
        return Err("package receipt revoked".into());
    }
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
pub(super) fn current_owner(scope: &mut v8::PinScope) -> Result<OwnerKey, String> {
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
pub(super) fn throw(scope: &mut v8::PinScope, error: impl AsRef<str>) {
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
// Host-created snapshots must not invoke inherited setters or expose inherited values.
fn set_own<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    key: &str,
    value: v8::Local<'s, v8::Value>,
) -> Result<(), String> {
    let key = v8::String::new(scope, key).ok_or("string allocation")?;
    if object.create_data_property(scope, key.into(), value) == Some(true) {
        Ok(())
    } else {
        Err("own data property creation failed".into())
    }
}
fn freeze_snapshot(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
) -> Result<(), String> {
    if object.set_integrity_level(scope, v8::IntegrityLevel::Frozen) == Some(true) {
        Ok(())
    } else {
        Err("snapshot freeze failed".into())
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
    if function.get_creation_context(scope) != Some(scope.get_current_context()) {
        return Err("package callback belongs to another context".into());
    }
    Ok(Some(v8::Global::new(scope, function)))
}
pub(crate) struct PackageExports {
    pub(crate) instance: PackageInstanceKey,
    pub(crate) modules: BTreeMap<String, v8::Global<v8::Object>>,
}
/// Snapshot only own data properties; host validation never invokes user getters or proxy traps.
fn capture_exports<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    result: v8::Local<'s, v8::Value>,
) -> Result<BTreeMap<String, v8::Global<v8::Object>>, String> {
    fn synchronous_object<'s>(
        scope: &mut v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<v8::Local<'s, v8::Object>, String> {
        if !value.is_object()
            || value.is_proxy()
            || value.is_promise()
            || value.is_function()
            || value.is_array()
        {
            return Err("package exports must be synchronous module objects".into());
        }
        let object =
            v8::Local::<v8::Object>::try_from(value).map_err(|_| "module object required")?;
        let mut cursor = Some(object);
        for _ in 0..32 {
            let Some(current) = cursor else {
                return Ok(object);
            };
            if current.is_proxy() {
                return Err("proxy export prototype".into());
            }
            let then = v8::String::new(scope, "then").ok_or("property allocation")?;
            let descriptor = current
                .get_own_property_descriptor(scope, then.into())
                .ok_or("export descriptor")?;
            if !descriptor.is_undefined() {
                return Err("asynchronous or accessor then export".into());
            }
            let prototype = current.get_prototype(scope).ok_or("export prototype")?;
            cursor = v8::Local::<v8::Object>::try_from(prototype).ok();
        }
        Err("export prototype depth exceeded".into())
    }
    let map = synchronous_object(scope, result)?;
    let keys = map
        .get_own_property_names(
            scope,
            v8::GetPropertyNamesArgs {
                property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                key_conversion: v8::KeyConversionMode::KeepNumbers,
                ..Default::default()
            },
        )
        .ok_or("export map keys")?;
    if keys.length() == 0 || keys.length() > 64 {
        return Err("export map size limit".into());
    }
    let mut modules = BTreeMap::new();
    for i in 0..keys.length() {
        let key = keys.get_index(scope, i).ok_or("export key")?;
        if !key.is_string() {
            return Err("export key must be string".into());
        }
        let name = key.to_rust_string_lossy(scope);
        if name.len() > 256
            || (name != "."
                && !name.strip_prefix("./").is_some_and(|p| {
                    p.split('/').all(|part| {
                        !part.is_empty()
                            && part != "."
                            && part != ".."
                            && part
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    })
                }))
        {
            return Err("invalid package export subpath".into());
        }
        let key = v8::Local::<v8::Name>::try_from(key).map_err(|_| "export key")?;
        let descriptor = map
            .get_own_property_descriptor(scope, key)
            .ok_or("export property descriptor")?;
        let descriptor = v8::Local::<v8::Object>::try_from(descriptor)
            .map_err(|_| "export property descriptor")?;
        let value_key = v8::String::new(scope, "value").ok_or("property allocation")?;
        if descriptor.has_own_property(scope, value_key.into()) != Some(true) {
            return Err("export map accessor forbidden".into());
        }
        let value = descriptor
            .get(scope, value_key.into())
            .ok_or("export value")?;
        let module = synchronous_object(scope, value)?;
        modules.insert(name, v8::Global::new(scope, module));
    }
    if !modules.contains_key(".") {
        return Err("root package export required".into());
    }
    Ok(modules)
}
pub(crate) fn bootstrap(
    scope: &mut v8::PinScope,
    id: &str,
    generation: u64,
) -> Result<Vec<PackageExports>, String> {
    let mut exports = Vec::new();
    let packages = PACKAGES.with(|p| p.borrow().values().cloned().collect::<Vec<_>>());
    for package in packages {
        let instance = PackageInstanceKey {
            parent: OwnerKey::plugin(id, generation),
            package_owner: package.owner.key().clone(),
        };
        let prior = BOOTSTRAP.with(|b| b.replace(Some(instance.clone())));
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
        let data = v8::Array::new(scope, 4);
        let package_generation = v8::BigInt::new_from_u64(scope, package.owner.key().generation);
        let parent_generation = v8::BigInt::new_from_u64(scope, generation);
        data.set_index(scope, 0, package_generation.into());
        data.set_index(scope, 1, parent_generation.into());
        let parent_id = v8::String::new(scope, id).ok_or("parent allocation")?;
        data.set_index(scope, 2, parent_id.into());
        let provisional = v8::Integer::new(scope, 0);
        data.set_index(scope, 3, provisional.into());
        // One shared hidden token, not a second instance book. A failed evaluation
        // revokes all its captured callbacks, including those leaked by reviewed JS.
        let result = (|| -> Result<Option<BTreeMap<String, v8::Global<v8::Object>>>, String> {
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
            let lookup = v8::Function::builder(js_package_lookup)
                .data(data.into())
                .build(scope)
                .ok_or("lookup allocation")?;
            set(scope, global, "__s2_package_function", lookup.into())?;
            let mut storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let source = v8::String::new(&mut tc, &package.source).ok_or("source allocation")?;
            let evaluated = v8::Script::compile(&mut tc, source, None)
                .and_then(|s| s.run(&mut tc))
                .ok_or_else(|| {
                    tc.exception()
                        .map(|e| e.to_rust_string_lossy(&tc))
                        .unwrap_or("package bootstrap threw".into())
                })?;
            let modules = if package.exports_required {
                Some(capture_exports(&mut tc, evaluated)?)
            } else {
                None
            };
            if current_owner(&mut tc)? != instance.parent {
                return Err("package parent retired during bootstrap".into());
            }
            Ok(modules)
        })();
        let state = v8::Integer::new(scope, if result.is_ok() { 1 } else { 2 });
        data.set_index(scope, 3, state.into());
        for name in [
            "__s2_function_adapter_register",
            "__s2_function_adapter_subscribe",
            "__s2_package_function",
        ] {
            let key = v8::String::new(scope, name).unwrap();
            global.delete(scope, key.into());
        }
        if result.is_err() {
            drop_instance(&instance);
        }
        if let Some(modules) = result? {
            exports.push(PackageExports { instance, modules });
        }
    }
    Ok(exports)
}
pub(super) fn instance(
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
    let entered = scope.get_entered_or_microtask_context();
    if !entered
        .get_slot::<PluginId>()
        .is_some_and(|p| p.0 == parent.id)
        || entered.get_slot::<InteropGeneration>().map(|g| g.0) != Some(parent.generation)
    {
        return Err("package token used from another context".into());
    }
    let expected_id = data
        .get_index(scope, 2)
        .ok_or("missing parent id")?
        .to_rust_string_lossy(scope);
    let state = data
        .get_index(scope, 3)
        .and_then(|v| v.int32_value(scope))
        .ok_or("missing instance state")?;
    if state == 2 {
        return Err("package instance revoked".into());
    }
    if parent.generation != expected_parent || parent.id != expected_id {
        return Err("package token belongs to another parent generation".into());
    }
    let package = PACKAGES
        .with(|p| p.borrow().get(&generation).cloned())
        .ok_or("package receipt revoked")?;
    let instance = PackageInstanceKey {
        parent,
        package_owner: package.owner.key().clone(),
    };
    if state != 1 && !(state == 0 && BOOTSTRAP.with(|b| b.borrow().as_ref() == Some(&instance))) {
        return Err("package instance not active".into());
    }
    Ok((instance, package))
}
fn js_package_lookup(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| {
        let (instance, _) = instance(scope, args.data())?;
        if BOOTSTRAP.with(|b| b.borrow().as_ref() != Some(&instance)) {
            return Err("host bootstrap authority required".to_string());
        }
        if !args.get(0).is_string() {
            return Err("engine function name must be a string".into());
        }
        let binding = registry::named_binding(
            &instance.package_owner,
            &args.get(0).to_rust_string_lossy(scope),
        )?;
        Ok(super::engine_functions::facade(
            scope,
            &instance.parent,
            &binding,
            Some(args.data()),
        ))
    })();
    match result {
        Ok(v) => rv.set(v.into()),
        Err(e) => throw(scope, e),
    }
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
                    post_authority: package.grants.iter().map(|g| g.authority(&AdapterContract {
                        id: semantic.clone(), version: 1, contract_hash: hash.clone(),
                    })).find(|a| *a == PostReturnAuthority::Override).unwrap_or_default(),
                    instance,
                    semantic,
                    hash,
                    package,
                    pre,
                    post,
                }),
            )
        });
        receipt(scope, id, false, Some(args.data()))
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
        // Process bindings are resolved only in this package namespace. The numeric
        // path is the existing explicit plugin-binding compatibility/proof seam.
        let binding = if args.get(0).is_string() {
            registry::named_binding(
                &instance.package_owner,
                &args.get(0).to_rust_string_lossy(scope),
            )?
        } else {
            registry::binding(bigint(args.get(0))?, &instance.parent)?
        };
        let id = binding.id;
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
        receipt(scope, id, true, Some(args.data()))
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
    subscribe_generic_binding(scope, owner, None, binding, phase, observe_only, wrapper)
}
pub(super) fn subscribe_package_generic(
    scope: &mut v8::PinScope,
    token: v8::Local<v8::Value>,
    binding_id: u64,
    phase: i32,
    observe_only: bool,
    wrapper: v8::Global<v8::Function>,
) -> Result<u64, String> {
    let (instance, _) = instance(scope, token)?;
    let binding = registry::binding(binding_id, &instance.package_owner)?;
    subscribe_generic_binding(
        scope,
        instance.parent.clone(),
        Some(instance),
        binding,
        phase,
        observe_only,
        wrapper,
    )
}
fn subscribe_generic_binding(
    scope: &mut v8::PinScope,
    owner: OwnerKey,
    instance: Option<PackageInstanceKey>,
    binding: Rc<Binding>,
    phase: i32,
    observe_only: bool,
    wrapper: v8::Global<v8::Function>,
) -> Result<u64, String> {
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
    if instance.is_some()
        && function.get_creation_context(scope) != Some(scope.get_current_context())
    {
        return Err("package callback belongs to another context".into());
    }
    if function.is_async_function() {
        return Err("synchronous wrapper required".into());
    }
    let contract = AdapterContract::generic(&binding.function.policy)?;
    let implementation = crate::engine_functions::policy::public_adapter(&binding.function.policy)?;
    insert_subscription(
        owner,
        instance,
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
    // Only the test-only public-generic proof entry lacks a package instance.
    token: Option<v8::Local<v8::Value>>,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let object = v8::Object::new(scope);
    let data = v8::Array::new(scope, 3);
    let id = v8::BigInt::new_from_u64(scope, id);
    let kind = v8::Boolean::new(scope, subscription);
    data.set_index(scope, 0, id.into());
    data.set_index(scope, 1, kind.into());
    if let Some(token) = token {
        data.set_index(scope, 2, token);
    }
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
    if let Some(token) = data.get_index(scope, 2).filter(|v| !v.is_undefined()) {
        // Validate the retained native token even after its resource row was removed:
        // disposal stays idempotent only in the same live instance/context.
        let (instance, _) = instance(scope, token)?;
        let matches = if sub {
            SUBSCRIPTIONS.with(|s| {
                s.borrow()
                    .get(&id)
                    .map(|s| s.instance.as_ref() == Some(&instance))
            })
        } else {
            ADAPTERS.with(|a| a.borrow().get(&id).map(|a| a.instance == instance))
        };
        if matches == Some(false) {
            return Err("receipt package instance mismatch".into());
        }
    }
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
    let state = (|| -> Result<&'static str, String> {
        let (id, sub) = receipt_data(scope, args.data())?;
        if !sub {
            return Ok(if receipt_owner(id, false).is_some() {
                "active"
            } else {
                "disposed"
            });
        }
        let target = SUBSCRIPTIONS.with(|s| s.borrow().get(&id).and_then(|s| s.binding.target));
        Ok(match target.map(runtime::status) {
            None => "disposed",
            Some(Err(_)) => "unavailable",
            Some(Ok(status)) => match status.state {
                1 => "pending",
                2 => "active",
                3 => "removing",
                4 => "disposed",
                _ => "failed",
            },
        })
    })();
    match state {
        Ok(state) => {
            let text = v8::String::new(scope, state).unwrap();
            rv.set(text.into());
        }
        Err(error) => throw(scope, error),
    }
}
/// Public receipt projection; never leaks target or hook ids.
pub(super) fn subscription_state(id: u64, owner: &OwnerKey) -> Result<(&'static str, Option<String>), String> {
    let target = SUBSCRIPTIONS.with(|s| {
        let rows = s.borrow();
        match rows.get(&id) {
            Some(s) if s.owner == *owner => Ok(s.binding.target),
            Some(_) => Err("subscription owner mismatch".to_string()),
            None => Ok(None),
        }
    })?;
    Ok(match target.map(runtime::status) {
        None => ("disposed", None),
        Some(Err(e)) => ("failed", Some(e)),
        Some(Ok(status)) => match status.state {
            1 => ("pending", None), 2 => ("active", None), 3 | 4 => ("disposed", None),
            _ => ("failed", Some("native hook installation failed".into())),
        }
    })
}
pub(super) fn binding_observation(binding: u64) -> &'static str {
    let subscribers = SUBSCRIPTIONS.with(|s| s.borrow().values().filter(|s| s.binding.id == binding)
        .map(|s| (s.id, s.owner.clone())).collect::<Vec<_>>());
    let mut state = "not-requested";
    for (id, owner) in subscribers {
        match subscription_state(id, &owner).map(|s| s.0) {
            Ok("failed") | Err(_) => return "failed",
            Ok("pending") => state = "pending",
            Ok("active") if state != "pending" => state = "active",
            _ => {},
        }
    }
    state
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
        if !ADAPTERS.with(|a| a.borrow().values().any(|a| a.semantic == adapter.semantic)) {
            SEMANTICS.with(|s| s.borrow_mut().remove(&adapter.semantic));
        }
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
// Called only after the bootstrap token, parent, or package authority is revoked.
// Token state is carried by the native callback data, never reconstructed from these rows.
fn drop_instance(instance: &PackageInstanceKey) {
    let subscriptions = SUBSCRIPTIONS.with(|s| {
        s.borrow()
            .values()
            .filter(|s| s.instance.as_ref() == Some(instance))
            .map(|s| s.id)
            .collect::<Vec<_>>()
    });
    for id in subscriptions {
        release_resource(
            &instance.parent.id,
            instance.parent.generation,
            &plugin::Resource::FunctionSubscription(id),
        );
        drop_subscription(id);
    }
    let adapters = ADAPTERS.with(|a| {
        a.borrow()
            .values()
            .filter(|a| a.instance == *instance)
            .map(|a| a.id)
            .collect::<Vec<_>>()
    });
    for id in adapters {
        release_resource(
            &instance.parent.id,
            instance.parent.generation,
            &plugin::Resource::FunctionAdapter(id),
        );
        drop_adapter(id);
    }
}
pub(crate) fn drop_package(owner: &OwnerKey) {
    // Remove source authority first. Holds in callbacks can retain memory, never access.
    let removed = PACKAGES.with(|p| {
        let mut packages = p.borrow_mut();
        if packages
            .get(&owner.generation)
            .is_some_and(|p| p.owner.key() == owner)
        {
            packages.remove(&owner.generation)
        } else {
            None
        }
    });
    if let Some(package) = &removed {
        package.owner.retire_source();
    }
    let mut instances = ADAPTERS.with(|a| {
        a.borrow()
            .values()
            .filter(|a| a.instance.package_owner == *owner)
            .map(|a| a.instance.clone())
            .collect::<Vec<_>>()
    });
    SUBSCRIPTIONS.with(|s| {
        instances.extend(s.borrow().values().filter_map(|s| {
            s.instance
                .as_ref()
                .filter(|i| i.package_owner == *owner)
                .cloned()
        }))
    });
    for instance in instances {
        drop_instance(&instance);
    }
    AUTHORIZED.with(|a| a.borrow_mut().retain(|_, (package, _, _)| package != owner));
    drop(removed);
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

#[derive(Clone)]
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
    // Rust drops fields in declaration order. Keep the charge until all retained
    // adapter holds, delivery elements and their vector allocation are destroyed.
    copy_bookkeeping: Option<copied::Bookkeeping>,
}
type StagedEdits = std::collections::BTreeMap<i32, (ProjectedValue, String)>;
struct Dispatch {
    frame: Frame,
    binding: Rc<Binding>,
    adapter: Option<Rc<Adapter>>,
    subscribers: Vec<Rc<Subscription>>,
    cursor: Cell<usize>,
    revision: Rc<Cell<u64>>,
    edits: Rc<RefCell<StagedEdits>>,
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
    pending_edits: Rc<RefCell<StagedEdits>>,
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
    ) -> Result<(Self, u64, Rc<RefCell<StagedEdits>>), String> {
        let id = registry::next_id()?;
        let pending_edits = Rc::new(RefCell::new(std::collections::BTreeMap::new()));
        LEASES.with(|s| {
            s.borrow_mut().push(Lease {
                id,
                owner,
                dispatch,
                binding,
                adapter,
                mode,
                pending_edits: pending_edits.clone(),
                enabled: true,
            })
        });
        Ok((Self, id, pending_edits))
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
                .filter(|l| {
                    l.enabled
                        && l.id == id
                        && l.owner == owner
                        && l.binding.is_live()
                        && l.dispatch
                            .adapter
                            .as_ref()
                            .is_none_or(|a| ADAPTERS.with(|rows| rows.borrow().contains_key(&a.id)))
                })
                .cloned()
        })
        .ok_or("expired or suspended callback lease".into())
}
/// Private construction binds authority to one exact active adapter callback.
/// Holding this object does not extend its lease or revive a removed registration.
pub(crate) struct AdapterPostReturnPermit {
    lease: Lease,
}
impl AdapterPostReturnPermit {
    fn issue(lease: Lease) -> Result<Self, String> {
        let permit = Self { lease };
        permit.validate()?;
        Ok(permit)
    }
    pub(crate) fn validate(&self) -> Result<(Frame, Rc<Binding>), String> {
        let l = &self.lease;
        if !LEASES.with(|s| {
            s.borrow().last().is_some_and(|top| {
                top.enabled
                    && top.adapter
                    && top.id == l.id
                    && top.owner == l.owner
                    && Rc::ptr_eq(&top.dispatch, &l.dispatch)
                    && Rc::ptr_eq(&top.binding, &l.binding)
            })
        }) {
            return Err("expired or suspended adapter POST permit".into());
        }
        let adapter = l
            .dispatch
            .adapter
            .as_ref()
            .ok_or("adapter POST authority required")?;
        if !l.adapter
            || l.dispatch.frame.phase != 1
            || adapter.post_authority != PostReturnAuthority::Override
            || adapter.instance.parent != l.owner
            || !owner_is_live(&l.owner.id, l.owner.generation)
            || plugin_phase(&l.owner.id) != Some(plugin::Phase::Active)
            || !ADAPTERS.with(|a| {
                a.borrow()
                    .get(&adapter.id)
                    .is_some_and(|a| Rc::ptr_eq(a, adapter))
            })
            || !PACKAGES.with(|p| {
                p.borrow()
                    .get(&adapter.instance.package_owner.generation)
                    .is_some_and(|p| Rc::ptr_eq(p, &adapter.package))
            })
        {
            return Err("adapter POST authority revoked or unavailable".into());
        }
        registry::binding(l.binding.id, &l.binding.owner)?;
        if !AUTHORIZED.with(|a| {
            a.borrow()
                .get(&l.binding.id)
                .is_some_and(|(owner, id, hash)| {
                    *owner == adapter.instance.package_owner
                        && *id == adapter.semantic
                        && *hash == adapter.hash
                })
        }) {
            return Err("adapter POST binding authorization mismatch".into());
        }
        Ok((l.dispatch.frame.clone(), l.binding.clone()))
    }
}
struct GuardedPostFrame {
    permit: AdapterPostReturnPermit,
}
impl package_adapter::ProjectedFrame for GuardedPostFrame {
    fn original_return(&self) -> Result<Option<ProjectedValue>, String> {
        runtime::original_return(&self.permit)
    }
    fn override_return(&mut self, value: ProjectedValue) -> Result<ProjectedValue, String> {
        runtime::override_return(&self.permit, value)
    }
}
fn js_original_return(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| {
        let l = lease(scope, bigint(args.data())?)?;
        let permit = AdapterPostReturnPermit::issue(l)?;
        let frame = GuardedPostFrame { permit };
        match package_adapter::ProjectedFrame::original_return(&frame)? {
            Some(value) => projected_to_js(scope, value),
            None => Ok(v8::undefined(scope).into()),
        }
    })();
    match result {
        Ok(value) => rv.set(value),
        Err(e) => throw(scope, e),
    }
}
fn js_override_return(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| {
        let l = lease(scope, bigint(args.data())?)?;
        let permit = AdapterPostReturnPermit::issue(l)?;
        let (_, binding) = permit.validate()?;
        let ret = &binding.function.abi.returns;
        let value = callback_projected_from_js(scope, args.get(0), &ret.native, &ret.projection.id)
            .map_err(|e| format!("{}: {e}", binding.function.canonical_id))?;
        let mut frame = GuardedPostFrame { permit };
        projected_to_js(
            scope,
            package_adapter::ProjectedFrame::override_return(&mut frame, value)?,
        )
    })();
    match result {
        Ok(value) => rv.set(value),
        Err(e) => throw(scope, e),
    }
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
pub(super) fn projected_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: ProjectedValue,
) -> Result<v8::Local<'s, v8::Value>, String> {
    match value {
        ProjectedValue::Copied(copy) => {
            if copy.flags == 4 {
                let text =
                    std::str::from_utf8(copy.bytes()).map_err(|_| "invalid copied string")?;
                Ok(v8::String::new(scope, text)
                    .ok_or("FunctionCopyOutputAllocationFailure")?
                    .into())
            } else {
                let out = v8::Object::new(scope);
                for (key, bytes) in ["x", "y", "z"]
                    .into_iter()
                    .zip(copy.bytes().chunks_exact(4))
                {
                    let value = v8::Number::new(
                        scope,
                        f32::from_le_bytes(bytes.try_into().unwrap()) as f64,
                    );
                    set_own(scope, out, key, value.into())?;
                }
                freeze_snapshot(scope, out)?;
                Ok(out.into())
            }
        }
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
fn copied_from_js(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    flag: u8,
    producer: copied::Producer,
) -> Result<copied::Owned, String> {
    if flag == 4 {
        let text =
            v8::Local::<v8::String>::try_from(value).map_err(|_| "primitive string required")?;
        let len = text.length();
        if len > copied::MAX_STRING {
            return Err("FunctionCopyInvalidValue: string limit".into());
        }
        let mut utf16 = copied::Buffer::new(len * 2, producer)?;
        let mut chunk = [0u16; 512];
        for offset in (0..len).step_by(512) {
            let n = (len - offset).min(512);
            text.write_v2(
                scope,
                offset as u32,
                &mut chunk[..n],
                v8::WriteFlags::empty(),
            );
            for (i, u) in chunk[..n].iter().enumerate() {
                utf16.bytes_mut()[(offset + i) * 2..(offset + i) * 2 + 2]
                    .copy_from_slice(&u.to_le_bytes());
            }
        }
        let units = || {
            utf16
                .bytes()
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
        };
        let mut count = 0;
        for c in char::decode_utf16(units()) {
            let c = c.map_err(|_| "FunctionCopyInvalidValue: lone UTF-16 surrogate")?;
            if c == '\0' {
                return Err("FunctionCopyInvalidValue: NUL".into());
            }
            count += c.len_utf8();
        }
        if count > copied::MAX_STRING {
            return Err("FunctionCopyInvalidValue: UTF-8 limit".into());
        }
        let mut out = copied::Buffer::new(count, producer)?;
        let mut offset = 0;
        for c in char::decode_utf16(units()) {
            let c = c.unwrap();
            let n = c.len_utf8();
            c.encode_utf8(&mut out.bytes_mut()[offset..offset + n]);
            offset += n;
        }
        out.own(flag)
    } else {
        if value.is_proxy() || !value.is_object() || value.is_array() {
            return Err("strict vector data object required".into());
        }
        let object = v8::Local::<v8::Object>::try_from(value).map_err(|_| "vector required")?;
        let keys = object
            .get_own_property_names(
                scope,
                v8::GetPropertyNamesArgs {
                    property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                    key_conversion: v8::KeyConversionMode::KeepNumbers,
                    ..Default::default()
                },
            )
            .ok_or("vector keys unavailable")?;
        if keys.length() != 3 {
            return Err("vector requires exactly x/y/z".into());
        }
        let mut bytes = [0u8; 12];
        for (i, key) in ["x", "y", "z"].into_iter().enumerate() {
            let key = v8::String::new(scope, key).ok_or("vector key allocation")?;
            let desc = object
                .get_own_property_descriptor(scope, key.into())
                .ok_or("vector data property required")?;
            let desc = v8::Local::<v8::Object>::try_from(desc)
                .map_err(|_| "vector own data property required")?;
            let key = v8::String::new(scope, "value").ok_or("descriptor allocation")?;
            if desc.has_own_property(scope, key.into()) != Some(true) {
                return Err("vector accessors forbidden".into());
            }
            let value = desc.get(scope, key.into()).ok_or("vector value missing")?;
            if !value.is_number() {
                return Err("vector primitive numbers required".into());
            }
            let f = value.number_value(scope).unwrap() as f32;
            if !f.is_finite() {
                return Err("finite f32 vector required".into());
            }
            bytes[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
        }
        let mut out = copied::Buffer::new(12, producer)?;
        out.bytes_mut().copy_from_slice(&bytes);
        out.own(flag)
    }
}
fn callback_projected_from_js(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    native: &str,
    projection: &str,
) -> Result<ProjectedValue, String> {
    if let Some(flag) = copied::flag(projection) {
        let owner = LEASES
            .with(|s| {
                s.borrow().last().map(|l| {
                    if l.adapter {
                        l.dispatch
                            .adapter
                            .as_ref()
                            .unwrap()
                            .instance
                            .package_owner
                            .clone()
                    } else {
                        l.owner.clone()
                    }
                })
            })
            .unwrap_or(current_owner(scope)?);
        return copied_from_js(scope, value, flag, copied::Producer::owner(&owner))
            .map(ProjectedValue::Copied);
    }
    projected_from_js(scope, value, native, projection)
}
pub(super) fn projected_from_js(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    native: &str,
    projection: &str,
) -> Result<ProjectedValue, String> {
    if let Some(flag) = copied::flag(projection) {
        if native != "ptr" {
            return Err("copied projection requires pointer ABI".into());
        }
        let producer = copied::Producer::owner(&current_owner(scope)?);
        return copied_from_js(scope, value, flag, producer).map(ProjectedValue::Copied);
    }
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
        let staged = l.pending_edits.borrow().get(&i).map(|(value, _)| value.clone())
            .or_else(|| l.dispatch.edits.borrow().get(&i).map(|(value, _)| value.clone()));
        let v = if let Some(value) = staged {
            match value {
                ProjectedValue::Entity { reference, .. } =>
                    EntityProjection::parse(projection).ok_or("entity projection mismatch")?.value(reference),
                scalar => Ok(scalar),
            }
        } else {
            l.dispatch.frame.read_projected(i, native, projection)
        }.map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))?;
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
        let value = callback_projected_from_js(scope, args.get(0), native, projection)
            .map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))?;
        let next_revision = l.dispatch.revision.get().checked_add(1)
            .ok_or("frame revision exhausted")?;
        if runtime::has_copies(&l.binding.function.abi) || (!l.adapter && l.binding.function.policy.suppression == "none") {
            l.pending_edits.borrow_mut().insert(i, (value, l.binding.function.canonical_id.clone()));
        } else {
            l.dispatch.frame.write_projected(i, &value)
                .map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))?;
            l.dispatch.edits.borrow_mut().insert(i, (value, l.binding.function.canonical_id.clone()));
        }
        l.dispatch.revision.set(next_revision);
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
    if dispatch.frame.phase == 1 && LEASES.with(|s| s.borrow().last().is_some_and(|l|
        l.id == lease && l.adapter && dispatch.adapter.as_ref().is_some_and(|a|
            a.post_authority == PostReturnAuthority::Override))) {
        let data = v8::BigInt::new_from_u64(scope, lease);
        let getter = v8::Function::builder(js_original_return).data(data.into()).build(scope).ok_or("getter allocation")?;
        let undefined = v8::undefined(scope);
        let desc = v8::PropertyDescriptor::new_from_get_set(getter.into(), undefined.into());
        let key = v8::String::new(scope, "originalReturnValue").unwrap();
        object.define_property(scope,key.into(),&desc);
        let method = v8::Function::builder(js_override_return).data(data.into()).build(scope).ok_or("method allocation")?;
        set(scope,object,"overrideReturn",method.into())?;
    }
    object.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
    Ok(object)
}
fn delivery_key<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> Result<v8::Local<'s, v8::Private>, String> {
    let name = v8::String::new(scope, "s2script.function-copy.delivery.v1")
        .ok_or("delivery key allocation")?;
    Ok(v8::Private::for_api(scope, Some(name)))
}

fn decision(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    native: &str,
    projection: &str,
    phase: i32,
    suppression: &str,
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
        if suppression == "none" && (2..=3).contains(&action) {
            return Err("suppression:none forbids Handled/Stop".into());
        }
        if (0..=1).contains(&action) || (kind == 0 && (2..=3).contains(&action)) {
            return Ok(Decision {
                action,
                value: None,
            });
        }
        return Err("suppression requires typed decision object".into());
    }
    if value.is_proxy() {return Err("proxy decision forbidden".into())}
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
    if suppression == "none" {
        return Err("suppression:none forbids suppression decision objects".into());
    }
    let value = get(scope, object, "returnValue")?;
    let key=delivery_key(scope)?;
    if let Some(tag)=object.get_private(scope,key).filter(|v|!v.is_undefined()) {
        let tag=v8::Local::<v8::Array>::try_from(tag).map_err(|_|"invalid host delivery")?;
        let lease_id=bigint(tag.get_index(scope,0).ok_or("missing delivery lease")?)?;
        let index=bigint(tag.get_index(scope,1).ok_or("missing delivery index")?)? - 1;
        let original=tag.get_index(scope,2).ok_or("missing delivery value")?;
        let carried=LEASES.with(|s| {
            let s=s.borrow();let l=s.last().filter(|l|l.adapter && l.id==lease_id).ok_or("foreign or expired host delivery")?;
            let d=l.dispatch.deliveries.borrow();let d=d.get(index as usize).ok_or("expired delivery index")?;
            if d.action!=action || !value.strict_equals(original){return Err("host delivery changed")}
            Ok(d.clone())
        })?;
        return Ok(carried);
    }
    let value = callback_projected_from_js(scope, value, native, projection)?;
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
        // Adapter edits preceding invokeNext are visible to that subscriber.
        // An invalid adapter result still aborts the entire uncommitted frame.
        if runtime::has_copies(&l.binding.function.abi) {
            l.dispatch.edits.borrow_mut().append(&mut l.pending_edits.borrow_mut());
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
            let delivery_index=l.dispatch.deliveries.borrow().len();
            l.dispatch.deliveries.borrow_mut().push(value.clone());
            let out = v8::Object::new(scope);
            let action = v8::Integer::new(scope, value.action);
            set_own(scope, out, "action", action.into())?;
            let original = match value.value {
                Some(value) => projected_to_js(scope, value)?,
                None => v8::undefined(scope).into(),
            };
            set_own(scope, out, "returnValue", original)?;
            let lease_id = v8::BigInt::new_from_u64(scope, l.id);
            let index = v8::BigInt::new_from_u64(scope, delivery_index as u64 + 1);
            // Array construction initializes own elements directly; no prototype
            // setter or getter participates in recording the emitted return.
            let tag = v8::Array::new_with_elements(scope, &[lease_id.into(), index.into(), original]);
            freeze_snapshot(scope, tag.into())?;
            let key = delivery_key(scope)?;
            if out.set_private(scope, key, tag.into()) != Some(true) {
                return Err("delivery lineage allocation".into());
            }
            let revision = v8::Number::new(scope, l.dispatch.revision.get() as f64);
            set_own(scope, out, "frameRevision", revision.into())?;
            freeze_snapshot(scope, out)?;
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
    let prior_revision = dispatch.revision.get();
    let context = clone_plugin_context(&sub.owner.id).ok_or("subscriber context unavailable")?;
    let context = v8::Local::new(parent, &context);
    let scope = &mut v8::ContextScope::new(parent, context);
    let mut storage = v8::TryCatch::new(scope);
    let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
    let _busy = crate::dispatch::ParentBusy::enter(&sub.owner.id, sub.owner.generation);
    let (guard, id, pending_edits) = LeaseGuard::enter(
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
    let decision = value.ok_or_else(|| "subscriber wrapper threw".to_string()).and_then(|value| {
        decision(
            &mut tc,
            value,
            &sub.binding.function.abi.returns.native,
            &sub.binding.function.abi.returns.projection.id,
            dispatch.frame.phase,
            &sub.binding.function.policy.suppression,
        )
    }).and_then(|decision| {
        if sub.mode == SubscriptionMode::Observe && decision.action != 0 {
            Err("observe-only subscriber cannot change the decision".into())
        } else { Ok(decision) }
    });
    if decision.is_ok() {
        // A nonsuppressing generic callback's setters are private until its
        // decision validates. Later callbacks read accepted edits from this
        // overlay; the final transfer validates/writes before native commit.
        dispatch.edits.borrow_mut().extend(pending_edits.borrow().iter().map(|(k, v)| (*k, v.clone())));
    } else if runtime::has_copies(&sub.binding.function.abi) || sub.binding.function.policy.suppression == "none" {
        dispatch.revision.set(prior_revision);
    }
    decision
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
    let prior_revision=dispatch.revision.get();
    let (guard, id, pending_edits) = LeaseGuard::enter(
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
    let result=decision(
        &mut tc,
        value.ok_or("adapter threw")?,
        &dispatch.binding.function.abi.returns.native,
        &dispatch.binding.function.abi.returns.projection.id,
        dispatch.frame.phase,
        &dispatch.binding.function.policy.suppression,
    );
    if result.is_ok() {dispatch.edits.borrow_mut().extend(pending_edits.borrow().iter().map(|(k,v)|(*k,v.clone())));}
    else if runtime::has_copies(&dispatch.binding.function.abi) {dispatch.revision.set(prior_revision);}
    result
}
struct GenericCursor<'a, 's, 'i> {
    adapter_owner: Option<OwnerKey>,
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
                || (self.adapter_owner.as_ref() != Some(&sub.owner)
                    && crate::dispatch::parent_busy(&sub.owner.id, sub.owner.generation))
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
            self.dispatch.deliveries.borrow_mut().push(decision.clone());
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
                adapter_owner: None,
                scope,
                dispatch: part.clone(),
            };
            let mut args = package_adapter::AdapterDispatch {
                cursor: &mut cursor,
                frame: None,
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
            .extend(part.deliveries.borrow().iter().cloned());
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
        && PACKAGES.with(|p| {
            p.borrow()
                .contains_key(&adapter.instance.package_owner.generation)
        })
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
    let _copy_scope=copied::Scope::enter()?;
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
    // Take the exact paired PRE state into this POST stack, keeping its copied
    // deliveries and registration hold alive until POST actually completes.
    let post_state = if phase == 1 {
        INVOCATIONS.with(|i| i.borrow_mut().remove(&key))
    } else {
        None
    };
    // Reserve an upper bound for callback maps, cloned owned-value handles,
    // V8 delivery tags, and retained decision vectors before dispatch allocations.
    // Byte payloads are charged independently by Buffer for their actual capacity.
    let copy_bookkeeping=SUBSCRIPTIONS.with(|rows| {
        let rows=rows.borrow();let mut bytes=4096usize;let mut copies=false;
        for sub in rows.values().filter(|sub|sub.binding.target==Some(target)) {
            copies|=runtime::has_copies(&sub.binding.function.abi);
            bytes=bytes.checked_add(4*32*(256+sub.binding.function.canonical_id.len())).ok_or("FunctionCopyBudgetExceeded: dispatch bookkeeping")?;
        }
        if copies {copied::Bookkeeping::reserve(bytes,copied::Producer::engine()).map(Some)}else{Ok(None)}
    })?;
    let subscribers = SUBSCRIPTIONS.with(|s| {
        s.borrow()
            .values()
            .filter(|s| {
                s.binding.target == Some(target)
                    && s.binding.is_live()
                    && s.instance.as_ref().is_none_or(|i| PACKAGES.with(|p| p.borrow().contains_key(&i.package_owner.generation)))
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
                        copy_bookkeeping: None,
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
            // The binding may be process-owned and have no Plugin context. Callback
            // execution belongs to an admitted subscriber's exact live parent.
            let parent = &dispatch.subscribers[0].owner;
            if parent.kind != OwnerKind::Plugin || !owner_is_live(&parent.id, parent.generation) {
                return Err("dispatch parent generation unavailable".into());
            }
            let context = clone_plugin_context(&parent.id).ok_or("dispatch context unavailable")?;
            let context = v8::Local::new(&mut scope, &context);
            if context.get_slot::<InteropGeneration>().map(|g| g.0) != Some(parent.generation) {
                return Err("dispatch context generation mismatch".into());
            }
            let scope = &mut v8::ContextScope::new(&mut scope, context);
            invoke_domains(scope, dispatch.clone())
        })
        .map_err(|_| "synchronous host isolate unavailable")?
    };
    let result = result?; // Adapter failure leaves the staged native frame uncommitted.
    if phase == 0 {
        INVOCATIONS.with(|i| {
            if let Some(state) = i.borrow_mut().get_mut(&key) {
                state.copy_bookkeeping=copy_bookkeeping.clone();
                state.deliveries = dispatch.deliveries.borrow().clone();
                state.retained_bytes =
                    state.deliveries.capacity() * std::mem::size_of::<Decision>();
            }
        });
        // All callbacks have returned. Revalidate copied host identities, then
        // let the native atomic commit independently revalidate current slots.
        let edits=dispatch.edits.borrow().clone();
        // Validate all scalar/entity identities before any native staging.
        for (value,name) in edits.values() {
            if !matches!(value,ProjectedValue::Copied(_)) {projection::encode(value.clone()).map_err(|e|format!("{name}: {e}"))?;}
        }
        for (selector,(value,name)) in &edits {
            dispatch.frame.write_projected(*selector,value).map_err(|e|format!("{name}: {e}"))?;
        }
        let name=&dispatch.binding.function.canonical_id;
        dispatch.frame.commit_projected(result.action,result.value.as_ref(),runtime::has_copies(&dispatch.binding.function.abi)).map_err(|e|format!("{name}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod proof {
    use super::*;
    pub fn owner_resources(id: &str) -> (usize, usize) {
        (ADAPTERS.with(|a| a.borrow().values().filter(|a| a.instance.parent.id == id).count()),
         SUBSCRIPTIONS.with(|s| s.borrow().values().filter(|s| s.owner.id == id).count()))
    }
    #[test]
    fn copied_v8_roundtrip_is_strict_and_immutable() {
        fn roundtrip(
            scope: &mut v8::PinScope,
            args: v8::FunctionCallbackArguments,
            mut rv: v8::ReturnValue,
        ) {
            let projection = if args.get(1).is_true() {
                "vector"
            } else {
                "string"
            };
            match projected_from_js(scope, args.get(0), "ptr", projection)
                .and_then(|v| projected_to_js(scope, v))
            {
                Ok(v) => rv.set(v),
                Err(e) => throw(scope, e),
            }
        }
        init(frame_tests::logger).unwrap();
        frame_tests::load_body("copy-marshalling", "return {};", "{}");
        HOST.with(|h| {
            let mut host = h.borrow_mut();
            let context = clone_plugin_context("copy-marshalling").unwrap();
            let mut storage = v8::HandleScope::new(&mut host.as_mut().unwrap().isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = v8::Local::new(&mut hs, &context);
            let scope = &mut v8::ContextScope::new(&mut hs, context);
            let f = v8::Function::new(scope, roundtrip).unwrap();
            let key = v8::String::new(scope, "copyValue").unwrap();
            context.global(scope).set(scope, key.into(), f.into());
        });
        let result = eval_in_context(
            "copy-marshalling",
            r#"
            for (const s of ['', 'hello', 'é🔥', 'x'.repeat(511)+'🔥', 'é'.repeat(32767)+'x', 'x'.repeat(65535)]) {
                if(copyValue(s)!==s) throw Error('string copy');
            }
            let touched=0;
            for(const s of ['\ud800','\udfff','nul\0byte','x'.repeat(65536),'é'.repeat(32768),new String('x'),{toString(){touched++;return 'x'}}]) {
                let rejected=false;try{copyValue(s)}catch(_){rejected=true}if(!rejected)throw Error('invalid string accepted');
            }
            const input={x:-0,y:1.25,z:-3};const saved=copyValue(input,true);input.y=100;
            if(!Object.is(saved.x,-0)||saved.y!==1.25||saved.z!==-3||!Object.isFrozen(saved))throw Error('vector copy');
            for(const v of [{x:Infinity,y:0,z:0},{x:1e100,y:0,z:0},{x:'1',y:0,z:0},{get x(){touched++;return 1},y:0,z:0},new Proxy({x:1,y:2,z:3},{ownKeys(){touched++;return ['x','y','z']}}),{x:1,y:2,z:3,w:4},{x:1,y:2,z:3,[Symbol()]:4},[1,2,3]]) {
                let rejected=false;try{copyValue(v,true)}catch(_){rejected=true}if(!rejected)throw Error('invalid vector accepted');
            }
            Object.defineProperty(Object.prototype,'x',{configurable:true,get(){touched++;return 99},set(_){touched++;throw Error('inherited x setter')}});
            try {
                const poisoned=copyValue({x:-0,y:2,z:3},true);
                const desc=Object.getOwnPropertyDescriptor(poisoned,'x');
                if(!desc||!('value' in desc)||!Object.is(desc.value,-0)||!Object.isFrozen(poisoned))throw Error('poisoned vector snapshot');
            } finally { delete Object.prototype.x; }
            if(touched)throw Error('user coercion executed');globalThis.savedCopy=saved;
        "#,
        );
        unload_plugin("copy-marshalling");
        shutdown();
        result.unwrap();
    }
    pub struct CopyConformance {
        package: PreparedPackageReceipt,
        pub ready: bool,
    }
    const COPY_ID: &str = "proof.copied.v1";
    pub(super) fn copy_binding(id: &str) -> u64 {
        prepared_binding(id, |f| {
            f["target"]["pattern"] = "50".into();
            f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:ptr(ptr)".into();
            f["abi"]["parameters"] = serde_json::json!([{"name":"text","native":"ptr","projection":{"id":"string","version":1},"ownership":"callee-retained","mutable":["pre"]}]);
            f["abi"]["returns"] = serde_json::json!({"native":"ptr","projection":{"id":"string","version":1},"ownership":"caller-borrowed"});
        })
    }
    pub fn copy_begin() -> CopyConformance {
        let owner = HostPackageOwner::mint("@proof/copied").unwrap();
        let grant = HostAdapterGrant::override_return(
            &owner,
            AdapterContract {
                id: COPY_ID.into(),
                version: 1,
                contract_hash: HASH.into(),
            },
        )
        .unwrap();
        let source = format!(
            r#"(()=>{{
            const register=__s2_function_adapter_register,subscribe=__s2_function_adapter_subscribe;
            globalThis.mode='probe';globalThis.seen=[];globalThis.rejected=0;
            register('{COPY_ID}','{HASH}',{{
                pre(d){{
                    globalThis.savedFrame=d.frame;
                    if(mode==='stale')return oldDelivery;
                    let first=null,next;
                    while((next=d.cursor.invokeNext())!==null){{if(!first)first=next;}}
                    if(mode==='carry'){{globalThis.oldDelivery=first;return first.action>=2?first:first.action;}}
                    if(mode==='manufacture')return {{action:first.action,returnValue:first.returnValue}};
                    if(mode==='rollback'||mode==='none')return 0;
                    if(mode==='nested'){{const nested=__proofEntityCall(binding,'inner');if(nested!=='inner')throw Error('nested bypass');}}
                }},
                post(d){{
                    if(mode==='probe'){{seen.push('ready');return;}}
                    if(mode==='peer'){{if(d.frame.originalReturnValue!=='input'||d.frame.returnValue!=='late-native-peer')throw Error('native peer POST capture');}}
                    if(mode==='post'){{
                        if(d.frame.originalReturnValue!=='input')throw Error('original capture');
                        globalThis.savedOverride=d.frame.overrideReturn;
                        if(d.frame.overrideReturn('post-owned')!=='post-owned')throw Error('override');
                        if(d.frame.originalReturnValue!=='input')throw Error('original changed');
                    }}
                    while(d.cursor.invokeNext()!==null){{}}
                }}
            }});
            globalThis.subscribeCopy=id=>{{globalThis.binding=id;
                subscribe(id,'{COPY_ID}','pre',v=>{{
                    globalThis.savedValue=v.text;globalThis.savedView=v;
                    if(mode==='rollback'){{v.text='rejected-edit';return {{action:-1,returnValue:'bad'}};}}
                    if(mode==='carry'||mode==='manufacture'){{v.text='winning-edit';return {{action:2,returnValue:'same-copied-result'}};}}
                    return 0;
                }});
                subscribe(id,'{COPY_ID}','post',v=>{{
                    if('overrideReturn' in v)throw Error('subscriber POST authority');
                    if(mode==='post'){{let denied=false;try{{savedOverride('forbidden')}}catch(_){{denied=true}}if(!denied)throw Error('suspended permit');}}
                    seen.push(v.returnValue);
                }});
            }};
        }})()"#
        );
        let package = register_prepared_package_with_authorities(
            owner,
            source.into(),
            ImplementationManifestHash::new(crate::engine_functions::contract::hash_bytes(
                b"copied-fixture-v1",
            ))
            .unwrap(),
            vec![grant],
        )
        .unwrap();
        for id in ["copy-a", "copy-b", "copy-caller"] {
            frame_tests::load_body(id, "return {};", "{}");
            entity_native(id);
            let binding = copy_binding(id);
            eval_in_context(id, &format!("globalThis.binding={binding}n;")).unwrap();
            if id != "copy-caller" {
                let owner = OwnerKey::plugin(id, plugin_generation(id));
                authorize_binding(&package, &owner, binding, COPY_ID, HASH).unwrap();
                eval_in_context(id, "subscribeCopy(binding);").unwrap();
            }
        }
        eval_in_context(
            "copy-caller",
            "if(__proofEntityCall(binding,'input')!=='input')throw Error('native alias');",
        )
        .unwrap();
        CopyConformance {
            package,
            ready: false,
        }
    }
    pub fn copy_probe(state: &mut CopyConformance) {
        eval_in_context("copy-a", "seen.length=0;").unwrap();
        eval_in_context(
            "copy-caller",
            "if(__proofEntityCall(binding,'input')!=='input')throw Error('probe alias');",
        )
        .unwrap();
        state.ready = eval_in_context(
            "copy-a",
            "if(!seen.includes('ready'))throw Error('not ready');",
        )
        .is_ok();
    }
    pub fn copy_mode(mode: &str) {
        for id in ["copy-a", "copy-b"] {
            eval_in_context(id, &format!("mode='{mode}';seen.length=0;")).unwrap();
        }
    }
    pub fn copy_exercise() {
        for (mode, expected) in [
            ("rollback", "input"),
            ("nested", "input"),
            ("carry", "same-copied-result"),
            ("manufacture", "same-copied-result"),
            ("post", "post-owned"),
        ] {
            copy_mode(mode);
            eval_in_context("copy-caller",&format!("globalThis.result=__proofEntityCall(binding,'input');if(result!=='{expected}')throw Error('copied result: '+result);")).unwrap();
            eval_in_context("copy-a","{if(savedValue!=='input')throw Error('immutable PRE snapshot');let denied=0;try{savedView.text}catch(_){denied++}try{savedFrame.text}catch(_){denied++}if(denied!==2)throw Error('retained lease');}").unwrap();
            if mode == "post" {
                eval_in_context("copy-a","{let denied=false;try{savedOverride('bad')}catch(_){denied=true}if(!denied)throw Error('POST permit survived');}").unwrap();
            }
            println!("PASS copied shared V8 mode={mode}: result={expected}, expired accessors");
        }
        copy_mode("stale");
        // A prior delivery's private tag cannot recover its original lineage.
        copy_expect_delivery_refusal("copy-caller", "input");
        copy_mode("probe");
        eval_in_context("copy-caller", "if(__proofEntityCall(binding,'input')!=='input')throw Error('call after stale refusal');").unwrap();
        assert_eq!(pending_invocations(), 0);
        println!("PASS copied shared V8 stale delivery: named invocation refusal, POST cleanup, subsequent valid call");
    }
    pub fn copy_expect_delivery_refusal(parent: &str, input: &str) {
        DISPATCH_ERRORS.with(|errors| errors.borrow_mut().clear());
        eval_in_context(parent, &format!(r#"{{
            let failure='';
            try {{ __proofEntityCall(binding,'{input}'); }} catch(error) {{ failure=String(error); }}
            if(!failure.includes('FunctionCopyInvocationFailure') ||
               !failure.includes('synchronous core function dispatch failed'))
                throw Error('stale/foreign delivery invocation did not fail: '+failure);
        }}"#)).unwrap();
        let errors = DISPATCH_ERRORS.with(|errors| errors.borrow_mut().drain(..).collect::<Vec<_>>());
        assert!(errors.iter().any(|error| error == "foreign or expired host delivery"), "missing exact delivery rejection: {errors:?}");
        assert_eq!(pending_invocations(), 0, "failed call retained PRE state after POST cleanup");
    }
    pub fn copy_peer_exercise(){
        copy_mode("peer");
        eval_in_context("copy-caller","if(__proofEntityCall(binding,'input')!=='late-native-peer')throw Error('native peer final');").unwrap();
        eval_in_context("copy-a","if(!seen.includes('late-native-peer'))throw Error('peer observation');").unwrap();
    }
    pub fn copy_abort(state: CopyConformance) {
        for id in ["copy-a", "copy-b", "copy-caller"] {
            unload_plugin(id);
        }
        drop(state.package);
    }
    pub struct CopyProcess {
        active: registry::ActivePackageFunctions,
        source: PreparedPackageReceipt,
    }
    pub fn copy_process_begin() -> CopyProcess {
        use crate::engine_functions::{contract, overrides, tests};
        let host = HostPackageOwner::mint("@proof/copied-process").unwrap();
        let mut bundle = tests::fixture();
        bundle["ownerId"] = host.key().id.clone().into();
        let f = &mut bundle["functions"][0];
        f["canonicalId"] = format!("{}::fire", host.key().id).into();
        f["requirement"] = "required".into();
        f["target"]["pattern"] = "50".into();
        f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:ptr(ptr)".into();
        f["abi"]["parameters"] = serde_json::json!([{"name":"text","native":"ptr","projection":{"id":"string","version":1},"ownership":"callee-retained","mutable":["pre"]}]);
        f["abi"]["returns"] = serde_json::json!({"native":"ptr","projection":{"id":"string","version":1},"ownership":"caller-borrowed"});
        f["policy"]["surfaces"] = serde_json::json!(["call", "pre", "post"]);
        f["policy"]["suppression"] = "none".into();
        tests::seal(&mut bundle);
        let mut summary = tests::summary(&bundle);
        summary["functions"][0]["mutates"] = true.into();
        let parsed = contract::parse(
            &bundle.to_string(),
            &host.key().id,
            &summary,
            &["engine:calls".into(), "engine:hooks".into()],
        )
        .unwrap();
        let candidate = overrides::prepare(parsed, "copied-process-fixture", vec![]).unwrap();
        let active = registry::activate_package_owner(
            registry::prepare_package_owner(&host, candidate).unwrap(),
            &host,
        )
        .unwrap();
        let source=register_prepared_package(host,r#"
            globalThis.processCopy=__s2_package_function('fire');globalThis.edit=null;globalThis.seen=0;
            globalThis.pre=processCopy.onPre(v=>{seen++;globalThis.saved=v.text;if(edit!==null)v.text=edit;});
            globalThis.post=processCopy.onPost(v=>{globalThis.result=v.returnValue;});
        "#.into(),ImplementationManifestHash::new(HASH.into()).unwrap()).unwrap();
        for id in ["copy-process-a", "copy-process-b"] {
            frame_tests::load_body(id, "return {};", "{}");
            assert!(
                registry::owner_bindings(&OwnerKey::plugin(id, plugin_generation(id))).is_empty()
            );
        }
        CopyProcess { active, source }
    }
    pub fn copy_process_probe() -> bool {
        eval_in_context(
            "copy-process-a",
            "if(processCopy.call('parent-a')!=='parent-a')throw Error('process alias');",
        )
        .unwrap();
        eval_in_context("copy-process-b", "if(seen===0)throw Error('pending');").is_ok()
    }
    pub fn copy_process_exercise() {
        eval_in_context("copy-process-a", "edit='from-a';").unwrap();
        eval_in_context("copy-process-b", "edit='from-b';").unwrap();
        eval_in_context(
            "copy-process-a",
            "if(processCopy.call('parent-a')!=='from-b')throw Error('other parent B mutation');",
        )
        .unwrap();
        eval_in_context(
            "copy-process-b",
            "if(processCopy.call('parent-b')!=='from-a')throw Error('other parent A mutation');",
        )
        .unwrap();
    }
    pub fn copy_process_finish(state: CopyProcess) {
        drop(state.active);
        drop(state.source);
        for id in ["copy-process-a", "copy-process-b"] {
            let result=eval_in_context(id,"{let refused=false;try{processCopy.call('gone')}catch(_){refused=true}if(!refused||typeof result!=='string')throw Error('retired copy binding');}");
            unload_plugin(id);
            result.unwrap();
        }
    }
    pub fn copy_process_abort(state: CopyProcess) {
        for id in ["copy-process-a", "copy-process-b"] {
            unload_plugin(id);
        }
        drop(state.active);
        drop(state.source);
    }
    pub fn copy_borrowed_begin() {
        frame_tests::load_body("copy-vector", "return {};", "{}");
        entity_native("copy-vector");
        let binding = prepared_binding("copy-vector", |f| {
            f["target"]["pattern"] = "50".into();
            f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:ptr(ptr)".into();
            f["abi"]["parameters"] = serde_json::json!([{"name":"vector","native":"ptr","projection":{"id":"vector","version":1},"ownership":"callee-borrowed","mutable":[]}]);
            f["abi"]["returns"] = serde_json::json!({"native":"ptr","projection":{"id":"vector","version":1},"ownership":"caller-borrowed"});
        });
        eval_in_context("copy-vector",&format!("{{const v=__proofEntityCall({binding}n,{{x:-0,y:1.25,z:-3}});if(!Object.is(v.x,-0)||v.y!==1.25||v.z!==-3||!Object.isFrozen(v))throw Error('native vector alias');}}")).unwrap();
        unload_plugin("copy-vector");
        for id in ["copy-borrowed", "copy-borrowed-caller"] {
            frame_tests::load_body(id, "return {};", "{}");
            entity_native(id);
            let binding = prepared_binding(id, |f| {
                f["target"]["pattern"] = "52".into();
                f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:i32(ptr)".into();
                f["abi"]["parameters"] = serde_json::json!([{"name":"text","native":"ptr","projection":{"id":"string","version":1},"ownership":"callee-borrowed","mutable":["pre"]}]);
                f["policy"]["suppression"] = "none".into();
            });
            eval_in_context(
                id,
                &format!(
                    "globalThis.binding={binding}n;globalThis.invalid=false;globalThis.seen=0;"
                ),
            )
            .unwrap();
            if id == "copy-borrowed" {
                eval_in_context(id,"globalThis.receipt=__proofSubscribeGeneric(binding,'pre',false,v=>{seen++;globalThis.saved=v.text;v.text=invalid?'rejected':'continued';if(invalid)return {action:2,returnValue:33};return 0;});").unwrap();
            }
        }
    }
    pub fn copy_borrowed_probe() -> bool {
        eval_in_context(
            "copy-borrowed-caller",
            "globalThis.result=__proofEntityCall(binding,'outer');",
        )
        .unwrap();
        eval_in_context(
            "copy-borrowed-caller",
            "if(result!==9)throw Error('not ready');",
        )
        .is_ok()
    }
    pub fn copy_borrowed_finish() {
        eval_in_context(
            "copy-borrowed",
            "if(saved!=='outer')throw Error('borrowed snapshot');invalid=true;",
        )
        .unwrap();
        eval_in_context(
            "copy-borrowed-caller",
            "if(__proofEntityCall(binding,'outer')!==5)throw Error('suppression:none rollback');",
        )
        .unwrap();
        for id in ["copy-borrowed", "copy-borrowed-caller"] {
            unload_plugin(id);
        }
    }
    pub fn copy_finish(state: CopyConformance) {
        // Another plugin keeps independent JavaScript-owned results across source owner retirement.
        for id in ["copy-a", "copy-b"] {
            unload_plugin(id);
        }
        let result = eval_in_context(
            "copy-caller",
            "if(result!=='post-owned')throw Error('saved result after retirement');",
        );
        unload_plugin("copy-caller");
        drop(state.package);
        assert_eq!(pending_invocations(), 0);
        result.unwrap();
    }

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
    // Both native Service and portable host tests execute this exact package body.
    pub struct PostConformance {
        pub package: PreparedPackageReceipt,
        pub ready: bool,
    }
    fn js_rust_post(
        scope: &mut v8::PinScope,
        args: v8::FunctionCallbackArguments,
        _: v8::ReturnValue,
    ) {
        struct RustPost {
            original: ProjectedValue,
            desired: ProjectedValue,
        }
        fn same(a: ProjectedValue, b: ProjectedValue) -> bool {
            match (a, b) {
                (ProjectedValue::Scalar(a), ProjectedValue::Scalar(b)) => {
                    a.kind == b.kind && a.bits == b.bits
                }
                (
                    ProjectedValue::Entity { reference: a, .. },
                    ProjectedValue::Entity { reference: b, .. },
                ) => a == b,
                _ => false,
            }
        }
        impl DispatchAdapter for RustPost {
            fn pre(
                &self,
                _: &mut package_adapter::AdapterDispatch<'_>,
            ) -> Result<package_adapter::PreDecision, String> {
                Err("proof is POST-only".into())
            }
            fn post(&self, d: &mut package_adapter::AdapterDispatch<'_>) -> Result<(), String> {
                let frame = d.frame.as_deref_mut().ok_or("missing guarded frame")?;
                if !frame
                    .original_return()?
                    .is_some_and(|v| same(v, self.original.clone()))
                {
                    return Err("Rust original snapshot mismatch".into());
                }
                for malformed in [
                    S2FunctionValue {
                        kind: 2,
                        flags: 1,
                        reserved: 0,
                        aux: 0,
                        bits: 41,
                    },
                    S2FunctionValue {
                        kind: 2,
                        flags: 0,
                        reserved: 1,
                        aux: 0,
                        bits: 41,
                    },
                    S2FunctionValue {
                        kind: 2,
                        flags: 0,
                        reserved: 0,
                        aux: 1,
                        bits: 41,
                    },
                    S2FunctionValue {
                        kind: 2,
                        flags: 0,
                        reserved: 0,
                        aux: 0,
                        bits: u64::MAX,
                    },
                    // A Scalar POD cannot impersonate a host EntityReference, even
                    // when its index/serial happen to identify a currently live slot.
                    S2FunctionValue {
                        kind: 8,
                        flags: 1,
                        reserved: 0,
                        aux: 901,
                        bits: 71,
                    },
                    S2FunctionValue {
                        kind: 8,
                        flags: 2,
                        reserved: 0,
                        aux: 901,
                        bits: 71,
                    },
                ] {
                    if frame
                        .override_return(ProjectedValue::Scalar(malformed))
                        .is_ok()
                    {
                        return Err("malformed Rust effect accepted".into());
                    }
                }
                if !same(frame.override_return(self.desired.clone())?, self.desired.clone()) {
                    return Err("Rust effective mismatch".into());
                }
                while d.cursor.invoke_next()?.is_some() {}
                if !d
                    .frame
                    .as_deref_mut()
                    .unwrap()
                    .original_return()?
                    .is_some_and(|v| same(v, self.original.clone()))
                {
                    return Err("Rust original changed".into());
                }
                Ok(())
            }
        }
        let result = (|| {
            let id = LEASES
                .with(|s| s.borrow().last().map(|l| l.id))
                .ok_or("missing adapter lease")?;
            let l = lease(scope, id)?;
            let mut frame = GuardedPostFrame {
                permit: AdapterPostReturnPermit::issue(l.clone())?,
            };
            let ret = &l.binding.function.abi.returns;
            let (original, desired) = if ret.native == "ptr" {
                let context = scope.get_current_context();
                let global = context.global(scope);
                let a = get(scope, global, "a")?;
                (
                    projected_from_js(scope, a, &ret.native, &ret.projection.id)?,
                    projected_from_js(scope, args.get(0), &ret.native, &ret.projection.id)?,
                )
            } else {
                let mut a = runtime::blank();
                a.kind = 2;
                a.bits = 7;
                let mut b = a;
                b.bits = 41;
                (ProjectedValue::Scalar(a), ProjectedValue::Scalar(b))
            };
            let mut cursor = GenericCursor {
                adapter_owner: Some(l.owner),
                scope,
                dispatch: l.dispatch,
            };
            RustPost { original, desired }.post(&mut package_adapter::AdapterDispatch {
                cursor: &mut cursor,
                frame: Some(&mut frame),
            })
        })();
        if let Err(e) = result {
            throw(scope, e)
        }
    }
    fn js_publish_post_methods(
        scope: &mut v8::PinScope,
        args: v8::FunctionCallbackArguments,
        _: v8::ReturnValue,
    ) {
        let method = v8::Global::new(scope, args.get(0));
        let getter = v8::Global::new(scope, args.get(1));
        let context = clone_plugin_context("post-observer").unwrap();
        let context = v8::Local::new(scope, &context);
        let scope = &mut v8::ContextScope::new(scope, context);
        let global = context.global(scope);
        let method = v8::Local::new(scope, &method);
        let getter = v8::Local::new(scope, &getter);
        set(scope, global, "borrowed", method).unwrap();
        set(scope, global, "borrowedOriginal", getter).unwrap();
    }

    pub fn post_begin() -> PostConformance {
        const ID: &str = "proof.trusted-post.v1";
        let owner = HostPackageOwner::mint("@proof/trusted-post-shared").unwrap();
        let grant = HostAdapterGrant::override_return(
            &owner,
            AdapterContract {
                id: ID.into(),
                version: 1,
                contract_hash: HASH.into(),
            },
        )
        .unwrap();
        let source = format!(
            r#"(()=>{{
          const register=__s2_function_adapter_register,subscribe=__s2_function_adapter_subscribe;
          globalThis.events=[];globalThis.mode='probe';globalThis.expected=41;globalThis.original=7;
          globalThis.adapterReceipt=register('{ID}','{HASH}',{{
            pre(d){{
              if('overrideReturn' in d.frame || 'originalReturnValue' in d.frame)throw Error('PRE authority');
              if(globalThis.retained){{let denied=false;try{{retained(999)}}catch(_){{denied=true}}if(!denied)throw Error('old permit revived');}}
              if(mode==='skip')return {{action:2,returnValue:63}};
            }},
            post(d){{
              if(mode==='probe'){{events.push('ready');return;}}
              if(d.frame.originalReturnValue!==original)throw Error('original snapshot');
              globalThis.retained=d.frame.overrideReturn;
              globalThis.retainedOriginal=Object.getOwnPropertyDescriptor(d.frame,'originalReturnValue').get;
              if(mode==='rust'){{__proofRustPost();events.push('adapter');return;}}
              if(mode==='nested'){{
                // Caller bypass leaves a different instance selected during this nested frame.
                __proofPublishPost(retained,retainedOriginal);
                const nested=__proofEntityCall(globalThis.localBinding,3);
                if(nested!==3)throw Error('nested original');
              }}
              if(retained(41)!==expected || d.frame.returnValue!==expected)throw Error('provider current');
              if(d.frame.originalReturnValue!==original)throw Error('original changed');
              while(d.cursor.invokeNext()!==null){{}}
              if(retained(43)!==expected)throw Error('equal Override replaced first winner');
              if(mode==='throw')throw Error('after effect');
              events.push('adapter');
            }}
          }});
          globalThis.subscribePost=id=>{{
            globalThis.localBinding=id;
            globalThis.preReceipt=subscribe(id,'{ID}','pre',()=>{{}});
            globalThis.postReceipt=subscribe(id,'{ID}','post',v=>{{
              if('originalReturnValue' in v || 'overrideReturn' in v)throw Error('public grant');
              let refused=0;try{{retained(99)}}catch(_){{refused++}}try{{retainedOriginal()}}catch(_){{refused++}}
              try{{v.returnValue=99}}catch(_){{refused++}}
              if(refused!==3 || v.returnValue!==expected)throw Error('subscriber lease or timing');
              events.push('named:'+v.returnValue);
            }});
          }};
        }})()"#
        );
        let package = register_prepared_package_with_authorities(
            owner,
            source.into(),
            ImplementationManifestHash::new(HASH.into()).unwrap(),
            vec![grant],
        )
        .unwrap();
        for id in ["post-adapter", "post-caller", "post-observer"] {
            frame_tests::load_body(id, "return {};", "{}");
            entity_native(id);
            let binding = prepared_binding(id, |_| {});
            eval_in_context(id, &format!("globalThis.binding={binding}n;")).unwrap();
            if id == "post-adapter" {
                authorize_binding(
                    &package,
                    &OwnerKey::plugin(id, plugin_generation(id)),
                    binding,
                    ID,
                    HASH,
                )
                .unwrap();
                eval_in_context(id, &format!("subscribePost({binding}n);")).unwrap();
                with_host_isolate(|isolate| {
                    let mut storage = v8::HandleScope::new(isolate);
                    let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
                    let context = clone_plugin_context(id).unwrap();
                    let context = v8::Local::new(&mut hs, &context);
                    let scope = &mut v8::ContextScope::new(&mut hs, context);
                    let f = v8::Function::new(scope, js_rust_post).unwrap();
                    let global = context.global(scope);
                    set(scope, global, "__proofRustPost", f.into()).unwrap();
                })
                .unwrap();
            } else if id == "post-observer" {
                eval_in_context(id,"__proofSubscribeGeneric(binding,'post',true,v=>{if('overrideReturn' in v || 'originalReturnValue' in v)throw Error('generic authority');if(v.returnValue===3){let n=0;try{borrowed(99)}catch(_){n++}try{borrowedOriginal()}catch(_){n++}if(n!==2)throw Error('nested cross-context permit');events.push('nested-refused');return;}events.push(v.returnValue);});").unwrap();
            }
        }
        PostConformance {
            package,
            ready: false,
        }
    }
    pub fn post_probe(state: &mut PostConformance) {
        eval_in_context("post-adapter", "events.length=0;").unwrap();
        eval_in_context("post-caller", "__proofEntityCall(binding,7);").unwrap();
        state.ready = eval_in_context(
            "post-adapter",
            "if(events.join(',')!=='ready')throw Error('not ready');",
        )
        .is_ok();
    }
    pub fn post_exercise(mode: &str, expected: i32, original: Option<i32>) {
        let original = original
            .map(|v| v.to_string())
            .unwrap_or("undefined".into());
        eval_in_context(
            "post-adapter",
            &format!("mode='{mode}';expected={expected};original={original};events.length=0;"),
        )
        .unwrap();
        eval_in_context("post-observer", "events.length=0;").unwrap();
        eval_in_context("post-caller",&format!("if(__proofEntityCall(binding,7)!=={expected})throw Error('final provider result');")).unwrap();
        eval_in_context("post-adapter",&format!("if(events.join(',')!=='named:{expected},adapter')throw Error(events);{{let n=0;try{{retained(99)}}catch(_){{n++}}try{{retainedOriginal()}}catch(_){{n++}}if(n!==2)throw Error('expired permit');}}")).unwrap();
        let observer_expected=if mode=="nested" {format!("nested-refused,{expected}")}else{expected.to_string()};
        eval_in_context(
            "post-observer",
            &format!("if(events.join(',')!=='{observer_expected}')throw Error('generic current:'+events);"),
        )
        .unwrap();
        assert_eq!(pending_invocations(), 0);
        println!("PASS trusted POST {mode}: original={original}, current/final={expected}, named/generic observation, expired and suspended leases");
    }
    pub fn post_finish(state: PostConformance) {
        for id in ["post-adapter", "post-caller", "post-observer"] {
            unload_plugin(id);
        }
        drop(state.package);
        assert_eq!(pending_invocations(), 0);
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
        let receipt = registry::prepare_owner(&owner.id, candidate).unwrap();
        registry::activate_owner(receipt, owner).unwrap()[0]
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
            receipt(scope, id, true, None)
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
            let _copy_scope=copied::Scope::enter()?;
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
            let value = crate::nest::with_outbound(&args, || {
                runtime::call_binding(&binding, &owner, &values)
            })?;
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
            let rust_post = v8::Function::new(scope, js_rust_post).unwrap();
            set(scope, global, "__proofRustPost", rust_post.into()).unwrap();
            let publish = v8::Function::new(scope, js_publish_post_methods).unwrap();
            set(scope, global, "__proofPublishPost", publish.into()).unwrap();
        })
        .unwrap();
    }
    pub(crate) fn entity_binding(id: &str, nullable: bool, writable: bool, receiver: bool) -> u64 {
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
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum EntityStage {
        Identity,
        Member,
        Done,
    }
    // Only owned fixture IDs/control survive returned frames; no V8 or frame lease.
    pub struct EntityConformance {
        nullable_first: bool,
        reverse_sub: bool,
        slot: EntitySlot,
        a: u64,
        b: u64,
        target: i64,
        receipt: Option<u64>,
        native_status: bool,
        pub stage: EntityStage,
        pub ready: bool,
        pub detail: String,
    }
    impl EntityConformance {
        fn owners(&self) -> &[&str] {
            match self.stage {
                EntityStage::Identity => &["entity-strict", "entity-nullable"],
                EntityStage::Member => &["entity-member"],
                EntityStage::Done => panic!("completed entity fixture has no probe"),
            }
        }
        fn status(&self) -> Option<S2FunctionHookStatus> {
            if engine_ops().and_then(|ops| ops.function_hook_status).is_none() {
                assert!(!self.native_status, "native entity receipt unavailable");
                return None;
            }
            let status = runtime::status(self.target).unwrap();
            assert!(matches!(status.state, 1 | 2), "entity target {} unexpected state {}", self.target, status.state);
            assert_ne!(status.receipt, 0, "entity target {} missing receipt", self.target);
            assert_eq!(status.reserved, 0);
            if let Some(receipt) = self.receipt {
                assert_eq!(status.receipt, receipt, "entity target {} receipt changed", self.target);
            }
            Some(status)
        }
        fn snapshots(&self) -> Vec<serde_json::Value> {
            self.owners().iter().map(|id| {
                serde_json::from_str(&frame_tests::eval_in_context_string(
                    id, "JSON.stringify({seen,pre:phaseCounts.pre,post:phaseCounts.post})",
                )).unwrap()
            }).collect()
        }
        fn diagnostic(&self, point: &str) -> String {
            let status = self.status().map(|s| format!("state={} receipt={}", s.state, s.receipt))
                .unwrap_or_else(|| "status unavailable: explicit host transport".into());
            let detail = format!(
                "entity order nullable-first={} reverse-sub={} stage={:?} point={point} target={} {status} owners={:?} traces={:?} pending={}",
                self.nullable_first, self.reverse_sub, self.stage, self.target,
                self.owners(), self.snapshots(), pending_invocations()
            );
            println!("DIAG {detail}");
            detail
        }
        fn clear_probe(&self) {
            for id in self.owners() {
                eval_in_context(id, "seen.length=0;phaseCounts.pre=0;phaseCounts.post=0;delete globalThis.saved;delete globalThis.copy;delete globalThis.memberView;").unwrap();
            }
        }
        fn call_member(&self) -> Result<(), String> {
            let receiver = projection::encode(
                EntityProjection::Strict
                    .value(Some(projection::EntityReference { index: 901, id: self.a }))?
            )?;
            let mut arg = runtime::blank();
            arg.kind = 2;
            arg.bits = 3;
            assert_eq!(runtime::call(self.target, None, &[receiver, arg])?.bits, 13);
            Ok(())
        }
    }
    /// One harmless call per returned-frame probe. Only Pending + zero delivery retries.
    pub fn entity_probe(state: &mut EntityConformance) {
        assert!(!state.ready);
        state.clear_probe();
        state.diagnostic("before probe");
        let result = match state.stage {
            EntityStage::Identity => eval_in_context("entity-caller", "if(call(a).id!==a.id)throw Error('probe identity');"),
            EntityStage::Member => state.call_member(),
            EntityStage::Done => panic!("probe after entity cleanup"),
        };
        state.detail = state.diagnostic("after probe");
        result.unwrap_or_else(|error| panic!("{error}; {}", state.detail));
        assert_eq!(pending_invocations(), 0, "{}", state.detail);
        let snapshots = state.snapshots();
        let expected = match state.stage {
            EntityStage::Identity => serde_json::json!({"seen":[state.a,format!("post:{}",state.a)],"pre":1,"post":1}),
            EntityStage::Member => serde_json::json!({"seen":["receiver","receiver-post"],"pre":1,"post":1}),
            EntityStage::Done => unreachable!(),
        };
        let complete = snapshots.iter().all(|v| *v == expected);
        let empty = snapshots.iter().all(|v| *v == serde_json::json!({"seen":[],"pre":0,"post":0}));
        match state.status() {
            Some(status) if status.state == 1 && empty => return,
            Some(status) => assert!(status.state == 2 && complete, "{}", state.detail),
            None => assert!(complete, "{}", state.detail),
        }
        state.ready = true;
        println!("PASS entity readiness {}", state.detail);
    }
    /// Same staged semantic body for host transport and real native outer frames.
    pub fn entity_begin(nullable_first: bool, reverse_sub: bool, slot: EntitySlot, native_status: bool) -> EntityConformance {
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
        let target = lookup("entity-strict", strict).target.unwrap();
        let mut state = EntityConformance {
            nullable_first, reverse_sub, slot, a, b, target, receipt: None,
            native_status, stage: EntityStage::Identity, ready: false, detail: String::new(),
        };
        state.receipt = state.status().map(|s| s.receipt);
        state.detail = state.diagnostic("subscribed");
        assert_eq!(pending_invocations(), 0);
        state
    }
    pub fn entity_advance(state: &mut EntityConformance) {
        assert!(state.ready, "advance before readiness: {}", state.detail);
        state.clear_probe();
        state.ready = false;
        match state.stage {
            EntityStage::Identity => entity_identity_exercise(state),
            EntityStage::Member => entity_member_finish(state),
            EntityStage::Done => panic!("advance after entity cleanup"),
        }
        assert_eq!(pending_invocations(), 0);
    }
    fn entity_identity_exercise(state: &mut EntityConformance) {
        let reverse_sub = state.reverse_sub;
        let slot = state.slot;
        let b = state.b;
        let seed = |index, serial| {
            assert_eq!(unsafe { slot(index, serial, 1) }, 1);
            crate::entity_live::on_created(index, serial as i32)
        };
        for (stage, code) in [
            ("call(a)", "if(call(a).id!==a.id)throw Error('callback identity');"),
            ("call(null)", "if(call(null)!==null)throw Error('null callback');"),
        ] {
            let result = eval_in_context("entity-caller", code);
            let detail = state.diagnostic(stage);
            result.unwrap_or_else(|error| panic!("{error}; {detail}"));
        }
        let detail = state.diagnostic("before strict assertions");
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
        entity_post_conformance(a,replacement,slot);
        let member = entity_binding("entity-member", false, false, true);
        eval_in_context("entity-member",&format!("if(__proofEntityCall({member}n,a,3)!==13)throw Error('receiver convention');let refused=false;try{{__proofEntityCall({member}n,null,3)}}catch(_){{refused=true}}if(!refused)throw Error('nullable receiver');")).unwrap();
        eval_in_context("entity-member",&format!(r#"
            globalThis.memberPre=__proofSubscribeGeneric({member}n,'pre',true,v=>{{
                phaseCounts.pre++;globalThis.memberView=v;if(v.self.id!==a.id||v.required!==3)throw Error('receiver view');
                let refused=false;try{{v.self=b}}catch(_){{refused=true}}if(!refused)throw Error('receiver writable');seen.push('receiver');
            }});
            globalThis.memberPost=__proofSubscribeGeneric({member}n,'post',true,v=>{{phaseCounts.post++;if(v.self.id!==a.id||v.returnValue!==13)throw Error('receiver POST');seen.push('receiver-post')}});
        "#)).unwrap();
        state.a = a;
        state.b = replacement;
        state.target = registry::binding(member, &OwnerKey::plugin("entity-member", plugin_generation("entity-member"))).unwrap().target.unwrap();
        state.stage = EntityStage::Member;
        state.receipt = None;
        state.receipt = state.status().map(|s| s.receipt);
        state.detail = state.diagnostic("subscribed");
    }
    fn entity_post_conformance(a: u64, b: u64, slot: EntitySlot) {
        const ID: &str = "proof.entity-post.v1";
        let owner = HostPackageOwner::mint("@proof/entity-post").unwrap();
        let grant = HostAdapterGrant::override_return(
            &owner,
            AdapterContract {
                id: ID.into(),
                version: 1,
                contract_hash: HASH.into(),
            },
        )
        .unwrap();
        let source = format!(
            r#"(()=>{{
          const subscribe=__s2_function_adapter_subscribe;
          globalThis.mode='normal';globalThis.seen=[];
          __s2_function_adapter_register('{ID}','{HASH}',{{
            pre(d){{if(mode==='skip')return {{action:2,returnValue:b}};}},
            post(d){{
              const original=d.frame.originalReturnValue;
              if(mode==='skip'){{if(original!==undefined)throw Error('skipped original not undefined');}}
              else if(mode==='null-original'){{if(original!==null)throw Error('nullable original not null');}}
              else if(original?.id!==a.id)throw Error('entity original');
              if(mode==='rust'){{__proofRustPost(b);seen.push('adapter');return;}}
              let result;
              if(mode==='invalid'){{
                let refused=0;for(const value of [null,17,{{index:902,id:b.id}}]){{try{{d.frame.overrideReturn(value)}}catch(_){{refused++}}}}
                if(refused!==3 || d.frame.returnValue.id!==a.id)throw Error('strict invalid effect');
              }}
              if(mode==='books'||mode==='native'){{
                __proofEntityDelete(902,73,mode);
                let refused=false;try{{d.frame.overrideReturn(b)}}catch(e){{refused=String(e).includes('entity-post::fire');}}
                if(!refused || d.frame.returnValue.id!==a.id)throw Error('stale entity effect');
                seen.push('refused');return;
              }}
              result=d.frame.overrideReturn(mode==='null-effect'?null:b);
              if(mode==='null-effect'){{if(result!==null || d.frame.returnValue!==null)throw Error('nullable override');}}
              else if(result.id!==b.id || d.frame.returnValue.id!==b.id)throw Error('entity override');
              const after=d.frame.originalReturnValue;
              if(after!==original && after?.id!==original?.id)throw Error('original snapshot changed');
              while(d.cursor.invokeNext()!==null){{}}
              seen.push('adapter');
            }}
          }});
          globalThis.subscribeEntity=id=>{{
            globalThis.pre=subscribe(id,'{ID}','pre',()=>{{}});
            globalThis.post=subscribe(id,'{ID}','post',v=>{{
              if('overrideReturn' in v || 'originalReturnValue' in v)throw Error('entity subscriber grant');
              if(mode==='null-effect'?v.returnValue!==null:v.returnValue.id!==b.id)throw Error('entity observer');
              seen.push('named');
            }});
          }};
        }})()"#
        );
        let package = register_prepared_package_with_authorities(
            owner,
            source.into(),
            ImplementationManifestHash::new(HASH.into()).unwrap(),
            vec![grant],
        )
        .unwrap();
        let mut b = b;
        // Existing generic observers keep the proven physical hook alive throughout.
        eval_in_context("entity-strict", "pre.dispose();").unwrap();
        for nullable in [false, true] {
            frame_tests::load_body("entity-post", "return {};", "{}");
            entity_native("entity-post");
            eval_in_context("entity-post",&format!("globalThis.E=__s2require('@s2script/entity').EntityRef;globalThis.a=new E(901,{a});globalThis.b=new E(902,{b});")).unwrap();
            let binding = entity_binding("entity-post", nullable, false, false);
            authorize_binding(
                &package,
                &OwnerKey::plugin("entity-post", plugin_generation("entity-post")),
                binding,
                ID,
                HASH,
            )
            .unwrap();
            eval_in_context("entity-post", &format!("subscribeEntity({binding}n);")).unwrap();
            for mode in if nullable {
                vec!["normal", "rust", "null-effect", "null-original", "skip"]
            } else {
                vec!["normal", "rust", "invalid", "native", "books"]
            } {
                eval_in_context("entity-post", &format!("mode='{mode}';seen.length=0;")).unwrap();
                let (input, want) = match mode {
                    "null-original" => ("null", "b"),
                    "null-effect" => ("a", "null"),
                    "native" | "books" => ("a", "a"),
                    _ => ("a", "b"),
                };
                eval_in_context("entity-caller",&format!("if(call({input})?.id!=={want}?.id)throw Error('entity POST final {mode}');")).unwrap();
                eval_in_context(
                    "entity-post",
                    if matches!(mode, "native" | "books") {
                        "if(seen.join(',')!=='refused')throw Error(seen);"
                    } else {
                        "if(seen.join(',')!=='named,adapter')throw Error(seen);"
                    },
                )
                .unwrap();
                assert_eq!(unsafe { slot(902, 73, 1) }, 1);
                let current = crate::entity_live::on_created(902, 73);
                b = current;
                for id in [
                    "entity-post",
                    "entity-caller",
                    "entity-strict",
                    "entity-nullable",
                ] {
                    eval_in_context(id, &format!("globalThis.b=new E(902,{current});")).unwrap();
                }
                assert_eq!(pending_invocations(), 0);
                println!("PASS trusted entity POST nullable={nullable} mode={mode}: exact binding, original/current, observer timing");
            }
            unload_plugin("entity-post");
        }
        drop(package);
    }

    fn entity_member_finish(state: &mut EntityConformance) {
        let nullable_first = state.nullable_first;
        let reverse_sub = state.reverse_sub;
        state.call_member().unwrap();
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
        state.stage = EntityStage::Done;
    }
    pub fn entity_conformance(nullable_first: bool, reverse_sub: bool, slot: EntitySlot) {
        let mut state = entity_begin(nullable_first, reverse_sub, slot, false);
        entity_probe(&mut state);
        entity_advance(&mut state);
        entity_probe(&mut state);
        entity_advance(&mut state);
        assert_eq!(state.stage, EntityStage::Done);
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
            runtime::call(binding.target.unwrap(), None, &[input])
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
            runtime::call(binding.target.unwrap(), None, &[input])
                .unwrap()
                .bits,
            31
        );
        eval_in_context("policy-observer","if(policyEvents.join(',')!=='pre:12,post:31')throw Error(policyEvents);policyEvents.length=0;").unwrap();
        eval_in_context("policy-generic","if(policyEvents.join(',')!=='tail')throw Error(policyEvents);policyEvents.length=0;stopEnabled=true;").unwrap();
        assert_eq!(
            runtime::call(binding.target.unwrap(), None, &[input])
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
pub(crate) mod scalar_transport_tests {
    use super::*;
    #[derive(Clone)]
    struct MockFrame {
        id: u64,
        input: S2FunctionValue,
        output: S2FunctionValue,
        action: i32,
        overridden: bool,
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
                overridden: false,
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
            crate::nest::with_outbound(&args, || runtime::call(1, Some(&owner), &[value])).unwrap();
        rv.set_int32(output.bits as i32);
    }
    thread_local! {static EFFECTS:Cell<usize>=const{Cell::new(0)};}
    extern "C" fn override_return(_:i64,token:u64,_:u64,_:*const i8,value:*const S2FunctionValue,out:*mut S2FunctionValue,_:*mut i8,_:i32)->i32 {
        EFFECTS.with(|c|c.set(c.get()+1));
        STACK.with(|s| {
            let mut s=s.borrow_mut();let f=s.last_mut().unwrap();assert_eq!(f.id,token);
            if !f.overridden && f.action < 2 { f.output=unsafe{*value}; f.overridden=true; }
            unsafe{*out=f.output};1
        })
    }
    pub(crate) fn init_transport() {
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
        ops.function_frame_override_return = Some(override_return);
        EFFECTS.with(|c|c.set(0));
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
                overridden: false,
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
    // Catches grants leaking across contracts, lost lease suspension, and delayed effects.
    #[test]
    fn trusted_post_absent_and_foreign_grants_cannot_be_forged() {
        init_transport();
        let owner = HostPackageOwner::mint("@proof/foreign").unwrap();
        let grant = HostAdapterGrant::override_return(
            &owner,
            AdapterContract {
                id: "proof.review.v1".into(),
                version: 1,
                contract_hash: proof::HASH.into(),
            },
        )
        .unwrap();
        assert!(register_prepared_package_with_authorities(
            HostPackageOwner::mint("@proof/foreign").unwrap(),
            "ignored".into(),
            ImplementationManifestHash::new(proof::HASH.into()).unwrap(),
            vec![grant]
        )
        .is_err());
        let package = review_package(&format!(
            r#"
            register('proof.review.v1','{}',{{postReturnAuthority:'override',post(d){{
                if('originalReturnValue' in d.frame || 'overrideReturn' in d.frame)throw Error('forged authority');events.push('refused');
            }}}},{{id:'proof.review.v1',contractHash:'{}',overrideReturn:true}});
            globalThis.wrapper=()=>{{}};
        "#,
            proof::HASH,
            proof::HASH
        ));
        frame_tests::load_body("no-grant", "return {};", "{}");
        let binding = proof::prepared_binding("no-grant", |_| {});
        authorize(&package, "no-grant", binding, "proof.review.v1").unwrap();
        eval_in_context(
            "no-grant",
            &format!("subscribeProof({binding}n,'proof.review.v1','post');"),
        )
        .unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        assert_eq!(close_frame(&info).output.bits, 7);
        eval_in_context(
            "no-grant",
            "if(events.join(',')!=='refused')throw Error(events);",
        )
        .unwrap();
        assert_eq!(EFFECTS.with(Cell::get), 0);
        unload_plugin("no-grant");
        drop(package);
        set_engine_ops(None);
        shutdown();
    }
    #[test]
    fn trusted_post_shared_production_body() {
        init_transport();
        let mut state = proof::post_begin();
        proof::post_probe(&mut state);
        assert!(state.ready);
        proof::post_exercise("js", 41, Some(7));
        proof::post_exercise("rust", 41, Some(7));
        proof::post_exercise("skip", 63, None);
        proof::post_exercise("nested", 41, Some(7));
        assert_eq!(EFFECTS.with(Cell::get), 7);
        proof::post_finish(state);
        set_engine_ops(None);
        shutdown();
    }
    #[test]
    fn trusted_post_authority_effect_and_lease_boundaries() {
        init_transport();
        let owner = HostPackageOwner::mint("@proof/trusted-post").unwrap();
        let grant = HostAdapterGrant::override_return(
            &owner,
            AdapterContract {
                id: "proof.review.v1".into(),
                version: 1,
                contract_hash: proof::HASH.into(),
            },
        )
        .unwrap();
        let source = format!(
            r#"(()=>{{
            const register=__s2_function_adapter_register,subscribe=__s2_function_adapter_subscribe;
            globalThis.events=[];
            globalThis.receipt=register('proof.review.v1','{}',{{
              pre(d){{if('overrideReturn' in d.frame || 'originalReturnValue' in d.frame)throw Error('PRE authority');}},
              post(d){{
                if(d.frame.originalReturnValue!==7 || d.frame.returnValue!==7)throw Error('initial snapshot');
                globalThis.retained=d.frame.overrideReturn;
                if(globalThis.stale){{let refused=false;try{{stale(90)}}catch(_){{refused=true}}if(!refused)throw Error('old generation revived');}}
                Promise.resolve().then(()=>{{let refused=false;try{{retained(90)}}catch(_){{refused=true}}if(!refused)throw Error('await authority');globalThis.awaitRefused=true;}});
                let rejected=0;for(const value of [null,true,1.5,2147483648,{{}}]){{try{{retained(value);}}catch(_){{rejected++;}}}}
                if(rejected!==5)throw Error('invalid input accepted');
                if(retained(41)!==41 || d.frame.returnValue!==41 || d.frame.originalReturnValue!==7)throw Error('immediate effect/snapshot');
                if(globalThis.revoke){{receipt.dispose();let refused=false;try{{retained(90)}}catch(_){{refused=true}}if(!refused)throw Error('revoked permit');return;}}
                while(d.cursor.invokeNext()!==null){{}}
                if(retained(43)!==41 || d.frame.originalReturnValue!==7)throw Error('outer lease did not resume');
                events.push('adapter');
              }}
            }});
            register('proof.other.v1','{}',{{post(d){{if('overrideReturn' in d.frame)throw Error('grant leaked');events.push('other');}}}},{{overrideReturn:true}});
            globalThis.subscribeProof=(id,semantic='proof.review.v1',phase='pre')=>subscribe(id,semantic,phase,(view)=>{{
                if('overrideReturn' in view || 'originalReturnValue' in view)throw Error('subscriber authority');
                let rejected=0;try{{retained(99);}}catch(_){{rejected++;}}
                try{{view.returnValue=99;}}catch(_){{rejected++;}}
                if(rejected!==2 || view.returnValue!==41)throw Error('lease/observer order');
                events.push('wrapper');
            }});
        }})()"#,
            proof::HASH,
            proof::HASH
        );
        let package = register_prepared_package_with_authorities(
            owner,
            source.into(),
            ImplementationManifestHash::new(proof::HASH.into()).unwrap(),
            vec![grant],
        )
        .unwrap();
        frame_tests::load_body("trusted-post", "return {};", "{}");
        let binding = proof::prepared_binding("trusted-post", |_| {});
        authorize(&package, "trusted-post", binding, "proof.review.v1").unwrap();
        eval_in_context(
            "trusted-post",
            &format!("globalThis.sub=subscribeProof({binding}n,'proof.review.v1','post');"),
        )
        .unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        let result = close_frame(&info);
        assert_eq!(result.output.bits, 41);
        assert_eq!(EFFECTS.with(Cell::get), 2);
        eval_in_context("trusted-post","if(events.join(',')!=='wrapper,adapter')throw Error(events);let refused=false;try{retained(99);}catch(_){refused=true;}if(!refused)throw Error('expired authority');").unwrap();
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            hs.perform_microtask_checkpoint();
        })
        .unwrap();
        eval_in_context("trusted-post","if(!awaitRefused)throw Error('missing awaited refusal');sub.dispose();events.length=0;").unwrap();
        authorize(&package, "trusted-post", binding, "proof.other.v1").unwrap();
        eval_in_context(
            "trusted-post",
            &format!("subscribeProof({binding}n,'proof.other.v1','post');"),
        )
        .unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        assert_eq!(close_frame(&info).output.bits, 7);
        eval_in_context(
            "trusted-post",
            "if(events.join(',')!=='other')throw Error('exact-contract grant leaked');",
        )
        .unwrap();
        let stale = with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = clone_plugin_context("trusted-post").unwrap();
            let context = v8::Local::new(&mut hs, &context);
            let scope = &mut v8::ContextScope::new(&mut hs, context);
            let global = context.global(scope);
            let value = get(scope, global, "retained").unwrap();
            v8::Global::new(scope, value)
        })
        .unwrap();
        unload_plugin("trusted-post");
        frame_tests::load_body("trusted-post", "return {};", "{}");
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = clone_plugin_context("trusted-post").unwrap();
            let context = v8::Local::new(&mut hs, &context);
            let scope = &mut v8::ContextScope::new(&mut hs, context);
            let global = context.global(scope);
            let value = v8::Local::new(scope, &stale);
            set(scope, global, "stale", value).unwrap();
        })
        .unwrap();
        let binding = proof::prepared_binding("trusted-post", |_| {});
        authorize(&package, "trusted-post", binding, "proof.review.v1").unwrap();
        eval_in_context(
            "trusted-post",
            &format!("subscribeProof({binding}n,'proof.review.v1','post');"),
        )
        .unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        assert_eq!(close_frame(&info).output.bits, 41);
        assert_eq!(EFFECTS.with(Cell::get), 4);
        eval_in_context("trusted-post", "globalThis.revoke=true;").unwrap();
        let info = open_frame();
        assert_eq!(crate::ffi::s2script_core_dispatch_function(1, &info, 0), 1);
        assert_eq!(close_frame(&info).output.bits, 41);
        assert_eq!(EFFECTS.with(Cell::get), 5);
        unload_plugin("trusted-post");
        drop(stale);
        drop(package);
        set_engine_ops(None);
        shutdown();
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
pub(super) mod entity_transport_tests {
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
        overridden: bool,
        writes: usize,
    }
    thread_local! {
        static SLOTS:RefCell<std::collections::BTreeMap<i32,u32>>=const{RefCell::new(std::collections::BTreeMap::new())};
        static STACK:RefCell<Vec<MockFrame>>=const{RefCell::new(Vec::new())};
        static EFFECTS:Cell<usize>=const{Cell::new(0)};
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
    extern "C" fn override_return(_:i64,token:u64,_:u64,_:*const i8,value:*const S2FunctionValue,out:*mut S2FunctionValue,why:*mut i8,cap:i32)->i32 {
        EFFECTS.with(|c|c.set(c.get()+1));
        let input=unsafe{*value};
        let Some(input)=project(input,input.flags) else{return refusal(why,cap)};
        STACK.with(|s|{
            let mut s=s.borrow_mut();let f=s.last_mut().unwrap();assert_eq!(f.id,token);
            if !f.overridden && f.action<2 {f.output=input;f.overridden=true;}
            let Some(result)=project(f.output,unsafe{(*out).flags}) else{return refusal(why,cap)};
            unsafe{*out=result};1
        })
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
            f.writes += 1;
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
                overridden: false,
                writes: 0,
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
    pub(crate) fn init_public_transport() {
        init(frame_tests::logger).unwrap();
        set_engine_ops(Some(S2EngineOps {
            function_prepare: Some(prepare), function_call: Some(call),
            function_hook_acquire: Some(acquire), function_hook_release: Some(release),
            function_target_release: Some(release), function_frame_read: Some(read),
            function_frame_write: Some(write), function_frame_commit: Some(commit),
            function_frame_override_return: Some(override_return), ..Default::default()
        }));
    }
    pub(crate) fn seed_public_entity(index: i32, serial: u32) -> u64 {
        assert_eq!(unsafe { slot(index, serial, 1) }, 1);
        crate::entity_live::on_created(index, serial as i32)
    }
    #[test]
    fn rejected_none_decision_does_not_write_unadoptable_entity_or_replay_prior_edit() {
        // Real V8 callback path with a transport model of the bridge's lossy
        // nullable entity read and write/changed bookkeeping. This is not a
        // native shim proof.
        init_public_transport();
        frame_tests::load_body("entity-rollback", "return {};", "{}");
        proof::install_generic_test_native("entity-rollback");
        let binding = proof::prepared_binding("entity-rollback", |f| {
            f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:void(ptr)".into();
            f["abi"]["parameters"] = serde_json::json!([{"name":"x","native":"ptr","projection":{"id":"entity?","version":1},"mutable":["pre"]}]);
            f["abi"]["returns"] = serde_json::json!({"native":"void","projection":{"id":"void","version":1}});
            f["policy"]["surfaces"] = serde_json::json!(["pre"]);
            f["policy"]["suppression"] = "none".into();
        });
        frame_tests::load_body("entity-strict-witness", "return {};", "{}");
        proof::install_generic_test_native("entity-strict-witness");
        let strict_binding = proof::prepared_binding("entity-strict-witness", |f| {
            f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:void(ptr)".into();
            f["abi"]["parameters"] = serde_json::json!([{"name":"x","native":"ptr","projection":{"id":"entity","version":1},"mutable":[]}]);
            f["abi"]["returns"] = serde_json::json!({"native":"void","projection":{"id":"void","version":1}});
            f["policy"]["surfaces"] = serde_json::json!(["pre"]);
            f["policy"]["suppression"] = "none".into();
        });
        eval_in_context("entity-strict-witness", &format!(
            "globalThis.denied=0;__proofSubscribeGeneric({strict_binding}n,'pre',true,v=>{{let failed=false;try{{void v.x;}}catch(_){{failed=true;}}if(!failed)throw Error('strict read accepted null');denied++;}});"
        )).unwrap();
        let raw = S2FunctionValue { kind: 8, flags: 2, reserved: 0, aux: 903, bits: 44 };
        let run = |id: u64| {
            STACK.with(|s| s.borrow_mut().push(MockFrame {
                id, input: raw, output: raw, receiver: None, edit: None,
                action: 0, overridden: false, writes: 0,
            }));
            let info = S2FunctionFrameInfo {
                version: 1, struct_size: 48, frame_token: id, native_epoch: id,
                invocation_id: id, suppressed_owner: 0, parameter_count: 1, flags: 0,
            };
            assert_eq!(crate::ffi::s2script_core_dispatch_function(11, &info, 0), 1);
            STACK.with(|s| s.borrow_mut().pop().unwrap())
        };
        eval_in_context("entity-rollback", &format!(
            "globalThis.bad=__proofSubscribeGeneric({binding}n,'pre',false,v=>{{if(v.x!==null)throw Error('unadoptable value');return 3;}});"
        )).unwrap();
        let untouched = run(71);
        assert_eq!(untouched.writes, 0, "rejected no-setter callback wrote native staging");
        assert!(untouched.edit.is_none());
        assert_eq!((untouched.input.aux, untouched.input.bits), (903, 44));
        eval_in_context("entity-rollback", &format!(
            "bad.dispose();globalThis.good=__proofSubscribeGeneric({binding}n,'pre',false,v=>{{v.x=null;return 1;}});globalThis.bad=__proofSubscribeGeneric({binding}n,'pre',false,v=>3);"
        )).unwrap();
        let prior = run(72);
        assert_eq!(prior.writes, 1, "rejected callback replayed an accepted native edit");
        assert_eq!(prior.action, 1);
        assert_eq!(prior.input.aux, u32::MAX);
        eval_in_context("entity-strict-witness", "if(denied!==2)throw Error('strict overlay read');").unwrap();
        unload_plugin("entity-rollback");
        unload_plugin("entity-strict-witness");
        set_engine_ops(None);
        shutdown();
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
        ops.function_frame_override_return = Some(override_return);
        set_engine_ops(Some(ops));
        EFFECTS.with(|c|c.set(0));
        for first in [false, true] {
            for reverse in [false, true] {
                proof::entity_conformance(first, reverse, slot);
            }
        }
        assert_eq!(EFFECTS.with(Cell::get),36,"books/typed rejection must precede native effect");
        set_engine_ops(None);
        shutdown();
    }
}

#[cfg(test)]
mod copied_transport_tests {
    use super::*;
    struct NativeFrame {
        token: u64,
        argument: Vec<u8>,
        original: Vec<u8>,
        result: Vec<u8>,
        staged: Option<Vec<u8>>,
        skipped: bool,
    }
    thread_local! {
        static FRAMES:RefCell<Vec<NativeFrame>>=const{RefCell::new(Vec::new())};
        static CALL_PRODUCERS:RefCell<Vec<S2FunctionCopyProducer>>=const{RefCell::new(Vec::new())};
        static COPY_CALLS:Cell<usize>=const{Cell::new(0)};
        static LAST_COMMIT:RefCell<Option<S2FunctionCopyProducer>>=const{RefCell::new(None)};
        static WRITERS:RefCell<Vec<S2FunctionCopyProducer>>=const{RefCell::new(Vec::new())};
    }
    unsafe fn bytes(v: *const S2FunctionValue, i: *const S2FunctionCopyInput) -> Vec<u8> {
        let v = &*v;
        let i = &*i;
        assert_eq!((i.version, i.struct_size), (1, 24));
        assert!(v.bits + v.aux as u64 <= i.size);
        if v.aux == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(i.data.add(v.bits as usize), v.aux as usize).to_vec()
        }
    }
    unsafe fn output(b: &[u8], v: *mut S2FunctionValue, o: *mut S2FunctionCopyOutput) {
        let o = &mut *o;
        assert_eq!((o.version, o.struct_size), (1, 32));
        assert!(o.capacity >= 65535);
        std::ptr::copy_nonoverlapping(b.as_ptr(), o.data, b.len());
        o.size = b.len() as u64;
        (*v).aux = b.len() as u32;
        (*v).bits = 0;
    }
    extern "C" fn call(
        id: i64,
        owner: u64,
        args: *const S2FunctionValue,
        argc: i32,
        out: *mut S2FunctionValue,
        input: *const S2FunctionCopyInput,
        output_span: *mut S2FunctionCopyOutput,
        producer: *const S2FunctionCopyProducer,
        reason: *mut i8,
        reason_capacity: i32,
    ) -> i32 {
        COPY_CALLS.with(|c| c.set(c.get() + 1));
        CALL_PRODUCERS.with(|s| s.borrow_mut().push(unsafe { *producer }));
        assert_eq!(argc, 1);
        assert_eq!(unsafe { (*producer).domain }, 1);
        assert_eq!(unsafe { (*producer).generation }, owner);
        let argument = unsafe { bytes(args, input) };
        let token = registry::next_id().unwrap();
        FRAMES.with(|s| {
            s.borrow_mut().push(NativeFrame {
                token,
                argument: argument.clone(),
                original: argument.clone(),
                result: argument,
                staged: None,
                skipped: false,
            })
        });
        let mut info = S2FunctionFrameInfo {
            version: 1,
            struct_size: 48,
            frame_token: token,
            native_epoch: token,
            invocation_id: token,
            suppressed_owner: owner,
            parameter_count: 1,
            flags: 0,
        };
        let pre = crate::ffi::s2script_core_dispatch_function(id, &info, 0);
        FRAMES.with(|s| {
            let mut s = s.borrow_mut();
            let f = s.last_mut().unwrap();
            if !f.skipped {
                f.result = f.argument.clone();
                f.original = f.argument.clone();
            }
            info.flags = u32::from(f.skipped);
        });
        let post = crate::ffi::s2script_core_dispatch_function(id, &info, 1);
        let f = FRAMES.with(|s| s.borrow_mut().pop()).unwrap();
        // The real RuntimeBinding runs matched POST cleanup, then reports a
        // recorded PRE/POST dispatch failure instead of returning copied output.
        if pre != 1 || post != 1 {
            let message = b"FunctionCopyInvocationFailure: synchronous core function dispatch failed";
            if !reason.is_null() && reason_capacity > 0 {
                let len = message.len().min(reason_capacity as usize - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(message.as_ptr(), reason.cast(), len);
                    *reason.add(len) = 0;
                }
            }
            return 0;
        }
        unsafe { output(&f.result, out, output_span) };
        1
    }
    extern "C" fn read(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        selector: i32,
        value: *mut S2FunctionValue,
        out: *mut S2FunctionCopyOutput,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        FRAMES.with(|s| {
            let s = s.borrow();
            let f = s.last().unwrap();
            assert_eq!(token, f.token);
            let bytes = match selector {
                -3 => &f.original,
                -2 => &f.result,
                0 => &f.argument,
                _ => panic!("selector"),
            };
            unsafe { output(bytes, value, out) }
        });
        1
    }
    extern "C" fn write(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        selector: i32,
        value: *const S2FunctionValue,
        input: *const S2FunctionCopyInput,
        producer: *const S2FunctionCopyProducer,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        assert_eq!(selector, 0);
        let value = unsafe { bytes(value, input) };
        FRAMES.with(|s| {
            let mut s = s.borrow_mut();
            let f = s.last_mut().unwrap();
            assert_eq!(f.token, token);
            f.staged = Some(value)
        });
        WRITERS.with(|s| s.borrow_mut().push(unsafe { *producer }));
        1
    }
    extern "C" fn commit(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        action: i32,
        value: *const S2FunctionValue,
        input: *const S2FunctionCopyInput,
        producer: *const S2FunctionCopyProducer,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        let result = (!value.is_null()).then(|| unsafe { bytes(value, input) });
        FRAMES.with(|s| {
            let mut s = s.borrow_mut();
            let f = s.last_mut().unwrap();
            assert_eq!(f.token, token);
            if let Some(v) = f.staged.take() {
                f.argument = v
            }
            if let Some(v) = result {
                f.result = v;
            }
            f.skipped = action >= 2;
        });
        LAST_COMMIT.with(|s| *s.borrow_mut() = Some(unsafe { *producer }));
        1
    }
    extern "C" fn override_return(
        _: i64,
        token: u64,
        _: u64,
        _: *const i8,
        value: *const S2FunctionValue,
        input: *const S2FunctionCopyInput,
        producer: *const S2FunctionCopyProducer,
        out: *mut S2FunctionValue,
        span: *mut S2FunctionCopyOutput,
        _: *mut i8,
        _: i32,
    ) -> i32 {
        assert_eq!(unsafe { (*producer).domain }, 2);
        let result = unsafe { bytes(value, input) };
        FRAMES.with(|s| {
            let mut s = s.borrow_mut();
            let f = s.last_mut().unwrap();
            assert_eq!(f.token, token);
            f.result = result;
            unsafe { output(&f.result, out, span) }
        });
        1
    }
    #[test]
    fn retained_copy_bookkeeping_survives_delivery_destruction_and_retirement() {
        scalar_transport_tests::init_transport();
        let mut ops = engine_ops().unwrap();
        ops.function_call_copy = Some(call);
        ops.function_frame_read_copy = Some(read);
        ops.function_frame_write_copy = Some(write);
        ops.function_frame_commit_copy = Some(commit);
        ops.function_frame_override_return_copy = Some(override_return);
        set_engine_ops(Some(ops));
        let state = proof::copy_begin();
        let target = SUBSCRIPTIONS.with(|rows| rows.borrow().values().next().unwrap().binding.target.unwrap());
        let mut charged_during_destruction = Vec::new();
        for retirement in [false, true] {
            let charged = Rc::new(Cell::new(false));
            charged_during_destruction.push(charged.clone());
            let guard = copied::Bookkeeping::reserve(4096, copied::Producer::engine()).unwrap();
            let alive = guard.alive_probe();
            let after = guard.alive_probe();
            let destroyed = Rc::new(Cell::new(0));
            let observed = destroyed.clone();
            let mut buffer = copied::Buffer::new(8, copied::Producer::engine()).unwrap();
            buffer.bytes_mut().copy_from_slice(b"retained");
            let mut value = buffer.own(4).unwrap();
            value.observe_destruction(Box::new(move || {
                charged.set(alive());
                observed.set(observed.get() + 1);
            }));
            INVOCATIONS.with(|rows| rows.borrow_mut().insert((target, 12345), InvocationState {
                copy_bookkeeping: Some(guard),
                adapters: [None, None],
                deliveries: vec![Decision { action: 2, value: Some(ProjectedValue::Copied(value)) }],
                retained_bytes: std::mem::size_of::<Decision>(),
            }));
            if retirement {
                let ids = SUBSCRIPTIONS.with(|rows| rows.borrow().keys().copied().collect::<Vec<_>>());
                for id in ids { drop_subscription(id); }
            } else {
                // Matched POST takes the retained row; dropping it destroys its deliveries.
                drop(INVOCATIONS.with(|rows| rows.borrow_mut().remove(&(target, 12345))));
            }
            assert_eq!(destroyed.get(), 1);
            assert!(!after(), "dispatch charge leaked after retained storage destruction");
        }
        proof::copy_abort(state);
        set_engine_ops(None);
        shutdown();
        assert_eq!(charged_during_destruction.iter().map(|v| v.get()).collect::<Vec<_>>(), [true, true], "dispatch charge released during delivery element destruction (POST, retirement)");
    }

    #[test]
    fn real_v8_copied_callbacks_use_sidecars_and_exact_delivery_lineage_mock_transport() {
        scalar_transport_tests::init_transport();
        let mut ops = engine_ops().unwrap();
        ops.function_call_copy = Some(call);
        ops.function_frame_read_copy = Some(read);
        ops.function_frame_write_copy = Some(write);
        ops.function_frame_commit_copy = Some(commit);
        ops.function_frame_override_return_copy = Some(override_return);
        set_engine_ops(Some(ops));
        let mut state = proof::copy_begin();
        proof::copy_probe(&mut state);
        assert!(state.ready);
        let caller = OwnerKey::plugin("copy-caller", plugin_generation("copy-caller"));
        let binding = registry::binding(registry::owner_bindings(&caller)[0], &caller).unwrap();
        for missing in 0..5 {
            let mut incomplete = ops;
            let name = match missing {
                0 => {
                    incomplete.function_call_copy = None;
                    "function_call_copy"
                }
                1 => {
                    incomplete.function_frame_read_copy = None;
                    "function_frame_read_copy"
                }
                2 => {
                    incomplete.function_frame_write_copy = None;
                    "function_frame_write_copy"
                }
                3 => {
                    incomplete.function_frame_commit_copy = None;
                    "function_frame_commit_copy"
                }
                _ => {
                    incomplete.function_frame_override_return_copy = None;
                    "function_frame_override_return_copy"
                }
            };
            set_engine_ops(Some(incomplete));
            assert!(runtime::prepare(&binding.function)
                .unwrap_err()
                .contains(name));
        }
        set_engine_ops(Some(ops));
        let calls = COPY_CALLS.with(Cell::get);
        let exhausted =
            copied::Buffer::new(8 * 1024 * 1024 - 256, copied::Producer::engine()).unwrap();
        eval_in_context("copy-caller","{let denied=false;try{__proofEntityCall(binding,'input')}catch(e){denied=String(e).includes('FunctionCopyBudgetExceeded')}if(!denied)throw Error('output admission');}").unwrap();
        assert_eq!(COPY_CALLS.with(Cell::get), calls);
        drop(exhausted);

        for id in ["copy-a", "copy-b"] {
            eval_in_context(id,"globalThis.poisonTouches=0;Object.defineProperty(Object.prototype,'returnValue',{configurable:true,get(){poisonTouches++;return 'spoofed'},set(_){poisonTouches++;}});").unwrap();
        }
        proof::copy_mode("carry");
        eval_in_context(
            "copy-caller",
            "if(__proofEntityCall(binding,'input')!=='same-copied-result')throw Error('carry');",
        )
        .unwrap();
        for id in ["copy-a", "copy-b"] {
            eval_in_context(id,"if(poisonTouches)throw Error('delivery inherited accessor ran');if(globalThis.oldDelivery){const d=Object.getOwnPropertyDescriptor(oldDelivery,'returnValue');if(!d||!('value' in d)||d.value!=='same-copied-result'||!Object.isFrozen(oldDelivery))throw Error('delivery visible bytes');}delete Object.prototype.returnValue;").unwrap();
        }
        let a = copied::Producer::owner(&OwnerKey::plugin("copy-a", plugin_generation("copy-a")))
            .wire();
        let b = copied::Producer::owner(&OwnerKey::plugin("copy-b", plugin_generation("copy-b")))
            .wire();
        let carry = LAST_COMMIT.with(|s| s.borrow().unwrap());
        assert_eq!(
            (carry.domain, carry.digest, carry.generation),
            (a.domain, a.digest, a.generation)
        );
        let writer = WRITERS.with(|s| *s.borrow().last().unwrap());
        assert_eq!(
            (writer.domain, writer.digest, writer.generation),
            (b.domain, b.digest, b.generation)
        );
        proof::copy_mode("manufacture");
        eval_in_context("copy-caller", "__proofEntityCall(binding,'input');").unwrap();
        assert_eq!(LAST_COMMIT.with(|s| s.borrow().unwrap().domain), 2);
        with_host_isolate(|isolate| {
            let mut storage=v8::HandleScope::new(isolate);let mut hs=unsafe{std::pin::Pin::new_unchecked(&mut storage)}.init();
            let a=clone_plugin_context("copy-a").unwrap();let a=v8::Local::new(&mut hs,&a);
            let delivery={let scope=&mut v8::ContextScope::new(&mut hs,a);let global=a.global(scope);let object=get(scope,global,"oldDelivery").unwrap();v8::Global::new(scope,object)};
            let b=clone_plugin_context("copy-b").unwrap();let b=v8::Local::new(&mut hs,&b);let scope=&mut v8::ContextScope::new(&mut hs,b);
            let delivery=v8::Local::new(scope,&delivery);let global=b.global(scope);set(scope,global,"oldDelivery",delivery).unwrap();
        }).unwrap();
        proof::copy_mode("stale");
        let writes = WRITERS.with(|rows| rows.borrow().len());
        proof::copy_expect_delivery_refusal("copy-a", "foreign-input");
        assert_eq!(WRITERS.with(|rows| rows.borrow().len()), writes);
        assert!(FRAMES.with(|frames| frames.borrow().is_empty()));
        proof::copy_exercise();
        // Each failing callback drops its private staged writes; neither parent
        // can borrow the other's generation quota on the shared target.
        proof::copy_mode("carry");
        let a_full=copied::Buffer::new(8*1024*1024-256,copied::Producer::owner(&OwnerKey::plugin("copy-a",plugin_generation("copy-a")))).unwrap();
        let b_full=copied::Buffer::new(8*1024*1024-256,copied::Producer::owner(&OwnerKey::plugin("copy-b",plugin_generation("copy-b")))).unwrap();
        let writes=WRITERS.with(|s|s.borrow().len());
        eval_in_context("copy-caller","if(__proofEntityCall(binding,'input')!=='input')throw Error('quota rollback');").unwrap();
        assert_eq!(WRITERS.with(|s|s.borrow().len()),writes);drop(a_full);drop(b_full);
        // Copied JS values survive a microtask boundary while saved accessors do not.
        eval_in_context("copy-a","globalThis.awaitCopy=false;(async()=>{const value=savedValue;await 0;let denied=false;try{savedView.text}catch(_){denied=true}if(value!=='input'||!denied)throw Error('await copy lifetime');awaitCopy=true;})();").unwrap();
        with_host_isolate(|isolate|{let mut storage=v8::HandleScope::new(isolate);let mut scope=unsafe{std::pin::Pin::new_unchecked(&mut storage)}.init();scope.perform_microtask_checkpoint();}).unwrap();
        eval_in_context("copy-a","if(!awaitCopy)throw Error('microtask did not run');").unwrap();
        proof::copy_finish(state);
        let state = proof::copy_process_begin();
        assert!(proof::copy_process_probe());
        proof::copy_process_exercise();
        for (offset, id) in [(2, "copy-process-a"), (1, "copy-process-b")] {
            let expected =
                copied::Producer::owner(&OwnerKey::plugin(id, plugin_generation(id))).wire();
            let actual = CALL_PRODUCERS.with(|s| {
                let s = s.borrow();
                s[s.len() - offset]
            });
            assert_eq!(
                (actual.domain, actual.digest, actual.generation),
                (expected.domain, expected.digest, expected.generation)
            );
        }
        let expected = copied::Producer::owner(&OwnerKey::plugin(
            "copy-process-a",
            plugin_generation("copy-process-a"),
        ))
        .wire();
        let actual = WRITERS.with(|s| *s.borrow().last().unwrap());
        assert_eq!(
            (actual.domain, actual.digest, actual.generation),
            (expected.domain, expected.digest, expected.generation)
        );
        proof::copy_process_finish(state);
        set_engine_ops(None);
        shutdown();
    }
}

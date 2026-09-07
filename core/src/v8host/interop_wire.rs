//! Strict protocol 2 crossings. Snapshot owned metadata before entering JavaScript.
use super::*;
thread_local! { static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
pub(super) fn at_interop_boundary() -> bool { DEPTH.with(|d| d.get() == 0) }
pub(super) struct InteropGuard;
impl InteropGuard {
    pub(super) fn enter(scope: &mut v8::PinScope) -> Option<Self> {
        if DEPTH.with(|d| d.get() >= 32) {
            throw_named(scope, "InterfaceRecursionLimit", "maximum 32 active calls");
            return None;
        }
        DEPTH.with(|d| d.set(d.get() + 1));
        Some(Self)
    }
}
impl Drop for InteropGuard {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get() - 1));
    }
}
pub(super) fn live_interop_context(scope: &mut v8::PinScope, id: &str) -> bool {
    scope
        .get_current_context()
        .get_slot::<InteropGeneration>()
        .map_or(false, |g| REGISTRY.with(|r| r.borrow().is_live(id, g.0)))
}
// Store the original SDK prototype in a V8-private property on its own context global.
// JS cannot replace this identity, and it adds no Rust Global root / teardown registry.
pub(super) fn capture_entity_ref_prototype(scope: &mut v8::PinScope) {
    let global = scope.get_current_context().global(scope);
    let package = data_property(scope, global, "__s2pkg_entity").unwrap();
    let package = v8::Local::<v8::Object>::try_from(package).unwrap();
    let constructor = data_property(scope, package, "EntityRef").unwrap();
    let constructor = v8::Local::<v8::Object>::try_from(constructor).unwrap();
    let prototype = data_property(scope, constructor, "prototype").unwrap();
    let name = v8::String::new(scope, "s2script.interop.EntityRef.prototype").unwrap();
    let key = v8::Private::for_api(scope, Some(name));
    assert_eq!(global.set_private(scope, key, prototype), Some(true));
}
fn is_entity_ref(scope: &mut v8::PinScope, prototype: v8::Local<v8::Value>) -> bool {
    let global = scope.get_current_context().global(scope);
    let name = v8::String::new(scope, "s2script.interop.EntityRef.prototype").unwrap();
    let key = v8::Private::for_api(scope, Some(name));
    global
        .get_private(scope, key)
        .is_some_and(|original| original.is_object() && original.strict_equals(prototype))
}
pub(super) fn published_contract(name: &str) -> Option<crate::interop::Contract> {
    let (owner, _) = IFACES.with(|r| r.borrow().producer_of(name))?;
    PLUGIN_PUBLISHES.with(|p| p.borrow().get(&owner)?.get(name)?.contract.clone())
}
pub(super) fn checked_contract(
    consumer: &str,
    name: &str,
) -> Result<Option<crate::interop::Contract>, &'static str> {
    let actual = published_contract(name);
    let expected =
        PLUGIN_INTEROP.with(|p| p.borrow().get(consumer).and_then(|m| m.get(name)).cloned());
    if actual.is_some() || expected.is_some() {
        let live = IFACES
            .with(|r| r.borrow().producer_of(name))
            .map_or(false, |(owner, generation)| {
                REGISTRY.with(|r| r.borrow().is_live(&owner, generation))
            });
        if !live {
            return Err("InterfaceUnavailable");
        }
        let (Some(actual), Some(expected)) = (actual, expected) else {
            return Err("InterfaceTypesMismatch");
        };
        if actual.sha256 != expected.sha256
            || !IFACES.with(|r| r.borrow().verified_import(consumer, name))
        {
            return Err("InterfaceTypesMismatch");
        }
        return Ok(Some(actual));
    }
    Ok(None)
}
fn data_property<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<v8::Object>,
    key: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let key = v8::String::new(scope, key)?;
    let descriptor = object.get_own_property_descriptor(scope, key.into())?;
    let descriptor = v8::Local::<v8::Object>::try_from(descriptor).ok()?;
    let value_key = v8::String::new(scope, "value")?;
    if !descriptor.has_own_property(scope, value_key.into())? {
        return None;
    }
    descriptor.get(scope, value_key.into())
}
/// No JSON.stringify, toJSON, getters, proxy traps, silent omissions, or numeric coercion.
fn copy_value(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
    depth: usize,
) -> Option<serde_json::Value> {
    use serde_json::Value;
    if depth > 64 {
        return None;
    }
    if value.is_null() {
        return Some(Value::Null);
    }
    if value.is_boolean() {
        return Some(Value::Bool(value.boolean_value(scope)));
    }
    if value.is_number() {
        let n = value.number_value(scope)?;
        return serde_json::Number::from_f64(n).map(Value::Number);
    }
    if value.is_string() {
        return Some(Value::String(value.to_rust_string_lossy(scope)));
    }
    if !value.is_object() || value.is_function() || value.is_proxy() {
        return None;
    }
    let obj = v8::Local::<v8::Object>::try_from(value).ok()?;
    let proto = obj.get_prototype(scope)?;
    if is_entity_ref(scope, proto) {
        // The shipped SDK ref has only index/id own data properties. Include non-enumerable
        // and symbol keys: silently dropping extra values would weaken the strict boundary.
        let keys = obj.get_own_property_names(
            scope,
            v8::GetPropertyNamesArgs {
                property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                key_conversion: v8::KeyConversionMode::ConvertToString,
                ..Default::default()
            },
        )?;
        if keys.length() != 2 {
            return None;
        }
        for i in 0..keys.length() {
            let key = keys.get_index(scope, i)?;
            if !key.is_string()
                || !matches!(key.to_rust_string_lossy(scope).as_str(), "index" | "id")
            {
                return None;
            }
        }
        let index = data_property(scope, obj, "index")?;
        let id = data_property(scope, obj, "id")?;
        if !index.is_number() || !id.is_number() {
            return None;
        }
        let index = index.number_value(scope)?;
        let id = id.number_value(scope)?;
        if index < 0.0
            || index > i32::MAX as f64
            || id < 0.0
            || id > 9_007_199_254_740_991.0
            || index.fract() != 0.0
            || id.fract() != 0.0
            || !index.is_finite()
            || !id.is_finite()
        {
            return None;
        }
        return Some(serde_json::json!({"__s2ref":[index as u64,id as u64]}));
    }
    if !value.is_array() && !proto.is_null() {
        let plain = v8::Object::new(scope);
        if !proto.strict_equals(plain.get_prototype(scope)?) {
            return None;
        }
    }
    let keys = obj.get_own_property_names(
        scope,
        v8::GetPropertyNamesArgs {
            property_filter: v8::PropertyFilter::ALL_PROPERTIES,
            key_conversion: v8::KeyConversionMode::ConvertToString,
            ..Default::default()
        },
    )?;
    let mut fields = serde_json::Map::new();
    for i in 0..keys.length() {
        let key = keys.get_index(scope, i)?;
        if !key.is_string() {
            return None;
        }
        let key = key.to_rust_string_lossy(scope);
        if value.is_array() && key == "length" {
            continue;
        }
        let item = data_property(scope, obj, &key)?;
        fields.insert(key, copy_value(scope, item, depth + 1)?);
    }
    if value.is_array() {
        let arr = v8::Local::<v8::Array>::try_from(value).ok()?;
        if fields.len() != arr.length() as usize {
            return None;
        }
        let mut values = Vec::with_capacity(fields.len());
        for i in 0..arr.length() {
            values.push(fields.remove(&i.to_string())?);
        }
        Some(Value::Array(values))
    } else {
        Some(Value::Object(fields))
    }
}
pub(super) fn strict_json(
    scope: &mut v8::PinScope,
    value: v8::Local<v8::Value>,
) -> Option<(String, serde_json::Value)> {
    let value = copy_value(scope, value, 0)?;
    Some((serde_json::to_string(&value).ok()?, value))
}
pub(super) fn observe_thenable(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> bool {
    if let Ok(promise) = v8::Local::<v8::Promise>::try_from(value) {
        promise.mark_as_handled();
        return true;
    }
    let Ok(obj) = v8::Local::<v8::Object>::try_from(value) else {
        return false;
    };
    let Some(key) = v8::String::new(scope, "then") else {
        return false;
    };
    let Some(then) = obj.get(scope, key.into()) else {
        return true;
    };
    let Ok(then) = v8::Local::<v8::Function>::try_from(then) else {
        return false;
    };
    if let Some(ignore) = v8::Function::new(
        scope,
        |_: &mut v8::PinScope, _: v8::FunctionCallbackArguments, _: v8::ReturnValue| {},
    ) {
        let _ = then.call(scope, value, &[ignore.into(), ignore.into()]);
    }
    true
}

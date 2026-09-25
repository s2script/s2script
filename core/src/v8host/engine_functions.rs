//! Public facades carry exact context identities exclusively in native callback data.
use super::*;
use crate::engine_functions::{contract::OwnerKey, registry, runtime};
use function_adapter::{current_owner, projected_from_js, projected_to_js, throw};

fn set(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    key: &str,
    value: v8::Local<v8::Value>,
) {
    let key = v8::String::new(scope, key).unwrap();
    object.set(scope, key.into(), value);
}
fn callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    owner: &OwnerKey,
    binding: u64,
    subscription: u64,
    op: i32,
    package_token: Option<v8::Local<v8::Value>>,
) -> v8::Local<'s, v8::Function> {
    let data = v8::Array::new(scope, 6);
    let id = v8::String::new(scope, &owner.id).unwrap();
    data.set_index(scope, 0, id.into());
    for (i, value) in [owner.generation, binding, subscription]
        .into_iter()
        .enumerate()
    {
        let value = v8::BigInt::new_from_u64(scope, value);
        data.set_index(scope, (i + 1) as u32, value.into());
    }
    if let Some(token) = package_token {
        data.set_index(scope, 5, token);
    }
    let op = v8::Integer::new(scope, op);
    data.set_index(scope, 4, op.into());
    v8::Function::builder(invoke)
        .data(data.into())
        .build(scope)
        .unwrap()
}
fn getter(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    key: &str,
    function: v8::Local<v8::Function>,
) {
    let key = v8::String::new(scope, key).unwrap();
    let undef = v8::undefined(scope);
    let descriptor = v8::PropertyDescriptor::new_from_get_set(function.into(), undef.into());
    object.define_property(scope, key.into(), &descriptor);
}
fn frozen_json<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &serde_json::Value,
) -> v8::Local<'s, v8::Value> {
    match value {
        serde_json::Value::Object(fields) => {
            let obj = v8::Object::new(scope);
            for (key, value) in fields {
                let value = frozen_json(scope, value);
                set(scope, obj, key, value);
            }
            obj.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
            obj.into()
        }
        serde_json::Value::Array(values) => {
            let obj = v8::Array::new(scope, values.len() as i32);
            for (i, value) in values.iter().enumerate() {
                let value = frozen_json(scope, value);
                obj.set_index(scope, i as u32, value);
            }
            obj.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
            obj.into()
        }
        _ => {
            let text = v8::String::new(scope, &value.to_string()).unwrap();
            v8::json::parse(scope, text).unwrap()
        }
    }
}
pub(super) fn lookup(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let result = (|| {
        let owner = current_owner(scope)?;
        if !args.get(0).is_string() {
            return Err("engine function name must be a string".to_string());
        }
        let binding = registry::named_binding(&owner, &args.get(0).to_rust_string_lossy(scope))?;
        Ok(facade(scope, &owner, &binding, None))
    })();
    match result {
        Ok(v) => rv.set(v.into()),
        Err(e) => throw(scope, e),
    }
}
pub(super) fn facade<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    owner: &OwnerKey,
    binding: &registry::Binding,
    package_token: Option<v8::Local<v8::Value>>,
) -> v8::Local<'s, v8::Object> {
    let object = v8::Object::new(scope);
    for (name, op) in [("available", 0), ("status", 1)] {
        let function = callback(scope, owner, binding.id, 0, op, package_token);
        getter(scope, object, name, function);
    }
    if binding.target.is_some() {
        for (surface, name, op) in [
            ("call", "call", 2),
            ("pre", "onPre", 3),
            ("post", "onPost", 4),
        ] {
            if binding
                .function
                .policy
                .surfaces
                .iter()
                .any(|s| s == surface)
            {
                let function = callback(scope, owner, binding.id, 0, op, package_token);
                set(scope, object, name, function.into());
            }
        }
    }
    object.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
    object
}
fn invoke(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let result = (|| {
        let data =
            v8::Local::<v8::Array>::try_from(args.data()).map_err(|_| "invalid function facade")?;
        let id = data
            .get_index(scope, 0)
            .unwrap()
            .to_rust_string_lossy(scope);
        let number = |scope: &mut v8::PinScope, i| -> u64 {
            v8::Local::<v8::BigInt>::try_from(data.get_index(scope, i).unwrap())
                .unwrap()
                .u64_value()
                .0
        };
        let owner = OwnerKey::plugin(&id, number(scope, 1));
        if current_owner(scope)? != owner {
            return Err("function facade owner mismatch".into());
        }
        let token = data.get_index(scope, 5).filter(|v| !v.is_undefined());
        let instance = token
            .map(|token| function_adapter::instance(scope, token).map(|(i, _)| i))
            .transpose()?;
        let binding_owner = instance.as_ref().map_or(&owner, |i| &i.package_owner);
        let binding = registry::binding(number(scope, 2), binding_owner)?;
        let sub = number(scope, 3);
        let op = data
            .get_index(scope, 4)
            .unwrap()
            .int32_value(scope)
            .unwrap();
        match op {
            0 => Ok(v8::Boolean::new(scope, binding.target.is_some()).into()),
            1 => {
                let p = &binding.provenance;
                Ok(frozen_json(
                    scope,
                    &serde_json::json!({
                        "canonicalId": binding.function.canonical_id,
                        "availability": if binding.target.is_some() {"available"} else {"unavailable"},
                        "reason": binding.unavailable,
                        "hookObservation": function_adapter::binding_observation(binding.id),
                        "provenance": {"archiveHash":p.archive_hash,"baseContractHash":p.base_contract_hash,"instances":p.instances,
                            "appliedOverrides":p.overrides.iter().map(|o| serde_json::json!({"path":o.relative_path,"sha256":o.sha256})).collect::<Vec<_>>(),
                            "finalTargetHash":p.final_target_hash,"required":p.required,
                            "resolverReceipt":if binding.target.is_some() {"resolved"} else {"unavailable"}}
                    }),
                ))
            }
            2 => {
                if binding.function.abi.borrowed() || !binding.function.policy.surfaces.iter().any(|s|s=="call") {return Err("function call surface unavailable".into());}
                let _copy_scope=crate::engine_functions::copied::Scope::enter()?;
                let abi = &binding.function.abi;
                let receiver = usize::from(abi.member_receiver);
                if args.length() as usize != receiver + abi.parameters.len() {
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
                    values.push(projected_from_js(
                        scope,
                        args.get(i as i32),
                        native,
                        projection,
                    )?);
                }
                let value = crate::nest::with_outbound(&args, || {
                    if let Some(instance) = &instance {
                        runtime::call_package_binding(&binding, instance, &values)
                    } else {
                        runtime::call_binding(&binding, &owner, &values)
                    }
                })?;
                projected_to_js(scope, value)
            }
            3 | 4 => {
                let mut observe = op == 4;
                let handler = if op == 3 && args.length() == 2 {
                    let options = v8::Local::<v8::Object>::try_from(args.get(0))
                        .map_err(|_| "onPre options required")?;
                    let key = v8::String::new(scope, "observeOnly").unwrap();
                    let value = options
                        .get(scope, key.into())
                        .ok_or("observeOnly required")?;
                    if !value.is_true() {
                        return Err("observeOnly must be true".into());
                    }
                    observe = true;
                    args.get(1)
                } else if args.length() == 1 {
                    args.get(0)
                } else {
                    return Err("subscription handler required".into());
                };
                let handler = v8::Local::<v8::Function>::try_from(handler)
                    .map_err(|_| "subscription handler must be a function")?;
                let handler = v8::Global::new(scope, handler);
                let sub = if let Some(token) = token {
                    function_adapter::subscribe_package_generic(
                        scope,
                        token,
                        binding.id,
                        op - 3,
                        observe,
                        handler,
                    )?
                } else {
                    function_adapter::subscribe_generic(
                        scope,
                        owner.clone(),
                        binding.id,
                        op - 3,
                        observe,
                        handler,
                    )?
                };
                let object = v8::Object::new(scope);
                for (name, op) in [("status", 5), ("reason", 6)] {
                    let function = callback(scope, &owner, binding.id, sub, op, token);
                    getter(scope, object, name, function);
                }
                let function = callback(scope, &owner, binding.id, sub, 7, token);
                set(scope, object, "dispose", function.into());
                object.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
                Ok(object.into())
            }
            5 | 6 => {
                let (status, reason) = function_adapter::subscription_state(sub, &owner)?;
                Ok(frozen_json(
                    scope,
                    &if op == 5 {
                        serde_json::json!(status)
                    } else {
                        serde_json::json!(reason)
                    },
                ))
            }
            7 => {
                let removed = release_resource(
                    &owner.id,
                    owner.generation,
                    &plugin::Resource::FunctionSubscription(sub),
                );
                if removed {
                    function_adapter::drop_subscription(sub);
                }
                Ok(v8::Boolean::new(scope, removed).into())
            }
            _ => Err("invalid facade operation".into()),
        }
    })();
    match result {
        Ok(v) => rv.set(v),
        Err(e) => throw(scope, e),
    }
}

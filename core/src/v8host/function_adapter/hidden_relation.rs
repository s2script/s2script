//! `frame.hiddenReferencedBy(position, entityRef, className, fieldName) -> boolean`
//!
//! Present only on leased views of trusted bindings that declare a hidden
//! (`native-only`) position. Answers "does this live entity's pointer field hold
//! exactly the hidden native argument?" without the hidden pointer, the entity
//! address or the field value ever reaching JS. Engine-generic: the class and
//! field are opaque schema names resolved against the live schema.
//!
//! Error policy: authority/usage failures throw (expired or foreign lease, no
//! such hidden position, malformed arguments, native frame/capability mismatch).
//! Relationship-subject misses return `false` (stale entity, field absent from
//! the live schema, unreadable or unequal word).
use super::*;

pub(super) fn has_hidden(binding: &Binding) -> bool {
    let abi = &binding.function.abi;
    binding.function.trusted() && abi.receiver.iter().chain(&abi.parameters).any(|p| p.hidden())
}
/// Hidden names are not JS-visible fields, so they are resolved here and must be unique.
pub(super) fn hidden_selector(binding: &Binding, name: &str) -> Result<i32, String> {
    let abi = &binding.function.abi;
    let mut found = abi.receiver.iter().map(|p| (-1, p))
        .chain(abi.parameters.iter().enumerate().map(|(i, p)| (i as i32, p)))
        .filter(|(_, p)| p.hidden() && p.name == name)
        .map(|(i, _)| i);
    match (found.next(), found.next()) {
        (Some(i), None) => Ok(i),
        (None, _) => Err(format!("unknown hidden position: {name}")),
        _ => Err(format!("ambiguous hidden position: {name}")),
    }
}
fn bounded_name(scope: &mut v8::PinScope, value: v8::Local<v8::Value>, what: &str) -> Result<String, String> {
    let text = v8::Local::<v8::String>::try_from(value).map_err(|_| format!("{what} must be a string"))?;
    let text = text.to_rust_string_lossy(scope);
    if text.is_empty() || text.len() > 256 || text.contains('\0') {
        return Err(format!("invalid {what}"));
    }
    Ok(text)
}
pub(super) fn js_hidden_referenced_by(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let result = (|| {
        let l = lease(scope, bigint(args.data())?)?;
        if !has_hidden(&l.binding) {
            return Err("binding has no hidden native position".into());
        }
        let name = bounded_name(scope, args.get(0), "hidden position name")?;
        let selector = hidden_selector(&l.binding, &name)?;
        let reference = interop_wire::strict_entity_reference(scope, args.get(1))
            .ok_or("genuine current-context EntityRef required")?;
        let class = bounded_name(scope, args.get(2), "schema class name")?;
        let field = bounded_name(scope, args.get(3), "schema field name")?;
        if crate::entity_live::engine_serial_for(reference.index, reference.id).is_none() {
            return Ok(false);
        }
        let offset = schema_offset_cached(&class, &field);
        if offset < 0 {
            return Ok(false);
        }
        let entity = projection::encode(ProjectedValue::Entity { reference: Some(reference), nullable: false })?;
        l.dispatch.frame.hidden_referenced_by(&l.binding, selector, &entity, offset as u32)
            .map_err(|e| format!("{}: {e}", l.binding.function.canonical_id))
    })();
    match result {
        Ok(related) => rv.set_bool(related),
        Err(e) => throw(scope, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_functions::instance::*;
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Call { selector: i32, index: u32, serial: u64, offset: u32 }
    thread_local! {
        static TOKEN: Cell<u64> = const { Cell::new(0) };
        static ANSWER: Cell<i32> = const { Cell::new(1) };
        static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
    }
    const SOURCE: &str = r#"
      globalThis.probe=__s2_package_function('probe');
      globalThis.check=null;globalThis.saved=null;
      probe.onPre(v=>{if(check)check(v,'pre');});
      probe.onPost(v=>{if(check)check(v,'post');});
    "#;
    const CLASS: &str = "HiddenRelationOwnerProbe";
    extern "C" fn schema(class: *const i8, field: *const i8) -> i32 {
        let (class, field) = unsafe { (std::ffi::CStr::from_ptr(class), std::ffi::CStr::from_ptr(field)) };
        if class.to_bytes() == CLASS.as_bytes() && field.to_bytes() == b"m_pServices" { 0x40 } else { -1 }
    }
    extern "C" fn prepare(binding: u64, _: *const S2FunctionInstanceOwner, _: *const i8, _: *const i8, _: *const i8, out: *mut S2FunctionInstancePrepared, _: *mut i8, _: i32) -> i32 {
        unsafe { *out = S2FunctionInstancePrepared { version: 1, struct_size: 24, target: 1, capability: binding } }; 1
    }
    extern "C" fn activate(_: u64, _: *const S2FunctionInstanceOwner, _: *mut i8, _: i32) -> i32 { 1 }
    extern "C" fn release(_: u64) -> i32 { 1 }
    extern "C" fn presence(_: *const S2FunctionInstanceAccess, _: i32, _: *mut S2FunctionValue, _: *mut i8, _: i32) -> i32 { 0 }
    extern "C" fn field_read(_: *const S2FunctionInstanceAccess, _: i32, _: u32, _: *mut S2FunctionValue, _: *mut i8, _: i32) -> i32 { 0 }
    extern "C" fn field_write(_: *const S2FunctionInstanceAccess, _: i32, _: u32, _: *const S2FunctionValue, _: *mut i8, _: i32) -> i32 { 0 }
    extern "C" fn call_copy(_: i64, _: u64, _: *const S2FunctionValue, _: i32, _: *mut S2FunctionValue, _: *const S2FunctionCopyInput, _: *mut S2FunctionCopyOutput, _: *const S2FunctionCopyProducer, _: *mut i8, _: i32) -> i32 { 0 }
    extern "C" fn write_copy(_: i64, _: u64, _: u64, _: *const i8, _: i32, _: *const S2FunctionValue, _: *const S2FunctionCopyInput, _: *const S2FunctionCopyProducer, _: *mut i8, _: i32) -> i32 { 0 }
    extern "C" fn commit_copy(_: i64, token: u64, _: u64, _: *const i8, _: i32, _: *const S2FunctionValue, _: *const S2FunctionCopyInput, _: *const S2FunctionCopyProducer, _: *mut i8, _: i32) -> i32 {
        (token == TOKEN.with(Cell::get)) as i32
    }
    extern "C" fn override_copy(_: i64, _: u64, _: u64, _: *const i8, _: *const S2FunctionValue, _: *const S2FunctionCopyInput, _: *const S2FunctionCopyProducer, _: *mut S2FunctionValue, _: *mut S2FunctionCopyOutput, _: *mut i8, _: i32) -> i32 { 0 }
    /// Native-side string-indirect capture already happened; the host serves the snapshot.
    extern "C" fn read_copy(_: i64, token: u64, _: u64, _: *const i8, selector: i32, value: *mut S2FunctionValue, output: *mut S2FunctionCopyOutput, _: *mut i8, _: i32) -> i32 {
        if token != TOKEN.with(Cell::get) || selector != 1 { return 0; }
        let text = b"label-text";
        unsafe {
            if (*value).flags != 4 || (*output).capacity < text.len() as u64 { return 0; }
            std::ptr::copy_nonoverlapping(text.as_ptr(), (*output).data, text.len());
            (*output).size = text.len() as u64; (*value).aux = text.len() as u32;
        }
        1
    }
    extern "C" fn related(access: *const S2FunctionInstanceAccess, selector: i32, entity: *const S2FunctionValue, offset: u32, out: *mut i32, _: *mut i8, _: i32) -> i32 {
        let (access, entity) = unsafe { (*access, *entity) };
        if access.frame_token != TOKEN.with(Cell::get) || entity.kind != 8 || entity.flags != 1 { return 0; }
        CALLS.with(|c| c.borrow_mut().push(Call { selector, index: entity.aux, serial: entity.bits, offset }));
        unsafe { *out = ANSWER.with(Cell::get) };
        1
    }
    fn input() -> TrustedFunctionInput {
        let base = crate::engine_functions::tests::fixture();
        let public: crate::engine_functions::contract::NormalizedFunction = serde_json::from_value(base["functions"][0].clone()).unwrap();
        let mut f = Function::from_public(public).unwrap();
        if let NormalizedTarget::Signature { pattern, target_validate, .. } = &mut f.target {
            *pattern = "53".into(); *target_validate = Validator::default(); target_validate.prologue = Some("??".into());
        }
        let position = |name: &str, projection: &str, ownership: &str| Position {
            name: name.into(), native: "ptr".into(), projection: Projection { id: projection.into(), version: 1 },
            ownership: Some(ownership.into()), mutable: vec![], instance: None, nullable: false };
        f.abi.member_receiver = false; f.abi.receiver = None;
        f.abi.parameters = vec![position("services", "native-only", "invocation-passthrough"),
            position("label", "string-indirect", "native-observed")];
        f.abi.returns = Position { name: String::new(), native: "i32".into(), projection: Projection { id: "i32".into(), version: 1 },
            ownership: None, mutable: vec![], instance: None, nullable: false };
        f.abi.instances = vec![];
        f.abi.fingerprint = f.abi.physical().fingerprint().unwrap(); f.abi.stack_copy_bytes = f.abi.physical().stack_bytes().unwrap();
        f.policy.surfaces = vec!["pre".into(), "post".into()]; f.policy.suppression = "none".into();
        let mut p = serde_json::to_value(&f.policy).unwrap(); p.as_object_mut().unwrap().remove("contractHash");
        f.policy.contract_hash = crate::engine_functions::contract::hash(&p);
        TrustedFunctionInput { local_name: "probe".into(), target: f.target, signature: f.abi, policy: f.policy, requirement: "required".into() }
    }
    fn selected() -> SelectedLayoutData {
        let bytes: Arc<str> = "{}".into();
        SelectedLayoutData { sha256: crate::engine_functions::contract::hash_bytes(bytes.as_bytes()), bytes }
    }
    fn dispatch() {
        let token = registry::next_id().unwrap(); TOKEN.with(|t| t.set(token));
        let info = S2FunctionFrameInfo { version: 1, struct_size: 48, frame_token: token, native_epoch: token, invocation_id: token,
            suppressed_owner: 0, parameter_count: 2, flags: 0 };
        crate::ffi::s2script_core_dispatch_function(1, &info, 0);
        crate::ffi::s2script_core_dispatch_function(1, &info, 1);
    }
    fn js(src: &str) -> String { frame_tests::eval_in_context_string("hidden-a", src) }
    #[test]
    fn hidden_referenced_by_and_string_indirect_on_trusted_views() {
        scalar_transport_tests::init_transport();
        let mut ops = engine_ops().unwrap();
        ops.schema_offset = Some(schema); ops.function_prepare_instance = Some(prepare); ops.function_instance_activate = Some(activate);
        ops.function_instance_release = Some(release); ops.function_frame_read_instance = Some(presence);
        ops.function_frame_field_read = Some(field_read); ops.function_frame_field_write = Some(field_write);
        ops.function_call_copy = Some(call_copy); ops.function_frame_read_copy = Some(read_copy); ops.function_frame_write_copy = Some(write_copy);
        ops.function_frame_commit_copy = Some(commit_copy); ops.function_frame_override_return_copy = Some(override_copy);
        ops.function_frame_hidden_referenced_by = Some(related);
        set_engine_ops(Some(ops));
        let host = HostPackageOwner::mint("@proof/hidden-relation").unwrap();
        let source = register_prepared_package(host.clone(), SOURCE.into(), ImplementationManifestHash::new(
            crate::engine_functions::contract::hash_bytes(b"hidden-relation-fixture-manifest-v1")).unwrap()).unwrap();
        // Public-v2 never admits the trusted projection; trusted validation keeps it readonly.
        let mut writable = input(); writable.signature.parameters[1].mutable = vec!["pre".into()];
        assert!(prepare_verified_package(&source, vec![writable], selected(), unsafe { SynchronousRecordLifetime::registered_native_target() }).is_err());
        let mut returned = input(); returned.signature.parameters[1].ownership = Some("callee-borrowed".into());
        assert!(prepare_verified_package(&source, vec![returned], selected(), unsafe { SynchronousRecordLifetime::registered_native_target() }).is_err());
        let candidate = prepare_verified_package(&source, vec![input()], selected(), unsafe { SynchronousRecordLifetime::registered_native_target() }).unwrap();
        let active = registry::activate_package_owner(registry::prepare_package_owner(&host, candidate).unwrap(), &host).unwrap();
        let result = std::panic::catch_unwind(|| {
            frame_tests::load_body("hidden-a", "return {};", "{}");
            let id = crate::entity_live::on_created(611, 42);
            with_host_isolate(|isolate| {
                let mut storage = v8::HandleScope::new(isolate); let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
                let context = clone_plugin_context("hidden-a").unwrap(); let context = v8::Local::new(&mut scope, &context);
                let scope = &mut v8::ContextScope::new(&mut scope, context); let global = context.global(scope);
                let entity = interop_wire::projected_entity_ref(scope, projection::EntityReference { index: 611, id }).unwrap();
                set_own(scope, global, "owner", entity.into()).unwrap();
            }).unwrap();
            js(r#"globalThis.log=[];check=(v,phase)=>{
                const threw=f=>{try{f();return false}catch(_){return true}};
                log.push([phase,typeof v.hiddenReferencedBy,'services' in v,Object.keys(v).includes('services'),v.label,
                  v.hiddenReferencedBy('services',owner,'HiddenRelationOwnerProbe','m_pServices'),
                  v.hiddenReferencedBy('services',owner,'HiddenRelationOwnerProbe','m_missing'),
                  threw(()=>v.hiddenReferencedBy('label',owner,'HiddenRelationOwnerProbe','m_pServices')),
                  threw(()=>v.hiddenReferencedBy('services',{index:611,id:owner.id},'HiddenRelationOwnerProbe','m_pServices')),
                  threw(()=>v.hiddenReferencedBy('services',owner,7,'m_pServices')),
                  threw(()=>{v.label='x'})].join());
                saved=v.hiddenReferencedBy;};''"#);
            dispatch();
            assert_eq!(js("log.join('|')"), "pre,function,false,false,label-text,true,false,true,true,true,true|post,function,false,false,label-text,true,false,true,true,true,true");
            let calls = CALLS.with(|c| c.borrow().clone());
            assert_eq!(calls, vec![Call { selector: 0, index: 611, serial: 42, offset: 0x40 }; 2], "only resolved fields reach native");
            // Native "not referenced" is false; an escaped method is an expired lease.
            ANSWER.with(|a| a.set(0)); js("log=[];''"); dispatch();
            assert!(js("log[0]").starts_with("pre,function,false,false,label-text,false,"));
            assert_eq!(js("try{saved('services',owner,'HiddenRelationOwnerProbe','m_pServices');'live'}catch(e){'expired'}"), "expired");
            // A stale entity is simply unrelated and never reaches native.
            crate::entity_live::on_deleted(611, 42); CALLS.with(|c| c.borrow_mut().clear()); ANSWER.with(|a| a.set(1));
            js("log=[];check=(v)=>log.push(v.hiddenReferencedBy('services',owner,'HiddenRelationOwnerProbe','m_pServices'));''"); dispatch();
            assert_eq!(js("log.join()"), "false,false"); assert!(CALLS.with(|c| c.borrow().is_empty()));
        });
        unload_plugin("hidden-a"); drop(active); drop(source); set_engine_ops(None); shutdown();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }
}

//! Bounded scalar, entity and owned-copy transport. Native addresses stay in the shim.
use super::contract::{OwnerKey, OwnerKind};
use super::instance::Function;
use super::{copied, projection::ProjectedValue};
use crate::v8host::{engine_ops, S2FunctionFrameInfo, S2FunctionHookStatus, S2FunctionValue};
use std::ffi::{CStr, CString};
pub(crate) fn blank() -> S2FunctionValue {
    S2FunctionValue {
        kind: 0,
        flags: 0,
        reserved: 0,
        aux: 0,
        bits: 0,
    }
}
pub(crate) fn kind(atom: &str) -> Result<u8, String> {
    Ok(match atom {
        "void" => 0,
        "u8" => 1,
        "i32" => 2,
        "u32" => 3,
        "i64" => 4,
        "u64" => 5,
        "f32" => 6,
        "f64" => 7,
        _ => return Err(format!("unsupported scalar proof projection: {atom}")),
    })
}
fn reason(buf: &[i8]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}
pub(crate) fn has_copies(abi: &super::instance::Signature) -> bool {
    copied::flag(&abi.returns.projection.id).is_some()
        || abi
            .parameters
            .iter()
            .any(|p| copied::flag(&p.projection.id).is_some())
}
pub(crate) fn prepare(f: &Function) -> Result<i64, String> {
    if f.trusted() {return Err("trusted instance requires prepared binding authority".into());}
    if has_copies(&f.abi) {
        require_copy_ops()?;
    }
    super::projection::request(&f.abi.returns.native, &f.abi.returns.projection.id)?;
    for p in &f.abi.parameters {
        super::projection::request(&p.native, &p.projection.id)?;
    }
    let op = engine_ops()
        .and_then(|o| o.function_prepare)
        .ok_or("native function prepare unavailable")?;
    let args = [
        f.canonical_id.clone(),
        serde_json::to_string(&f.target).unwrap(),
        f.abi.public_wire().to_string(),
        f.abi.fingerprint.clone(),
    ];
    let args = args
        .into_iter()
        .map(CString::new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "NUL in declaration")?;
    let mut why = [0; 512];
    let id = op(
        args[0].as_ptr(),
        args[1].as_ptr(),
        args[2].as_ptr(),
        args[3].as_ptr(),
        why.as_mut_ptr(),
        512,
    );
    if id <= 0 {
        Err(reason(&why))
    } else {
        Ok(id)
    }
}
pub(crate) fn instance_owner(owner:&OwnerKey) -> crate::v8host::S2FunctionInstanceOwner {
    use sha2::{Digest,Sha256};
    crate::v8host::S2FunctionInstanceOwner {version:1,struct_size:56,
        kind:if owner.kind==OwnerKind::Plugin {1} else {2},reserved:0,
        id_digest:Sha256::digest(owner.id.as_bytes()).into(),generation:owner.generation}
}
pub(crate) fn prepare_binding(f:&Function,binding:u64) -> Result<(i64,Option<u64>),String> {
    if !f.trusted() {return prepare(f).map(|t|(t,None));}
    if has_copies(&f.abi) {require_copy_ops()?;}
    let ops=engine_ops().ok_or("native instance operations unavailable")?;
    if ops.function_instance_activate.is_none() || ops.function_instance_release.is_none()
        || ops.function_frame_read_instance.is_none() || ops.function_frame_field_read.is_none() || ops.function_frame_field_write.is_none() {
        return Err("native instance capability operations unavailable".into());
    }
    let op=ops.function_prepare_instance.ok_or("native instance prepare unavailable")?;
    let name=CString::new(f.canonical_id.as_str()).map_err(|_|"invalid instance name")?;
    let target=CString::new(serde_json::to_string(&f.target).unwrap()).unwrap();
    let contract=CString::new(f.trusted_wire()?).map_err(|_|"invalid instance contract")?;
    let owner=instance_owner(f.host_owner().ok_or("host instance owner unavailable")?);
    let mut out=crate::v8host::S2FunctionInstancePrepared {version:1,struct_size:24,target:0,capability:0};
    let mut why=[0;512];
    if op(binding,&owner,name.as_ptr(),target.as_ptr(),contract.as_ptr(),&mut out,why.as_mut_ptr(),512)!=1 {return Err(reason(&why));}
    if out.version!=1 || out.struct_size!=24 || out.target<=0 || out.capability==0 {
        if out.capability!=0 {instance_release(out.capability);} if out.target>0 {target_release(out.target);}
        return Err("invalid native instance preparation receipt".into());
    }
    Ok((out.target,Some(out.capability)))
}
pub(crate) fn instance_activate(capability:u64,owner:&OwnerKey) -> Result<(),String> {
    let op=engine_ops().and_then(|o|o.function_instance_activate).ok_or("native instance activation unavailable")?;
    let mut why=[0;512];if op(capability,&instance_owner(owner),why.as_mut_ptr(),512)==1 {Ok(())} else {Err(reason(&why))}
}
pub(crate) fn instance_release(capability:u64) {
    if let Some(op)=engine_ops().and_then(|o|o.function_instance_release) {op(capability);}
}
pub(crate) fn target_release(id: i64) {
    if let Some(op) = engine_ops().and_then(|o| o.function_target_release) {
        op(id);
    }
}
pub(crate) fn hook_acquire(id: i64) -> Result<(), String> {
    let op = engine_ops()
        .and_then(|o| o.function_hook_acquire)
        .ok_or("native hook acquire unavailable")?;
    let mut why = [0; 512];
    if op(id, why.as_mut_ptr(), 512) <= 0 {
        Err(reason(&why))
    } else {
        Ok(())
    }
}
pub(crate) fn hook_release(id: i64) {
    if let Some(op) = engine_ops().and_then(|o| o.function_hook_release) {
        op(id);
    }
}
pub(crate) fn status(id: i64) -> Result<S2FunctionHookStatus, String> {
    let op = engine_ops()
        .and_then(|o| o.function_hook_status)
        .ok_or("native hook status unavailable")?;
    let mut out = S2FunctionHookStatus {
        state: 0,
        reserved: 0,
        receipt: 0,
    };
    let mut why = [0; 512];
    if op(id, &mut out, why.as_mut_ptr(), 512) == 1 {
        Ok(out)
    } else {
        Err(reason(&why))
    }
}
pub(crate) fn call(
    id: i64,
    caller: Option<&OwnerKey>,
    values: &[S2FunctionValue],
) -> Result<S2FunctionValue, String> {
    call_requested(id, caller, values, blank())
}
fn call_requested(
    id: i64,
    caller: Option<&OwnerKey>,
    values: &[S2FunctionValue],
    mut out: S2FunctionValue,
) -> Result<S2FunctionValue, String> {
    // Mark the exact caller for the complete outbound FFI scope, regardless of
    // which host entry originally invoked its JavaScript.
    if let Some(caller) = caller {
        if caller.kind != OwnerKind::Plugin
            || !crate::v8host::owner_is_live(&caller.id, caller.generation)
            || crate::v8host::plugin_phase(&caller.id) == Some(crate::plugin::Phase::Unloading)
        {
            return Err("function caller generation unavailable".into());
        }
    }
    let _busy = caller.map(|p| crate::dispatch::ParentBusy::enter_target(&p.id, p.generation, id));
    let owner = caller.map_or(0, |p| p.generation);
    let op = engine_ops()
        .and_then(|o| o.function_call)
        .ok_or("native function call unavailable")?;
    let mut why = [0; 512];
    if op(
        id,
        owner,
        values.as_ptr(),
        values.len() as i32,
        &mut out,
        why.as_mut_ptr(),
        512,
    ) == 1
    {
        Ok(out)
    } else {
        Err(reason(&why))
    }
}
/// Exact owned binding supplies every projection; physical target metadata supplies ABI only.
pub(crate) fn call_binding(
    binding: &super::registry::Binding,
    caller: &OwnerKey,
    values: &[super::projection::ProjectedValue],
) -> Result<super::projection::ProjectedValue, String> {
    if binding.owner.kind != OwnerKind::Plugin || binding.owner != *caller {
        return Err("public binding caller mismatch".into());
    }
    call_binding_from(binding, caller, values)
}
/// The V8 caller has validated the sealed instance token; retain exact identities here too.
pub(crate) fn call_package_binding(
    binding: &super::registry::Binding,
    instance: &super::contract::PackageInstanceKey,
    values: &[super::projection::ProjectedValue],
) -> Result<super::projection::ProjectedValue, String> {
    if binding.owner.kind != OwnerKind::GamePackage || binding.owner != instance.package_owner {
        return Err("package binding caller mismatch".into());
    }
    call_binding_from(binding, &instance.parent, values)
}
/// Caller is supplied by the checked public facade or exact package-instance facade.
fn call_binding_from(
    binding: &super::registry::Binding,
    caller: &OwnerKey,
    values: &[super::projection::ProjectedValue],
) -> Result<super::projection::ProjectedValue, String> {
    let operation = || {
        super::registry::binding(binding.id, &binding.owner)?;
        if !binding.function.policy.surfaces.iter().any(|s| s == "call") {
            return Err("undeclared call surface".into());
        }
        let abi = &binding.function.abi;
        if abi.borrowed() {return Err("record/hidden native input cannot be manufactured".into());}
        let receiver = usize::from(abi.member_receiver);
        if values.len() != abi.parameters.len() + receiver {
            return Err("argument count mismatch".into());
        }
        if has_copies(abi) {
            return call_copied(binding, caller, values);
        }
        let mut wire = Vec::with_capacity(values.len());
        for (i, value) in values.iter().enumerate() {
            let (native, projection) = if i == 0 && receiver == 1 {
                ("ptr", "entity")
            } else {
                let p = &abi.parameters[i - receiver];
                (p.native.as_str(), p.projection.id.as_str())
            };
            let request = super::projection::request(native, projection)?;
            let value = super::projection::encode(value.clone())?;
            if value.kind != request.kind || value.flags != request.flags {
                return Err("binding argument projection mismatch".into());
            }
            wire.push(value);
        }
        let ret = &abi.returns;
        let result = call_requested(
            binding.target.ok_or("binding unavailable")?,
            Some(caller),
            &wire,
            super::projection::request(&ret.native, &ret.projection.id)?,
        )?;
        super::projection::decode(result, &ret.native, &ret.projection.id)
    };
    operation().map_err(|e: String| format!("{}: {e}", binding.function.canonical_id))
}
#[derive(Clone)]
pub(crate) struct Frame {
    pub target: i64,
    pub info: S2FunctionFrameInfo,
    pub phase: i32,
    pub fingerprint: CString,
}
impl Frame {
    fn instance_access(&self,binding:&super::registry::Binding) -> Result<crate::v8host::S2FunctionInstanceAccess,String> {
        super::registry::binding(binding.id,&binding.owner)?;
        if binding.target!=Some(self.target) {return Err("instance frame target mismatch".into());}
        Ok(crate::v8host::S2FunctionInstanceAccess {version:1,struct_size:48,target:self.target,
            frame_token:self.info.frame_token,native_epoch:self.info.native_epoch,
            capability:binding.capability.ok_or("instance capability unavailable")?,binding_id:binding.id})
    }
    pub(crate) fn read_instance(&self,binding:&super::registry::Binding,selector:i32) -> Result<S2FunctionValue,String> {
        let key=self.instance_access(binding)?;
        let op=engine_ops().and_then(|o|o.function_frame_read_instance).ok_or("native instance read unavailable")?;
        let mut out=blank();let mut why=[0;512];
        if op(&key,selector,&mut out,why.as_mut_ptr(),512)==1 {Ok(out)} else {Err(reason(&why))}
    }
    pub(crate) fn read_field(&self,binding:&super::registry::Binding,selector:i32,field:u32) -> Result<ProjectedValue,String> {
        let key=self.instance_access(binding)?;
        let row=binding.function.abi.layout(selector)?.fields.get(field as usize).ok_or("unknown record field")?;
        let op=engine_ops().and_then(|o|o.function_frame_field_read).ok_or("native record read unavailable")?;
        let mut out=blank();let mut why=[0;512];
        if op(&key,selector,field,&mut out,why.as_mut_ptr(),512)!=1 {return Err(reason(&why));}
        let (native,projection)=row.projection();super::projection::decode(out,native,projection)
    }
    pub(crate) fn write_field(&self,binding:&super::registry::Binding,selector:i32,field:u32,value:&ProjectedValue) -> Result<(),String> {
        let key=self.instance_access(binding)?;
        let op=engine_ops().and_then(|o|o.function_frame_field_write).ok_or("native record write unavailable")?;
        let wire=super::projection::encode(value.clone())?;let mut why=[0;512];
        if op(&key,selector,field,&wire,why.as_mut_ptr(),512)==1 {Ok(())} else {Err(reason(&why))}
    }
    pub(crate) fn validate(
        target: i64,
        info: S2FunctionFrameInfo,
        phase: i32,
        fingerprint: &str,
    ) -> Result<Self, String> {
        if target <= 0
            || info.version != 1
            || info.struct_size != 48
            || info.frame_token == 0
            || info.native_epoch == 0
            || info.invocation_id == 0
            || info.flags > 1
            || info.parameter_count > 32
            || ![0, 1].contains(&phase)
        {
            return Err("invalid native frame metadata".into());
        }
        Ok(Self {
            target,
            info,
            phase,
            fingerprint: CString::new(fingerprint).map_err(|_| "invalid fingerprint")?,
        })
    }
    pub(crate) fn read(&self, selector: i32, kind: u8) -> Result<S2FunctionValue, String> {
        let mut request = blank();
        request.kind = kind;
        self.read_requested(selector, request)
    }
    pub(crate) fn read_requested(
        &self,
        selector: i32,
        mut out: S2FunctionValue,
    ) -> Result<S2FunctionValue, String> {
        let kind = out.kind;
        let op = engine_ops()
            .and_then(|o| o.function_frame_read)
            .ok_or("native frame read unavailable")?;
        let mut why = [0; 512];
        if op(
            self.target,
            self.info.frame_token,
            self.info.native_epoch,
            self.fingerprint.as_ptr(),
            selector,
            kind,
            &mut out,
            why.as_mut_ptr(),
            512,
        ) == 1
        {
            Ok(out)
        } else {
            Err(reason(&why))
        }
    }
    pub(crate) fn write(&self, selector: i32, value: &S2FunctionValue) -> Result<(), String> {
        let op = engine_ops()
            .and_then(|o| o.function_frame_write)
            .ok_or("native frame write unavailable")?;
        let mut why = [0; 512];
        if op(
            self.target,
            self.info.frame_token,
            self.info.native_epoch,
            self.fingerprint.as_ptr(),
            selector,
            value,
            why.as_mut_ptr(),
            512,
        ) == 1
        {
            Ok(())
        } else {
            Err(reason(&why))
        }
    }
    pub(crate) fn commit(
        &self,
        action: i32,
        value: Option<&S2FunctionValue>,
    ) -> Result<(), String> {
        let op = engine_ops()
            .and_then(|o| o.function_frame_commit)
            .ok_or("native frame commit unavailable")?;
        let mut why = [0; 512];
        if op(
            self.target,
            self.info.frame_token,
            self.info.native_epoch,
            self.fingerprint.as_ptr(),
            action,
            value.map_or(std::ptr::null(), |v| v),
            why.as_mut_ptr(),
            512,
        ) == 1
        {
            Ok(())
        } else {
            Err(reason(&why))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_layout_is_final_v1() {
        assert_eq!(
            std::mem::size_of::<crate::v8host::S2FunctionCopyInput>(),
            24
        );
        assert_eq!(
            std::mem::size_of::<crate::v8host::S2FunctionCopyOutput>(),
            32
        );
        assert_eq!(
            std::mem::size_of::<crate::v8host::S2FunctionCopyProducer>(),
            56
        );
        assert_eq!(
            std::mem::offset_of!(crate::v8host::S2FunctionCopyProducer, generation),
            48
        );
        assert_eq!(std::mem::size_of::<crate::v8host::S2FunctionValue>(), 16);
        assert_eq!(std::mem::size_of::<S2FunctionFrameInfo>(), 48);
        assert_eq!(std::mem::align_of::<S2FunctionFrameInfo>(), 8);
        assert_eq!(std::mem::offset_of!(S2FunctionFrameInfo, invocation_id), 24);
        assert_eq!(std::mem::offset_of!(S2FunctionFrameInfo, flags), 44);
        assert_eq!(std::mem::size_of::<S2FunctionHookStatus>(), 16);
        assert!(kind("ptr").is_err());
    }
}

/// Only the leased adapter service can construct the permit required here.
pub(crate) fn original_return(
    permit: &crate::v8host::function_adapter::AdapterPostReturnPermit,
) -> Result<Option<super::projection::ProjectedValue>, String> {
    let (frame, binding) = permit.validate()?;
    let ret = &binding.function.abi.returns;
    if frame.info.flags & 1 != 0 || ret.native == "void" {
        return Ok(None);
    }
    frame
        .read_projected(-3, &ret.native, &ret.projection.id)
        .map(Some)
        .map_err(|e| format!("{}: {e}", binding.function.canonical_id))
}
pub(crate) fn override_return(
    permit: &crate::v8host::function_adapter::AdapterPostReturnPermit,
    value: super::projection::ProjectedValue,
) -> Result<super::projection::ProjectedValue, String> {
    let (frame, binding) = permit.validate()?;
    let run = || {
        let ret = &binding.function.abi.returns;
        let mut out = super::projection::request(&ret.native, &ret.projection.id)?;
        if out.kind == 0 {
            return Err("void return cannot be overridden".into());
        }
        if matches!(
            (out.kind, &value),
            (8, super::projection::ProjectedValue::Scalar(_))
                | (1..=7, super::projection::ProjectedValue::Entity { .. })
        ) {
            return Err("binding return value category mismatch".into());
        }
        if let ProjectedValue::Copied(copy) = &value {
            if copy.flags != out.flags {
                return Err("binding return projection mismatch".into());
            }
            let op = engine_ops()
                .and_then(|o| o.function_frame_override_return_copy)
                .ok_or("FunctionCopyExecutionUnavailable: POST sidecar")?;
            let mut buffer =
                copied::Buffer::new(copied::max_size(copy.flags), copied::Producer::engine())?;
            let mut output = buffer.output();
            let mut why = [0; 512];
            if op(
                frame.target,
                frame.info.frame_token,
                frame.info.native_epoch,
                frame.fingerprint.as_ptr(),
                &copy.wire(),
                &copy.input(),
                &copy.producer().wire(),
                &mut out,
                &mut output,
                why.as_mut_ptr(),
                512,
            ) != 1
            {
                return Err(reason(&why));
            }
            return buffer
                .finish(copy.flags, out, &output)
                .map(ProjectedValue::Copied)
                .map_err(|e| format!("FunctionCopyPostSubmitFailure: {e}"));
        }
        let wire = super::projection::encode(value)?;
        if wire.kind != out.kind
            || wire.flags != out.flags
            || wire.reserved != 0
            || (wire.kind != 8
                && (wire.aux != 0
                    || match wire.kind {
                        1 => wire.bits > 1,
                        2 | 3 => wire.bits > u32::MAX as u64,
                        6 => {
                            wire.bits > u32::MAX as u64
                                || !f32::from_bits(wire.bits as u32).is_finite()
                        }
                        7 => !f64::from_bits(wire.bits).is_finite(),
                        _ => false,
                    }))
        {
            return Err("binding return projection mismatch".into());
        }
        let op = engine_ops()
            .and_then(|o| o.function_frame_override_return)
            .ok_or("native POST override unavailable")?;
        let mut why = [0; 512];
        if op(
            frame.target,
            frame.info.frame_token,
            frame.info.native_epoch,
            frame.fingerprint.as_ptr(),
            &wire,
            &mut out,
            why.as_mut_ptr(),
            512,
        ) != 1
        {
            return Err(reason(&why));
        }
        super::projection::decode(out, &ret.native, &ret.projection.id)
            .map_err(|e| format!("POST override submitted: readback projection failed: {e}"))
    };
    run().map_err(|e: String| format!("{}: {e}", binding.function.canonical_id))
}

fn require_copy_ops() -> Result<(), String> {
    let o = engine_ops().ok_or("FunctionCopyExecutionUnavailable: engine ops")?;
    for (name, present) in [
        ("function_call_copy", o.function_call_copy.is_some()),
        (
            "function_frame_read_copy",
            o.function_frame_read_copy.is_some(),
        ),
        (
            "function_frame_write_copy",
            o.function_frame_write_copy.is_some(),
        ),
        (
            "function_frame_commit_copy",
            o.function_frame_commit_copy.is_some(),
        ),
        (
            "function_frame_override_return_copy",
            o.function_frame_override_return_copy.is_some(),
        ),
    ] {
        if !present {
            return Err(format!("FunctionCopyExecutionUnavailable: {name}"));
        }
    }
    Ok(())
}
fn call_copied(
    binding: &super::registry::Binding,
    caller: &OwnerKey,
    values: &[ProjectedValue],
) -> Result<ProjectedValue, String> {
    if caller.kind != OwnerKind::Plugin
        || !crate::v8host::owner_is_live(&caller.id, caller.generation)
        || crate::v8host::plugin_phase(&caller.id) == Some(crate::plugin::Phase::Unloading)
    {
        return Err("function caller generation unavailable".into());
    }
    require_copy_ops()?;
    let producer = copied::Producer::owner(caller);
    let abi = &binding.function.abi;
    let receiver = usize::from(abi.member_receiver);
    let mut wire = [blank(); 33];
    let mut size = 0;
    for (i, value) in values.iter().enumerate() {
        let (native, projection) = if i == 0 && receiver == 1 {
            ("ptr", "entity")
        } else {
            let p = &abi.parameters[i - receiver];
            (p.native.as_str(), p.projection.id.as_str())
        };
        let request = super::projection::request(native, projection)?;
        wire[i] = match value {
            ProjectedValue::Copied(v) => {
                // Public call values are newly materialized under the actual caller.
                if v.producer() != producer {
                    return Err("FunctionCopyProducerMismatch: direct call input".into());
                }
                let mut w = v.wire();
                w.bits = size as u64;
                size += v.bytes().len();
                w
            }
            other => super::projection::encode(other.clone())?,
        };
        if wire[i].kind != request.kind || wire[i].flags != request.flags {
            return Err("binding argument projection mismatch".into());
        }
    }
    let mut input = copied::Buffer::new(size, producer)?;
    let mut offset = 0;
    for value in values {
        if let ProjectedValue::Copied(v) = value {
            let n = v.bytes().len();
            input.bytes_mut()[offset..offset + n].copy_from_slice(v.bytes());
            offset += n;
        }
    }
    let ret = &abi.returns;
    let flag = copied::flag(&ret.projection.id);
    let mut buffer =
        copied::Buffer::new(flag.map_or(0, copied::max_size), copied::Producer::engine())?;
    let mut output = buffer.output();
    let mut out = super::projection::request(&ret.native, &ret.projection.id)?;
    let mut why = [0; 512];
    let target = binding.target.ok_or("binding unavailable")?;
    let _busy = crate::dispatch::ParentBusy::enter_target(&caller.id, caller.generation, target);
    let op = engine_ops().unwrap().function_call_copy.unwrap();
    if op(
        binding.target.ok_or("binding unavailable")?,
        caller.generation,
        wire.as_ptr(),
        values.len() as i32,
        &mut out,
        &input.input(),
        &mut output,
        &producer.wire(),
        why.as_mut_ptr(),
        512,
    ) != 1
    {
        return Err(reason(&why));
    }
    if let Some(flag) = flag {
        buffer
            .finish(flag, out, &output)
            .map(ProjectedValue::Copied)
            .map_err(|e| format!("FunctionCopyPostCallFailure: {e}"))
    } else {
        super::projection::decode(out, &ret.native, &ret.projection.id)
            .map_err(|e| format!("FunctionCopyPostCallFailure: {e}"))
    }
}
impl Frame {
    pub(crate) fn read_projected(
        &self,
        selector: i32,
        native: &str,
        projection: &str,
    ) -> Result<ProjectedValue, String> {
        let Some(flag) = copied::flag(projection) else {
            return self
                .read_requested(selector, super::projection::request(native, projection)?)
                .and_then(|v| super::projection::decode(v, native, projection));
        };
        let mut buffer = copied::Buffer::new(copied::max_size(flag), copied::Producer::engine())?;
        let mut output = buffer.output();
        let mut value = super::projection::request(native, projection)?;
        let mut why = [0; 512];
        let op = engine_ops()
            .and_then(|o| o.function_frame_read_copy)
            .ok_or("FunctionCopyExecutionUnavailable: read sidecar")?;
        if op(
            self.target,
            self.info.frame_token,
            self.info.native_epoch,
            self.fingerprint.as_ptr(),
            selector,
            &mut value,
            &mut output,
            why.as_mut_ptr(),
            512,
        ) != 1
        {
            return Err(reason(&why));
        }
        buffer
            .finish(flag, value, &output)
            .map(ProjectedValue::Copied)
    }
    pub(crate) fn write_projected(
        &self,
        selector: i32,
        value: &ProjectedValue,
    ) -> Result<(), String> {
        let ProjectedValue::Copied(copy) = value else {
            return self.write(selector, &super::projection::encode(value.clone())?);
        };
        let op = engine_ops()
            .and_then(|o| o.function_frame_write_copy)
            .ok_or("FunctionCopyExecutionUnavailable: write sidecar")?;
        let mut why = [0; 512];
        if op(
            self.target,
            self.info.frame_token,
            self.info.native_epoch,
            self.fingerprint.as_ptr(),
            selector,
            &copy.wire(),
            &copy.input(),
            &copy.producer().wire(),
            why.as_mut_ptr(),
            512,
        ) == 1
        {
            Ok(())
        } else {
            Err(reason(&why))
        }
    }
    pub(crate) fn commit_projected(
        &self,
        action: i32,
        value: Option<&ProjectedValue>,
        copies: bool,
    ) -> Result<(), String> {
        if !copies {
            let wire = value.cloned().map(super::projection::encode).transpose()?;
            return self.commit(action, wire.as_ref());
        }
        let op = engine_ops()
            .and_then(|o| o.function_frame_commit_copy)
            .ok_or("FunctionCopyExecutionUnavailable: commit sidecar")?;
        let mut input = copied::empty_input();
        let mut producer = copied::Producer::engine().wire();
        let wire = match value {
            Some(ProjectedValue::Copied(copy)) => {
                input = copy.input();
                producer = copy.producer().wire();
                Some(copy.wire())
            }
            Some(other) => Some(super::projection::encode(other.clone())?),
            None => None,
        };
        let mut why = [0; 512];
        if op(
            self.target,
            self.info.frame_token,
            self.info.native_epoch,
            self.fingerprint.as_ptr(),
            action,
            wire.as_ref().map_or(std::ptr::null(), |v| v),
            &input,
            &producer,
            why.as_mut_ptr(),
            512,
        ) == 1
        {
            Ok(())
        } else {
            Err(reason(&why))
        }
    }
}

impl Frame {
    /// Hidden-position relationship check on the exact trusted binding's top frame.
    /// `entity` is an already books-gated strict identity; `offset` is a live schema
    /// offset. The shim compares raw bits only; no address crosses this boundary.
    pub(crate) fn hidden_referenced_by(&self, binding: &super::registry::Binding, selector: i32,
        entity: &S2FunctionValue, offset: u32) -> Result<bool, String> {
        let key = self.instance_access(binding)?;
        let op = engine_ops().and_then(|o| o.function_frame_hidden_referenced_by)
            .ok_or("native relationship check unavailable")?;
        let (mut out, mut why) = (0, [0; 512]);
        if op(&key, selector, entity, offset, &mut out, why.as_mut_ptr(), 512) != 1 {return Err(reason(&why));}
        match out {0 => Ok(false), 1 => Ok(true), _ => Err("invalid native relationship result".into())}
    }
}

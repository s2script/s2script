//! The bounded scalar native transport. All pointers remain inside this C boundary.
use super::contract::NormalizedFunction;
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
pub(crate) fn prepare(f: &NormalizedFunction) -> Result<i64, String> {
    if f.abi.receiver != "none" {
        return Err("receiver projection awaits full Task 6".into());
    }
    kind(&f.abi.returns.native)?;
    for p in &f.abi.parameters {
        kind(&p.native)?;
    }
    let op = engine_ops()
        .and_then(|o| o.function_prepare)
        .ok_or("native function prepare unavailable")?;
    let args = [
        f.canonical_id.clone(),
        serde_json::to_string(&f.target).unwrap(),
        serde_json::to_string(&f.abi).unwrap(),
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
    owner: u64,
    values: &[S2FunctionValue],
) -> Result<S2FunctionValue, String> {
    // Mark the exact caller for the complete outbound FFI scope, regardless of
    // which host entry originally invoked its JavaScript.
    let parent = if owner == 0 {
        None
    } else {
        Some(
            super::registry::owner_for_token(owner)
                .ok_or("function caller generation unavailable")?,
        )
    };
    let _busy = parent
        .as_ref()
        .map(|p| crate::dispatch::ParentBusy::enter(&p.id, p.generation));
    let op = engine_ops()
        .and_then(|o| o.function_call)
        .ok_or("native function call unavailable")?;
    let mut out = blank();
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
#[derive(Clone)]
pub(crate) struct Frame {
    pub target: i64,
    pub info: S2FunctionFrameInfo,
    pub phase: i32,
    pub fingerprint: CString,
}
impl Frame {
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
        let op = engine_ops()
            .and_then(|o| o.function_frame_read)
            .ok_or("native frame read unavailable")?;
        let mut out = blank();
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
        assert_eq!(std::mem::size_of::<S2FunctionFrameInfo>(), 48);
        assert_eq!(std::mem::align_of::<S2FunctionFrameInfo>(), 8);
        assert_eq!(std::mem::offset_of!(S2FunctionFrameInfo, invocation_id), 24);
        assert_eq!(std::mem::offset_of!(S2FunctionFrameInfo, flags), 44);
        assert_eq!(std::mem::size_of::<S2FunctionHookStatus>(), 16);
        assert!(kind("ptr").is_err());
    }
}

mod manifest;
#[cfg(test)]
mod tests;

pub(crate) use manifest::{prepare_selection, PreparedSelection};
#[cfg(test)]
pub(crate) use manifest::PackageError;

use crate::engine_functions::contract::{HostPackageOwner, ImplementationManifestHash};
use crate::v8host::function_adapter::{self, PreparedPackageReceipt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

struct RegisteredPackage {
    selection: PreparedSelection,
    _receipt: PreparedPackageReceipt,
    _merged_data: Box<str>,
    status: Vec<u8>,
}
thread_local! {
    static PENDING: RefCell<Option<(u64, PreparedSelection)>> = const { RefCell::new(None) };
    static REGISTERED: RefCell<Option<RegisteredPackage>> = const { RefCell::new(None) };
    static STATUS: RefCell<Vec<u8>> = RefCell::new(b"{\"code\":\"unselected\"}".to_vec());
}
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

fn metadata(selection: &PreparedSelection) -> Value {
    json!({"code":"prepared", "id":selection.id, "gamedataOwner":selection.gamedata_owner,
        "bootstrapSha256":selection.bootstrap_sha256,"gamedataSha256":selection.gamedata_sha256,
        "manifestPath":selection.provenance.manifest_path,
        "target":{"engine":selection.provenance.engine,"game":selection.provenance.game,
            "platform":selection.provenance.platform}})
}
pub(crate) fn report_error(error: &str) {
    STATUS.with(|s| {
        *s.borrow_mut() = json!({"code":"failed","error":error})
            .to_string()
            .into_bytes()
    });
}
pub(crate) fn status() -> Vec<u8> {
    REGISTERED
        .with(|r| r.borrow().as_ref().map(|r| r.status.clone()))
        .unwrap_or_else(|| STATUS.with(|s| s.borrow().clone()))
}
pub(crate) fn selected_id() -> Option<String> {
    REGISTERED.with(|r| r.borrow().as_ref().map(|r| r.selection.id.clone()))
}
pub(crate) fn select(root: &Path, engine: &str, game: &str, platform: &str) -> Result<u64, String> {
    if REGISTERED.with(|r| r.borrow().is_some()) || PENDING.with(|p| p.borrow().is_some()) {
        return Err("package selection already retained".into());
    }
    let selection = prepare_selection(root, engine, game, platform)
        .map_err(|e| format!("{}: {:?}", e.code(), e.candidates()))?;
    // One pending record, with bounded retained bytes even before native merge.
    if selection.bootstrap_bytes.is_empty()
        || selection.bootstrap_bytes.len() > 16 * 1024 * 1024
        || selection.gamedata_bytes.len() > 4 * 1024 * 1024
    {
        return Err("package artifact size limit".into());
    }
    let handle = NEXT_HANDLE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| "selection handles exhausted")?;
    STATUS.with(|s| *s.borrow_mut() = metadata(&selection).to_string().into_bytes());
    PENDING.with(|p| *p.borrow_mut() = Some((handle, selection)));
    Ok(handle)
}
/// Returns owned bytes from the retained snapshot, never paths to reopen. 0=data, 1=metadata.
pub(crate) fn copy(handle: u64, member: u32) -> Result<Vec<u8>, String> {
    PENDING.with(|p| {
        let p = p.borrow();
        let (_, selection) = p
            .as_ref()
            .filter(|(h, _)| *h == handle)
            .ok_or("stale selection handle")?;
        match member {
            0 => Ok(selection.gamedata_bytes.clone()),
            1 => Ok(metadata(selection).to_string().into_bytes()),
            _ => Err("unknown selection member".into()),
        }
    })
}
pub(crate) fn abort(handle: u64) -> Result<(), String> {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().is_none_or(|(h, _)| *h != handle) {
            return Err("stale selection handle".into());
        }
        p.take();
        Ok(())
    })
}
pub(crate) fn commit(handle: u64, merged: &str, custom_paths: &str) -> Result<(), String> {
    if REGISTERED.with(|r| r.borrow().is_some()) {
        return Err("package already active".into());
    }
    let selection = PENDING
        .with(|p| {
            p.borrow()
                .as_ref()
                .filter(|(h, _)| *h == handle)
                .map(|(_, s)| s.clone())
        })
        .ok_or("stale selection handle")?;
    if merged.len() > 4 * 1024 * 1024 || custom_paths.len() > 65536 {
        return Err("merged package size limit".into());
    }
    let gd: Value = if merged.is_empty() {
        json!({})
    } else {
        serde_json::from_str(merged).map_err(|e| format!("invalid merged gamedata: {e}"))?
    };
    if !gd.is_object() {
        return Err("merged gamedata must be object".into());
    }
    for name in [
        "calls",
        "hooks",
        "signatures",
        "interfaces",
        "offsets",
        "keys",
    ] {
        if gd.get(name).is_some_and(|v| !v.is_object()) {
            return Err(format!("invalid merged {name}"));
        }
    }
    let custom: Vec<String> =
        serde_json::from_str(custom_paths).map_err(|_| "invalid custom provenance")?;
    let owner = crate::gamedata_calls::reserved_owner_id(&selection.id);
    if crate::gamedata_calls::game_package_owner().is_some() {
        return Err("legacy package owner already active".into());
    }
    let authority = HostPackageOwner::mint(&selection.id)?;
    let hash = ImplementationManifestHash::new(selection.bootstrap_sha256.clone())?;
    let source =
        std::str::from_utf8(&selection.bootstrap_bytes).map_err(|_| "invalid bootstrap UTF-8")?;
    let mut info = metadata(&selection);
    info["code"] = json!("active");
    info["mergedSha256"] = json!(format!("{:x}", Sha256::digest(merged.as_bytes())));
    info["customPaths"] = json!(custom);
    let status = info.to_string().into_bytes();
    // No public owner/source before every fallible preparation succeeds. Resolution failures are
    // descriptor-level unavailable entries. Hook reservations roll back if staging is dropped.
    let calls = crate::gamedata_calls::prepare_game_package(&owner, &gd);
    let hooks = crate::gamedata_hooks::prepare_game_package(&owner, &gd);
    let receipt = function_adapter::register_selected_package(authority, source.into(), hash)?;
    crate::gamedata_calls::commit_game_package(&owner, calls);
    crate::gamedata_hooks::commit_game_package(hooks);
    REGISTERED.with(|r| {
        *r.borrow_mut() = Some(RegisteredPackage {
            selection,
            _receipt: receipt,
            _merged_data: merged.into(),
            status,
        })
    });
    PENDING.with(|p| p.borrow_mut().take());
    Ok(())
}
/// Terminal retirement only. A plugin reload must never reach this path.
pub(crate) fn clear() -> Result<(), String> {
    if !crate::v8host::package_contexts_retired() {
        return Err("package contexts still live".into());
    }
    if let Some(package) = REGISTERED.with(|r| r.borrow_mut().take()) {
        let owner = crate::gamedata_calls::reserved_owner_id(&package.selection.id);
        crate::gamedata_calls::clear_game_package(&owner);
        crate::gamedata_hooks::drop_owner(&owner);
        drop(package);
    }
    PENDING.with(|p| p.borrow_mut().take());
    STATUS.with(|s| *s.borrow_mut() = b"{\"code\":\"unselected\"}".to_vec());
    Ok(())
}

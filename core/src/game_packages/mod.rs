mod manifest;
mod repair_snapshot;
mod trusted_source;
#[cfg(test)]
mod tests;

pub(crate) use manifest::{prepare_selection, PreparedSelection};
#[cfg(test)]
pub(crate) use manifest::PackageError;

use crate::engine_functions::contract::{HostPackageOwner, ImplementationManifestHash};
use crate::engine_functions::{contract, instance::SynchronousRecordLifetime, overrides, registry, trusted};
use crate::v8host::function_adapter::{self, PreparedPackageReceipt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

struct RegisteredPackage {
    selection: PreparedSelection,
    _functions: Option<registry::ActivePackageFunctions>,
    _receipt: PreparedPackageReceipt,
    _retention: Rc<RefCell<crate::loader::RetainedLease>>,
    _merged_data: Box<str>,
    _repair_snapshot: repair_snapshot::Snapshot,
    status: Vec<u8>,
}
struct PendingPackage {
    handle: u64,
    selection: PreparedSelection,
    overrides: Option<overrides::OverrideSet>,
    retention: Rc<RefCell<crate::loader::RetainedLease>>,
    preparation_bytes: usize,
}
thread_local! {
    static PENDING: RefCell<Option<PendingPackage>> = const { RefCell::new(None) };
    static REGISTERED: RefCell<Option<RegisteredPackage>> = const { RefCell::new(None) };
    static STATUS: RefCell<Vec<u8>> = RefCell::new(b"{\"code\":\"unselected\"}".to_vec());
}
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

pub(crate) const fn repair_snapshot_max_packet() -> usize { repair_snapshot::MAX_PACKET }

fn metadata(selection: &PreparedSelection) -> Value {
    let mut value = json!({"code":"prepared", "id":selection.id, "gamedataOwner":selection.gamedata_owner,
        "bootstrapSha256":selection.bootstrap_sha256,"gamedataSha256":selection.gamedata_sha256,
        "manifestPath":selection.provenance.manifest_path,
        "target":{"engine":selection.provenance.engine,"game":selection.provenance.game,
            "platform":selection.provenance.platform}});
    if let Some(functions) = &selection.functions {
        value["functionsSha256"] = json!(functions.sha256);
        value["functionsBundleHash"] = json!(functions.bundle_hash);
        value["functionsPath"] = json!(selection.provenance.functions_path);
    }
    if let Some(trusted) = &selection.trusted {
        value["trustedFunctionsSha256"] = json!(trusted.sha256);
        value["trustedFunctionsPath"] = json!(selection.provenance.trusted_path);
    }
    value
}
fn json_storage(value: &Value) -> usize {
    match value {
        Value::String(s) => s.capacity(),
        Value::Array(items) => items.capacity() * std::mem::size_of::<Value>()
            + items.iter().map(json_storage).sum::<usize>(),
        Value::Object(entries) => entries.iter().map(|(key, value)|
            key.capacity() + std::mem::size_of::<(String, Value)>() + json_storage(value)).sum(),
        _ => 0,
    }
}
fn selection_storage(selection: &PreparedSelection) -> usize {
    let path = |p: &Path| p.as_os_str().len();
    std::mem::size_of::<PreparedSelection>()
        + selection.id.capacity() + selection.gamedata_owner.capacity()
        + selection.bootstrap_bytes.capacity() + selection.gamedata_bytes.capacity()
        + selection.bootstrap_sha256.capacity() + selection.gamedata_sha256.capacity()
        + selection.provenance.engine.capacity() + selection.provenance.game.capacity()
        + selection.provenance.platform.capacity()
        + path(&selection.provenance.manifest_path)
        + path(&selection.provenance.bootstrap_path)
        + path(&selection.provenance.gamedata_path)
        + selection.provenance.functions_path.as_deref().map_or(0, path)
        + selection.provenance.trusted_path.as_deref().map_or(0, path)
        + selection.trusted.as_ref().map_or(0, |t| std::mem::size_of_val(t) + t.bytes.capacity() + t.sha256.capacity())
        + selection.functions.as_ref().map_or(0, |functions| {
            std::mem::size_of_val(functions) + functions.bytes.capacity()
                + functions.sha256.capacity() + functions.bundle_hash.capacity()
                + json_storage(&functions.summary)
                + functions.permissions.capacity() * std::mem::size_of::<String>()
                + functions.permissions.iter().map(String::capacity).sum::<usize>()
        })
}
pub(crate) fn report_error(error: &str) {
    STATUS.with(|s| {
        *s.borrow_mut() = json!({"code":"failed","error":error})
            .to_string()
            .into_bytes()
    });
}
pub(crate) fn report_selection_failure(handle: u64, error: &str) -> Result<(), String> {
    if error.is_empty() || error.len() > 4096 || error.contains('\0') {
        return Err("invalid selection failure reason".into());
    }
    let current = PENDING.with(|p| p.borrow().as_ref().is_some_and(|p| p.handle == handle));
    if !current { return Err("stale selection handle".into()); }
    let already_failed = STATUS.with(|s| serde_json::from_slice::<Value>(&s.borrow()).ok()
        .and_then(|v| v["code"].as_str().map(str::to_owned)).as_deref() == Some("failed"));
    if !already_failed { report_error(error); }
    Ok(())
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
    let total = selection.bootstrap_bytes.len()
        .checked_add(selection.gamedata_bytes.len())
        .and_then(|n| n.checked_add(selection.functions.as_ref().map_or(0, |f| f.bytes.len())))
        .and_then(|n| n.checked_add(selection.trusted.as_ref().map_or(0, |t| t.bytes.len())))
        .ok_or("package aggregate size limit")?;
    if selection.bootstrap_bytes.is_empty()
        || selection.bootstrap_bytes.len() > 16 * 1024 * 1024
        || selection.gamedata_bytes.len() > 4 * 1024 * 1024
        || selection.functions.as_ref().is_some_and(|f| f.bytes.len() > 4 * 1024 * 1024)
        || selection.trusted.as_ref().is_some_and(|t| t.bytes.len() > 4 * 1024 * 1024)
        || total > 24 * 1024 * 1024
    {
        return Err("package artifact size limit".into());
    }
    let (overrides, preparation_bytes, snapshot_bytes) = if let Some(functions) = &selection.functions {
        let snapshot = overrides::snapshot(&selection.id)
            .map_err(|e| format!("{}: override snapshot: {e}", selection.id))?;
        let snapshot_bytes = snapshot.retained_bytes()
            .map_err(|e| format!("{}: override snapshot: {e}", selection.id))?;
        let source = std::str::from_utf8(&functions.bytes).map_err(|_| "invalid function UTF-8")?;
        let bundle = contract::parse(source, &selection.id, &functions.summary, &functions.permissions)?;
        let candidate = overrides::prepare(bundle, &functions.sha256, snapshot.clone())
            .map_err(|e| format!("{}: override preparation: {e}", selection.id))?;
        let weight = registry::preparation_bytes(&candidate);
        (Some(snapshot), weight, snapshot_bytes)
    } else {
        (None, 0, 0)
    };
    // A trusted source is materialized (targets and offsets chosen) only at commit, so reserve a
    // conservative multiple of its sealed bytes for the decoded artifact and prepared receipt.
    let preparation_bytes = preparation_bytes.saturating_add(selection.trusted.as_ref()
        .map_or(0, |t| t.bytes.len().saturating_mul(32).saturating_add(256 * 1024)));
    // Commit temporarily clones selection and function overrides. GCR1 adds one inbound
    // packet, its decoded owned bytes/effects, and bounded status serialization. Reserve all
    // of that at selection so a failed admission cannot publish any owner or source.
    let prepared_status = metadata(&selection).to_string();
    let charged = selection_storage(&selection).saturating_mul(2)
        .saturating_add(selection.bootstrap_bytes.len())
        .saturating_add(prepared_status.len().saturating_mul(2))
        .saturating_add(snapshot_bytes.saturating_mul(2))
        .saturating_add(preparation_bytes)
        .saturating_add(4 * 1024 * 1024 + 4096)
        .saturating_add(repair_snapshot::MAX_PACKET.saturating_mul(3))
        .saturating_add(repair_snapshot::MAX_PARSE_TEMP)
        .saturating_add(2 * 1024 * 1024);
    let retention = crate::loader::retain_game_package(charged)
        .ok_or("game package retained-byte admission unavailable")?;
    let handle = NEXT_HANDLE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| "selection handles exhausted")?;
    STATUS.with(|s| *s.borrow_mut() = prepared_status.into_bytes());
    PENDING.with(|p| *p.borrow_mut() = Some(PendingPackage {
        handle, selection, overrides, retention: Rc::new(RefCell::new(retention)), preparation_bytes,
    }));
    Ok(handle)
}
/// Returns owned bytes from the retained snapshot, never paths to reopen.
pub(crate) fn copy(handle: u64, member: u32) -> Result<Vec<u8>, String> {
    PENDING.with(|p| {
        let p = p.borrow();
        let pending = p
            .as_ref()
            .filter(|p| p.handle == handle)
            .ok_or("stale selection handle")?;
        let selection = &pending.selection;
        match member {
            0 => Ok(selection.gamedata_bytes.clone()),
            1 => Ok(metadata(selection).to_string().into_bytes()),
            3 => selection.functions.as_ref().map(|f| f.bytes.clone()).ok_or("no function artifact".into()),
            _ => Err("unknown selection member".into()),
        }
    })
}
pub(crate) fn abort(handle: u64) -> Result<(), String> {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().is_none_or(|p| p.handle != handle) {
            return Err("stale selection handle".into());
        }
        p.take();
        STATUS.with(|s| {
            let mut status = s.borrow_mut();
            if let Ok(mut value) = serde_json::from_slice::<Value>(&status) {
                if value["code"] == "prepared" {
                    value["code"] = json!("aborted");
                    *status = value.to_string().into_bytes();
                }
            }
        });
        Ok(())
    })
}
pub(crate) fn commit(handle: u64, merged: &str, repair_bytes: &[u8]) -> Result<(), String> {
    if REGISTERED.with(|r| r.borrow().is_some()) {
        return Err("package already active".into());
    }
    let (selection, override_snapshot, retention, reserved) = PENDING
        .with(|p| {
            p.borrow()
                .as_ref()
                .filter(|p| p.handle == handle)
                .map(|p| (p.selection.clone(), p.overrides.clone(), p.retention.clone(), p.preparation_bytes))
        })
        .ok_or("stale selection handle")?;
    if merged.len() > 4 * 1024 * 1024 || repair_bytes.len() > repair_snapshot::MAX_PACKET {
        return Err("merged package size limit".into());
    }
    let repairs = repair_snapshot::decode(repair_bytes)?;
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
    // Trusted functions: fill each target from THIS merged view (shipped + operator custom) and
    // select record offsets from the live schema, before any owner or source becomes visible.
    let materialized = match &selection.trusted {
        Some(t) => Some(trusted_source::materialize(&t.bytes, &selection.id, &gd, &repairs,
            &|class, field| crate::v8host::schema_offset_cached(class, field))?),
        None => None,
    };
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
    info["customPaths"] = json!(&repairs.custom_paths);
    info["operatorRepairs"] = repairs.status();
    if let Some(m) = &materialized {
        info["trustedFunctions"] = m.status.clone();
    }
    if info.to_string().len() > 2 * 1024 * 1024 || repairs.retained_bytes() > repair_snapshot::MAX_PACKET * 2 {
        return Err("repair snapshot retention/status size limit".into());
    }
    // No public owner/source before every fallible preparation succeeds. Resolution failures are
    // descriptor-level unavailable entries. Hook reservations roll back if staging is dropped.
    let calls = crate::gamedata_calls::prepare_game_package(&owner, &gd);
    let hooks = crate::gamedata_hooks::prepare_game_package(&owner, &gd);
    let public = if let Some(functions) = &selection.functions {
        let snapshot = override_snapshot.ok_or("missing retained function override snapshot")?;
        let text = std::str::from_utf8(&functions.bytes).map_err(|_| "invalid function UTF-8")?;
        let bundle = contract::parse(text, &selection.id, &functions.summary, &functions.permissions)?;
        let candidate = overrides::prepare(bundle, &functions.sha256, snapshot)
            .map_err(|e| format!("{}: retained override preparation: {e}", selection.id))?;
        if registry::preparation_bytes(&candidate) > reserved {
            return Err("function preparation exceeded retained-byte admission".into());
        }
        Some(candidate)
    } else {
        None
    };
    let (receipt, functions) = if let Some(materialized) = materialized {
        // SAFETY: the trusted artifact's record positions are the verified native target's
        // synchronous arguments (see games/cs2/trusted-functions.jsonc); the host, not the JSON,
        // owns this promise. Its live proof across every engine callsite is recorded as an open
        // acceptance item, not established by this code.
        let lifetime = unsafe { SynchronousRecordLifetime::registered_native_target() };
        let activation = trusted::activate_selected_trusted(&authority, source.into(), hash, &materialized.bytes,
            public, lifetime, trusted::SelectedRetention { lease: retention.clone(), reserved_bytes: reserved })
            .map_err(|e| format!("{}: {e}", selection.id))?;
        // Each declared function's binding state: "available" or its named degrade reason.
        if let Some(rows) = info["trustedFunctions"]["functions"].as_array_mut() {
            for row in rows {
                let name = row["localName"].as_str().unwrap_or_default().to_owned();
                row["binding"] = match registry::named_binding(authority.key(), &name) {
                    Ok(b) if b.target.is_some() => json!("available"),
                    Ok(b) => json!(b.unavailable.clone().unwrap_or_else(|| "unavailable".into())),
                    Err(e) => json!(e),
                };
            }
        }
        (activation.source, Some(activation.functions))
    } else {
        let prepared_functions = public.map(|candidate| {
            let mut receipt = registry::prepare_package_owner(&authority, candidate)?;
            if receipt.retained_bytes() > reserved {
                return Err::<_, String>("function receipt exceeded retained-byte admission".into());
            }
            receipt.retain(retention.clone());
            Ok(receipt)
        }).transpose()?;
        let receipt = function_adapter::register_selected_package(authority.clone(), source.into(), hash)?;
        let functions = prepared_functions.map(|prepared| registry::activate_package_owner(prepared, &authority)).transpose()?;
        (receipt, functions)
    };
    let status = info.to_string().into_bytes();
    crate::gamedata_calls::commit_game_package(&owner, calls);
    crate::gamedata_hooks::commit_game_package(hooks);
    REGISTERED.with(|r| {
        *r.borrow_mut() = Some(RegisteredPackage {
            selection,
            _functions: functions,
            _receipt: receipt,
            _retention: retention.clone(),
            _merged_data: merged.into(),
            _repair_snapshot: repairs,
            status,
        })
    });
    PENDING.with(|p| p.borrow_mut().take());
    retention.borrow_mut().activate();
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

//! Exact generation identities and host-owned resources for function dispatch.
#[cfg(test)]
mod tests {
    use super::super::contract::{
        HostPackageOwner, ImplementationManifestHash, OwnerKey, OwnerKind,
    };
    #[test]
    fn package_authority_is_host_minted_and_manifest_hash_is_validated() {
        let a = HostPackageOwner::mint("@test/package").unwrap();
        let b = HostPackageOwner::mint("@test/package").unwrap();
        assert_ne!(a.key(), b.key());
        assert_eq!(a.key().kind, OwnerKind::GamePackage);
        assert!(ImplementationManifestHash::new("a".repeat(64)).is_ok());
        assert!(ImplementationManifestHash::new("A".repeat(64)).is_err());
        assert!(ImplementationManifestHash::new("source-not-manifest".into()).is_err());
        let old = OwnerKey::plugin("owner-a", 1);
        assert_ne!(old, OwnerKey::plugin("owner-a", 2));
    }
}

use super::{contract::*, provenance::PreparedCandidate, runtime};
use crate::{plugin::Resource, v8host};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
pub(crate) struct Binding {
    pub id: u64,
    pub owner: OwnerKey,
    pub function: NormalizedFunction,
    pub target: Option<i64>,
    pub unavailable: Option<String>,
    pub provenance: super::provenance::Provenance,
    pub retained_bytes: usize,
    _retention: Option<Rc<dyn std::any::Any>>,
}
impl Drop for Binding {
    fn drop(&mut self) {
        if let Some(target) = self.target {
            runtime::target_release(target);
        }
    }
}
thread_local! {
    static BINDINGS:RefCell<BTreeMap<u64,Rc<Binding>>>=const{RefCell::new(BTreeMap::new())};
    static NEXT:std::cell::Cell<u64>=const{std::cell::Cell::new(1)};
}
pub(crate) fn next_id() -> Result<u64, String> {
    NEXT.with(|n| {
        let id = n.get();
        n.set(id.checked_add(1).ok_or("function id exhausted")?);
        Ok(id)
    })
}
struct PreparedBinding {
    id: u64,
    function: NormalizedFunction,
    target: Option<i64>,
    unavailable: Option<String>,
    provenance: super::provenance::Provenance,
    retained_bytes: usize,
}
impl Drop for PreparedBinding {
    fn drop(&mut self) {
        if let Some(target) = self.target {
            runtime::target_release(target);
        }
    }
}
pub(crate) struct PreparedOwnerReceipt {
    intended_id: String,
    bindings: Vec<PreparedBinding>,
    retention: Option<Rc<dyn std::any::Any>>,
}
impl PreparedOwnerReceipt {
    pub(crate) fn retain(&mut self, lease: Rc<dyn std::any::Any>) {
        self.retention = Some(lease);
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.intended_id.capacity()
            + self.bindings.capacity() * std::mem::size_of::<PreparedBinding>()
            + self
                .bindings
                .iter()
                .map(|b| b.retained_bytes)
                .sum::<usize>()
    }
}
/// A conservative admission weight for decoded snapshots and native preparation receipts.
/// Reserve before acquiring targets. The 8x payload weight covers simultaneous decoded copies
/// and escaped native argument serialization; 4 KiB per function reserves container/receipt
/// overhead. This is admission accounting, not telemetry from the foreign allocator.
/// Charge both temporary decoded copies and retained bindings;
/// the shared loader lease outlives activation and is released by the last binding.
pub(crate) fn preparation_bytes(candidate: &PreparedCandidate) -> usize {
    candidate.functions().iter().fold(
        std::mem::size_of::<PreparedOwnerReceipt>() + candidate.base().owner_id.len(),
        |n, f| {
            n.saturating_add(
                4096 + 8
                    * (std::mem::size_of::<Binding>()
                        + candidate.base().owner_id.len()
                        + function_storage(f.function())
                        + provenance_storage(f.provenance())
                        + f.unavailable().map_or(512, str::len)),
            )
        },
    )
}
/// Native preparation grants no live owner or JS capability. Dropping it releases only candidates.
pub(crate) fn prepare_owner(
    intended_id: &str,
    candidate: PreparedCandidate,
) -> Result<PreparedOwnerReceipt, String> {
    prepare_owner_with_grants(intended_id, candidate, true, true)
}
pub(crate) fn prepare_owner_with_grants(
    intended_id: &str,
    candidate: PreparedCandidate,
    calls_allowed: bool,
    hooks_allowed: bool,
) -> Result<PreparedOwnerReceipt, String> {
    if candidate.base().owner_id != intended_id {
        return Err("prepared candidate owner mismatch".into());
    }
    let mut bindings = Vec::with_capacity(candidate.functions().len());
    for prepared in candidate.functions() {
        let f = prepared.function();
        if f.policy.id != "generic.v2" {
            return Err("internal policy requires host adapter binding authority".into());
        }
        let binding_id = next_id()?;
        let denied = f
            .policy
            .surfaces
            .iter()
            .find_map(|surface| match surface.as_str() {
                "call" if !calls_allowed => Some("operator permission denied: engine:calls"),
                "pre" | "post" if !hooks_allowed => {
                    Some("operator permission denied: engine:hooks")
                }
                _ => None,
            });
        let resolved = denied
            .or(prepared.unavailable())
            .map_or_else(|| runtime::prepare(f), |e| Err(e.into()));
        let (target, unavailable) = match resolved {
            Ok(id) => (Some(id), None),
            Err(e) if f.requirement == "required" => {
                return Err(format!("{}: {e}", f.canonical_id));
            }
            Err(e) => (None, Some(e)),
        };
        let function = f.clone();
        let provenance = prepared.provenance().clone();
        let retained_bytes = std::mem::size_of::<Binding>()
            + intended_id.len()
            + function_storage(&function)
            + provenance_storage(&provenance)
            + unavailable.as_ref().map_or(0, String::capacity);
        bindings.push(PreparedBinding {
            id: binding_id,
            function,
            target,
            unavailable,
            provenance,
            retained_bytes,
        });
    }
    Ok(PreparedOwnerReceipt {
        intended_id: intended_id.into(),
        bindings,
        retention: None,
    })
}
#[cfg(test)]
thread_local! { pub(crate) static ACTIVATION_LEDGER_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; }
fn record_activation(owner: &OwnerKey, id: u64) -> bool {
    #[cfg(test)]
    if !ACTIVATION_LEDGER_LIMIT.with(|limit| match limit.get() {
        Some(0) => {
            limit.set(None);
            false
        }
        Some(n) => {
            limit.set(Some(n - 1));
            true
        }
        None => true,
    }) {
        return false;
    }
    v8host::record_resource(&owner.id, owner.generation, Resource::FunctionBinding(id))
}
pub(crate) fn activate_owner(
    receipt: PreparedOwnerReceipt,
    owner: OwnerKey,
) -> Result<Vec<u64>, String> {
    if owner.kind != OwnerKind::Plugin || receipt.intended_id != owner.id {
        return Err("prepared candidate owner mismatch".into());
    }
    if !v8host::owner_is_live(&owner.id, owner.generation) {
        return Err("owner generation unavailable".into());
    }
    if BINDINGS.with(|b| b.borrow().values().any(|b| b.owner == owner)) {
        return Err("owner already activated".into());
    }
    // Stage the entire ledger before publishing any binding. Roll back in reverse acquisition order.
    let mut ids = Vec::new();
    for binding in &receipt.bindings {
        if !record_activation(&owner, binding.id) {
            for id in ids.iter().rev() {
                v8host::release_resource(
                    &owner.id,
                    owner.generation,
                    &Resource::FunctionBinding(*id),
                );
            }
            return Err("owner ledger unavailable".into());
        }
        ids.push(binding.id);
    }
    for mut prepared in receipt.bindings {
        let binding = Rc::new(Binding {
            id: prepared.id,
            owner: owner.clone(),
            function: prepared.function.clone(),
            target: prepared.target.take(),
            unavailable: prepared.unavailable.take(),
            provenance: prepared.provenance.clone(),
            retained_bytes: prepared.retained_bytes,
            _retention: receipt.retention.clone(),
        });
        BINDINGS.with(|b| b.borrow_mut().insert(binding.id, binding));
    }
    Ok(ids)
}
pub(crate) fn named_binding(owner: &OwnerKey, name: &str) -> Result<Rc<Binding>, String> {
    let id = BINDINGS
        .with(|b| {
            b.borrow()
                .values()
                .find(|b| b.owner == *owner && b.function.local_name == name)
                .map(|b| b.id)
        })
        .ok_or("undeclared engine function")?;
    binding(id, owner)
}
pub(crate) fn binding(id: u64, owner: &OwnerKey) -> Result<Rc<Binding>, String> {
    BINDINGS
        .with(|b| {
            b.borrow()
                .get(&id)
                .filter(|b| b.owner == *owner && v8host::owner_is_live(&owner.id, owner.generation))
                .cloned()
        })
        .ok_or("binding owner generation unavailable".into())
}
pub(crate) fn drop_binding(id: u64) {
    let removed = BINDINGS.with(|b| b.borrow_mut().remove(&id));
    drop(removed);
}
pub(crate) fn owner_bindings(owner: &OwnerKey) -> Vec<u64> {
    BINDINGS.with(|b| {
        b.borrow()
            .values()
            .filter(|b| b.owner == *owner)
            .map(|b| b.id)
            .collect()
    })
}

// Measure retained decoded payload allocations. Admission additionally reserves
// temporary preparation storage and container/receipt overhead at the loader seam.
fn strings(values: &Vec<String>) -> usize {
    values.capacity() * std::mem::size_of::<String>()
        + values.iter().map(String::capacity).sum::<usize>()
}
fn validator_storage(v: &Validator) -> usize {
    v.prologue.as_ref().map_or(0, String::capacity)
        + v.vtable_member.as_ref().map_or(0, String::capacity)
        + v.string_xref.as_ref().map_or(0, |x| x.expect.capacity())
}
fn function_storage(f: &NormalizedFunction) -> usize {
    let a = &f.abi;
    let p = &f.policy;
    let target = match &f.target {
        NormalizedTarget::Signature {
            module,
            pattern,
            resolve,
            derivation,
            candidate_validate,
            target_validate,
        } => {
            module.capacity()
                + pattern.capacity()
                + resolve.capacity()
                + derivation.capacity()
                + validator_storage(candidate_validate)
                + validator_storage(target_validate)
        }
        NormalizedTarget::Virtual {
            module,
            class,
            resolve,
            derivation,
            candidate_validate,
            target_validate,
            ..
        } => {
            module.capacity()
                + class.capacity()
                + resolve.capacity()
                + derivation.capacity()
                + validator_storage(candidate_validate)
                + validator_storage(target_validate)
        }
    };
    f.local_name.capacity()
        + f.canonical_id.capacity()
        + f.contract_hash.capacity()
        + f.requirement.capacity()
        + target
        + a.platform.capacity()
        + a.receiver.capacity()
        + a.fingerprint.capacity()
        + a.returns.native.capacity()
        + a.returns.projection.id.capacity()
        + a.parameters.capacity() * std::mem::size_of::<Parameter>()
        + a.parameters
            .iter()
            .map(|p| {
                p.name.capacity()
                    + p.native.capacity()
                    + p.projection.id.capacity()
                    + strings(&p.mutable)
            })
            .sum::<usize>()
        + p.id.capacity()
        + p.contract_hash.capacity()
        + p.self_call.capacity()
        + p.suppression.capacity()
        + strings(&p.surfaces)
}
fn provenance_storage(p: &super::provenance::Provenance) -> usize {
    use super::provenance::ValidationResult;
    let result = |r: &ValidationResult| match r {
        ValidationResult::Pending => 0,
        ValidationResult::Unavailable(s) => s.capacity(),
    };
    p.archive_hash.capacity()
        + p.base_contract_hash.capacity()
        + p.final_target_hash.capacity()
        + result(&p.resolver_result)
        + result(&p.validator_result)
        + p.overrides.capacity() * std::mem::size_of::<super::provenance::OverrideProvenance>()
        + p.overrides
            .iter()
            .map(|o| o.relative_path.capacity() + o.sha256.capacity())
            .sum::<usize>()
}
#[allow(dead_code)] // Diagnostic retained payload total; shared loader lease also reserves preparation work.
pub(crate) fn retained_bytes(owner: &OwnerKey) -> usize {
    BINDINGS.with(|b| {
        b.borrow()
            .values()
            .filter(|b| b.owner == *owner)
            .map(|b| b.retained_bytes)
            .sum()
    })
}

pub(crate) fn owner_for_token(token: u64) -> Option<OwnerKey> {
    BINDINGS.with(|b| {
        b.borrow()
            .values()
            .find(|b| b.owner.generation == token)
            .map(|b| b.owner.clone())
    })
}

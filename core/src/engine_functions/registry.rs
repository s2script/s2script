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
    #[allow(dead_code)] // Retained for Task 7's public availability projection.
    pub unavailable: Option<String>,
    #[allow(dead_code)] // Immutable Task 4 provenance retained for later public diagnostics.
    pub provenance: super::provenance::Provenance,
    #[allow(dead_code)] // Payload measurement; Task 7 supplies the loader-budget consumer.
    pub retained_bytes: usize,
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
pub(crate) struct PreparedOwnerReceipt {
    owner: OwnerKey,
    bindings: Vec<Rc<Binding>>,
}
/// Consumes the immutable Task 4 snapshot; it never rereads overrides or archive bytes.
/// This accounts owned decoded receipt storage, not the later loader/worker budget.
pub(crate) fn prepare_owner(
    owner: OwnerKey,
    candidate: PreparedCandidate,
) -> Result<PreparedOwnerReceipt, String> {
    if owner.kind != OwnerKind::Plugin || !v8host::owner_is_live(&owner.id, owner.generation) {
        return Err("owner generation unavailable".into());
    }
    if candidate.base().owner_id != owner.id {
        return Err("prepared candidate owner mismatch".into());
    }
    let mut bindings = Vec::new();
    for prepared in candidate.functions() {
        let f = prepared.function();
        // Host adapter binding authorization is explicit and separate from archive parsing.
        if f.policy.id != "generic.v2" {
            return Err("internal policy requires host adapter binding authority".into());
        }
        let binding_id = next_id()?;
        let resolved = prepared
            .unavailable()
            .map_or_else(|| runtime::prepare(f), |e| Err(e.into()));
        let (target, unavailable) = match resolved {
            Ok(id) => (Some(id), None),
            Err(e) if f.requirement == "required" => return Err(e),
            Err(e) => (None, Some(e)),
        };
        let function = f.clone();
        let provenance = prepared.provenance().clone();
        let retained_bytes = std::mem::size_of::<Binding>()
            + owner.id.len()
            + function_storage(&function)
            + provenance_storage(&provenance)
            + unavailable.as_ref().map_or(0, String::capacity);
        bindings.push(Rc::new(Binding {
            id: binding_id,
            owner: owner.clone(),
            function,
            target,
            unavailable,
            provenance,
            retained_bytes,
        }));
    }
    Ok(PreparedOwnerReceipt { owner, bindings })
}
pub(crate) fn activate_owner(receipt: PreparedOwnerReceipt) -> Result<Vec<u64>, String> {
    if !v8host::owner_is_live(&receipt.owner.id, receipt.owner.generation) {
        return Err("owner generation unavailable".into());
    }
    if BINDINGS.with(|b| b.borrow().values().any(|b| b.owner == receipt.owner)) {
        return Err("owner already activated".into());
    }
    let ids = receipt.bindings.iter().map(|b| b.id).collect();
    for binding in receipt.bindings {
        if !v8host::record_resource(
            &binding.owner.id,
            binding.owner.generation,
            Resource::FunctionBinding(binding.id),
        ) {
            return Err("owner ledger unavailable".into());
        }
        BINDINGS.with(|b| b.borrow_mut().insert(binding.id, binding));
    }
    Ok(ids)
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

// Charge retained decoded payload allocations at the owning receipt. Container
// allocator overhead and the loader worker's budget are accounted at Task 7's
// activation seam; this is deliberately not a claim of complete loader budgeting.
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
#[allow(dead_code)] // Task 7 activation consumes this measurement, not yet a loader budget charge.
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

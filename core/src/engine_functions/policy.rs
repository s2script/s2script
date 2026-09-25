//! Semantic domains and subscription rights, independent from native ABI layout.
use super::contract::Policy;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdapterContract {
    pub id: String,
    pub version: u32,
    pub contract_hash: String,
}
impl AdapterContract {
    pub(crate) fn generic(policy: &Policy) -> Result<Self, String> {
        if policy.id != "generic.v2" || policy.version != 1 {
            return Err("public generic adapter required".into());
        }
        Ok(Self {
            id: policy.id.clone(),
            version: policy.version,
            contract_hash: policy.contract_hash.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubscriptionMode {
    Mutating,
    Observe,
}
impl SubscriptionMode {
    pub(crate) fn for_phase(phase: i32, observe_only: bool) -> Result<Self, String> {
        match phase {
            0 if !observe_only => Ok(Self::Mutating),
            0 | 1 => Ok(Self::Observe),
            _ => Err("invalid subscription phase".into()),
        }
    }
    pub(crate) fn writable(self) -> bool {
        self == Self::Mutating
    }
}

/// Public executable registrations. Registration resolution happens before
/// dispatch; callback execution never selects behavior by a semantic ID.
pub(crate) fn public_adapter(
    policy: &Policy,
) -> Result<&'static dyn super::package_adapter::DispatchAdapter, String> {
    static GENERIC: super::package_adapter::GenericAdapter = super::package_adapter::GenericAdapter;
    let implementations: [(&str, u32, &dyn super::package_adapter::DispatchAdapter); 1] =
        [("generic.v2", 1, &GENERIC)];
    implementations
        .into_iter()
        .find(|(id, version, _)| *id == policy.id && *version == policy.version)
        .map(|(_, _, implementation)| implementation)
        .ok_or_else(|| "public executable adapter unavailable".into())
}

/// A host capability; neither contract strings nor package visibility grant effects.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PostReturnAuthority {
    #[default]
    None,
    Override,
}
pub(crate) struct HostAdapterGrant {
    owner: super::contract::OwnerKey,
    contract: AdapterContract,
    authority: PostReturnAuthority,
}
impl HostAdapterGrant {
    pub(crate) fn override_return(
        owner: &super::contract::HostPackageOwner,
        contract: AdapterContract,
    ) -> Result<Self, String> {
        if contract.id.is_empty() || contract.id == "generic.v2" || contract.version != 1 {
            return Err("invalid host adapter contract".into());
        }
        super::contract::ImplementationManifestHash::new(contract.contract_hash.clone())?;
        Ok(Self {
            owner: owner.key().clone(),
            contract,
            authority: PostReturnAuthority::Override,
        })
    }
    pub(crate) fn belongs_to(&self, owner: &super::contract::HostPackageOwner) -> bool {
        self.owner == *owner.key()
    }
    pub(crate) fn authority(&self, contract: &AdapterContract) -> PostReturnAuthority {
        if self.contract == *contract {
            self.authority
        } else {
            PostReturnAuthority::None
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.owner.id.len() + self.contract.id.len() + self.contract.contract_hash.len()
    }
}

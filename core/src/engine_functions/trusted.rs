//! Host-verified trusted function artifacts (first-party game packages only).
//! Community archives never reach this module: `contract::parse` stays the only
//! public grammar. Everything here is engine-generic; adapter ids and contract
//! hashes are opaque package facts, never interpreted by the core.
use super::contract::{self, HostPackageOwner, ImplementationManifestHash, NormalizedTarget, Policy};
use super::instance::{self, SelectedLayoutData, Signature, SynchronousRecordLifetime, TrustedFunctionInput};
use super::policy::{AdapterContract, HostAdapterGrant};
use super::provenance::PreparedCandidate;
use super::registry;
use crate::v8host::function_adapter::{self, PreparedPackageReceipt};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_FUNCTIONS: usize = 256;
const MAX_NAME: usize = 128;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactWire {
    schema_version: u32,
    owner_id: String,
    /// Exact selected offset-key -> u32 JSON text; hashed before it is decoded.
    selected_offsets: String,
    selected_offsets_sha256: String,
    functions: Vec<FunctionWire>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FunctionWire {
    local_name: String,
    requirement: String,
    target: NormalizedTarget,
    signature: Signature,
    policy: Policy,
    #[serde(default, deserialize_with = "contract::present")]
    adapter: Option<AdapterWire>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdapterWire {
    id: String,
    contract_hash: String,
    post_override: bool,
}

/// One declared function's host binding authorization.
pub(crate) struct TrustedAdapterBinding {
    pub local_name: String,
    pub contract: AdapterContract,
    pub post_override: bool,
}
pub(crate) struct TrustedArtifact {
    pub inputs: Vec<TrustedFunctionInput>,
    pub selected: SelectedLayoutData,
    pub adapters: Vec<TrustedAdapterBinding>,
}

fn fail(why: impl std::fmt::Display) -> String {
    format!("trusted functions artifact: {why}")
}

/// Strict decoder: unknown fields, duplicate keys, oversized input, owner mismatch,
/// selected-data hash mismatch and inconsistent adapter declarations are named errors.
/// Signature/layout semantics are validated later by `prepare_verified_package`.
pub(crate) fn decode(bytes: &[u8], owner_id: &str) -> Result<TrustedArtifact, String> {
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(fail("byte limit exceeded"));
    }
    let wire: ArtifactWire = serde_json::from_slice(bytes).map_err(fail)?;
    if wire.schema_version != 1 {
        return Err(fail("unsupported schemaVersion"));
    }
    if wire.owner_id != owner_id || owner_id.is_empty() {
        return Err(fail("ownerId does not match the verified package"));
    }
    if wire.functions.len() > MAX_FUNCTIONS {
        return Err(fail("function limit exceeded"));
    }
    ImplementationManifestHash::new(wire.selected_offsets_sha256.clone())
        .map_err(|_| fail("selectedOffsetsSha256 must be lowercase SHA256"))?;
    if contract::hash_bytes(wire.selected_offsets.as_bytes()) != wire.selected_offsets_sha256 {
        return Err(fail("selectedOffsets hash mismatch"));
    }
    let mut contracts: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut inputs = Vec::with_capacity(wire.functions.len());
    let mut adapters = Vec::new();
    for f in wire.functions {
        if f.local_name.len() > MAX_NAME {
            return Err(fail("localName too long"));
        }
        if let Some(a) = f.adapter {
            if a.id.is_empty() || a.id.len() > MAX_NAME || a.id.contains('\0') || a.id == "generic.v2" {
                return Err(fail(format!("{}: invalid adapter id", f.local_name)));
            }
            ImplementationManifestHash::new(a.contract_hash.clone())
                .map_err(|_| fail(format!("{}: adapter contractHash must be lowercase SHA256", f.local_name)))?;
            let prior = contracts.entry(a.id.clone()).or_insert((a.contract_hash.clone(), a.post_override));
            if *prior != (a.contract_hash.clone(), a.post_override) {
                return Err(fail(format!("adapter {} declared with conflicting contractHash/postOverride", a.id)));
            }
            adapters.push(TrustedAdapterBinding {
                local_name: f.local_name.clone(),
                contract: AdapterContract { id: a.id, version: 1, contract_hash: a.contract_hash },
                post_override: a.post_override,
            });
        }
        inputs.push(TrustedFunctionInput {
            local_name: f.local_name,
            target: f.target,
            signature: f.signature,
            policy: f.policy,
            requirement: f.requirement,
        });
    }
    Ok(TrustedArtifact {
        inputs,
        selected: SelectedLayoutData {
            sha256: wire.selected_offsets_sha256,
            bytes: wire.selected_offsets.into(),
        },
        adapters,
    })
}

/// Both halves of an activated trusted package. Dropping it retires the functions
/// (first) and then the package source authority.
pub(crate) struct TrustedPackageActivation {
    pub(crate) functions: registry::ActivePackageFunctions,
    pub(crate) source: PreparedPackageReceipt,
}

/// Activates a host-verified package's trusted functions artifact.
///
/// The caller (S3 `game_packages::commit`) has already verified the package, its
/// `source`, its manifest hash and the artifact bytes; `owner` is a freshly minted,
/// never-registered `HostPackageOwner`. In order this: decodes the artifact; issues a
/// POST override grant for each adapter contract declared `postOverride`; registers
/// the package source with those grants; prepares the trusted candidate; merges it
/// with the package's optional `public` v2 archive candidate; prepares and activates
/// the package owner's bindings; and authorizes each declared function's available
/// binding for its adapter id + contract hash. Any failure leaves nothing registered
/// and burns `owner` (mint a new one to retry).
pub(crate) fn activate_trusted(
    owner: &HostPackageOwner,
    source: Arc<str>,
    manifest: ImplementationManifestHash,
    artifact: &[u8],
    public: Option<PreparedCandidate>,
    lifetime: SynchronousRecordLifetime,
) -> Result<TrustedPackageActivation, String> {
    let decoded = decode(artifact, &owner.key().id)?;
    let mut grants = Vec::new();
    let mut granted = std::collections::BTreeSet::new();
    for a in decoded.adapters.iter().filter(|a| a.post_override) {
        if granted.insert(a.contract.id.clone()) {
            grants.push(HostAdapterGrant::override_return(owner, a.contract.clone())?);
        }
    }
    let receipt =
        function_adapter::register_prepared_package_with_authorities(owner.clone(), source, manifest, grants)?;
    let candidate = instance::prepare_verified_package(&receipt, decoded.inputs, decoded.selected, lifetime)?;
    let merged = PreparedCandidate::merge_trusted(public, candidate)?;
    let prepared = registry::prepare_package_owner(owner, merged)?;
    let functions = registry::activate_package_owner(prepared, owner)?;
    for a in &decoded.adapters {
        let binding = registry::named_binding(owner.key(), &a.local_name)?;
        if binding.target.is_none() {
            continue; // Optional and unavailable: its named reason stays on the binding.
        }
        function_adapter::authorize_binding(&receipt, owner.key(), binding.id, &a.contract.id, &a.contract.contract_hash)
            .map_err(|e| format!("{}: {e}", binding.function.canonical_id))?;
    }
    Ok(TrustedPackageActivation { functions, source: receipt })
}

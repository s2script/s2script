//! Trusted normalized positions. Public archives enter only through contract::parse.
//! A layout/hash describes bytes; only the host lifetime grant authorizes access.
use super::contract::{self, *};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) const MAX_EXTENT: u32 = 65536;
pub(crate) const MAX_FIELDS: usize = 256;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordField {
    pub name: String,
    pub offset_key: String,
    pub offset: u32,
    pub storage: String,
    pub nullable: bool,
    pub read: Vec<String>,
    pub write: Vec<String>,
}
impl RecordField {
    pub(crate) fn width(&self) -> Result<u32, String> {
        match self.storage.as_str() {
            "bool" => Ok(1),
            "u16" => Ok(2),
            "i32" | "u32" | "f32" | "entity-handle32" => Ok(4),
            "i64" | "u64" | "f64" => Ok(8),
            _ => Err("unsupported record storage".into()),
        }
    }
    pub(crate) fn projection(&self) -> (&str, &str) {
        match self.storage.as_str() {
            "bool" => ("u8", "bool"),
            "u16" => ("u32", "u32"),
            "entity-handle32" => ("ptr", if self.nullable { "entity?" } else { "entity" }),
            s => (s, s),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordLayout {
    pub extent: u32,
    pub alignment: u32,
    pub fields: Vec<RecordField>,
}
impl RecordLayout {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.extent == 0
            || self.extent > MAX_EXTENT
            || !self.alignment.is_power_of_two()
            || self.alignment > 8
            || self.extent % self.alignment != 0
            || self.fields.is_empty()
            || self.fields.len() > MAX_FIELDS
        {
            return Err("invalid record extent/alignment/field count".into());
        }
        let mut names = BTreeSet::new();
        let mut spans = Vec::new();
        for f in &self.fields {
            if !contract::identifier(&f.name)
                || f.name.len() > 128
                || !names.insert(&f.name)
                || f.offset_key.is_empty()
                || f.offset_key.len() > 256
                || f.offset_key.contains('\0')
            {
                return Err("invalid record field identity".into());
            }
            let width = f.width()?;
            let end = f
                .offset
                .checked_add(width)
                .ok_or("record extent overflow")?;
            if end > self.extent {
                return Err("record field exceeds extent".into());
            }
            if self.alignment < width || f.offset % width != 0 {
                return Err("unaligned record field".into());
            }
            if spans
                .iter()
                .any(|&(start, finish)| f.offset < finish && start < end)
            {
                return Err("record field overlap".into());
            }
            spans.push((f.offset, end));
            if !matches!(f.read.as_slice(), [a] if a == "pre" || a == "post")
                && f.read != ["pre", "post"]
            {
                return Err("invalid record read phases".into());
            }
            if !(f.write.is_empty() || f.write == ["pre"])
                || (!f.write.is_empty() && !f.read.iter().any(|s| s == "pre"))
                || (f.nullable && f.storage != "entity-handle32")
            {
                return Err("invalid record field rights/nullability".into());
            }
        }
        Ok(())
    }
    pub(crate) fn hash(&self) -> String {
        contract::hash(&serde_json::to_value(self).unwrap())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Instance {
    pub codec_id: String,
    pub codec_version: u32,
    pub kind: Option<KindVersion>,
    pub layout_hash: String,
    pub record: RecordLayout,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KindVersion {
    pub id: String,
    pub version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Position {
    pub name: String,
    pub native: String,
    pub projection: Projection,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "contract::present")]
    pub ownership: Option<String>,
    pub mutable: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "contract::present")]
    pub instance: Option<usize>,
    pub nullable: bool,
}
/// Trusted-only per-dispatch host scalar. It starts at zero for each native PRE
/// invocation, is shared by every PRE callback of that dispatch, follows staged-edit
/// acceptance/rollback, and is never written to native arguments or committed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchSlot {
    pub name: String,
    pub storage: String,
}
pub(crate) const MAX_SCRATCH: usize = 16;
/// Frame names owned by the host facade; neither positions nor scratch may shadow them.
pub(crate) const RESERVED_FRAME_NAMES: [&str; 5] =
    ["returnValue", "skipped", "originalReturnValue", "overrideReturn", "proposedReturn"];
impl ScratchSlot {
    /// (native atom, projection id) of the scalar value; storage names match record fields.
    pub(crate) fn projection(&self) -> Result<(&'static str, &'static str), String> {
        Ok(match self.storage.as_str() {
            "bool" => ("u8", "bool"),
            "i32" => ("i32", "i32"),
            "u32" => ("u32", "u32"),
            "i64" => ("i64", "i64"),
            "u64" => ("u64", "u64"),
            "f32" => ("f32", "f32"),
            "f64" => ("f64", "f64"),
            _ => return Err("unsupported scratch storage".into()),
        })
    }
}
impl Position {
    pub(crate) fn hidden(&self) -> bool {
        self.projection.id == "native-only"
    }
    pub(crate) fn record(&self) -> bool {
        self.projection.id == "borrowed-record"
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Signature {
    pub platform: String,
    pub member_receiver: bool,
    pub receiver: Option<Position>,
    pub parameters: Vec<Position>,
    pub returns: Position,
    pub fingerprint: String,
    pub stack_copy_bytes: usize,
    pub instances: Vec<Instance>,
    /// Host-only; omitted from the serialized contract when empty so existing hashes hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scratch: Vec<ScratchSlot>,
}
#[derive(Clone, Debug)]
pub(crate) struct PhysicalAbi {
    pub platform: String,
    pub member_receiver: bool,
    pub parameters: Vec<String>,
    pub returns: String,
}
impl PhysicalAbi {
    pub(crate) fn stack_bytes(&self) -> Result<usize, String> {
        use super::abi;
        if self.platform != abi::PLATFORM
            || self.parameters.len() > abi::MAX_PARAMETERS
            || self.parameters.len() + usize::from(self.member_receiver) > abi::MAX_CIF_ARGUMENTS
        {
            return Err("unsupported physical ABI".into());
        }
        let valid = |s: &str| abi::ATOMS.iter().any(|(name, _)| *name == s);
        if self.returns != "void" && !valid(&self.returns) {
            return Err("invalid physical return".into());
        }
        let mut gp = usize::from(self.member_receiver);
        let mut sse = 0;
        let mut spill = 0;
        for p in &self.parameters {
            if !valid(p) {
                return Err("invalid physical parameter".into());
            }
            if matches!(p.as_str(), "f32" | "f64") {
                if sse >= abi::SSE_REGISTERS {
                    spill += 8;
                }
                sse += 1;
            } else {
                if gp >= abi::GP_REGISTERS {
                    spill += 8;
                }
                gp += 1;
            }
        }
        let bytes = abi::STACK_SAFETY_BUFFER.max((spill + 15) / 16 * 16);
        if bytes > abi::MAX_STACK_BYTES {
            return Err("physical stack limit".into());
        }
        Ok(bytes)
    }
    pub(crate) fn fingerprint(&self) -> Result<String, String> {
        self.stack_bytes()?;
        // Historical encoding: entity denotes a physical member pointer only.
        Ok(format!(
            "{}:{}:{}({})",
            self.platform,
            if self.member_receiver {
                "entity"
            } else {
                "none"
            },
            self.returns,
            self.parameters.join(",")
        ))
    }
}
impl Signature {
    pub(crate) fn physical(&self) -> PhysicalAbi {
        PhysicalAbi {
            platform: self.platform.clone(),
            member_receiver: self.member_receiver,
            parameters: self.parameters.iter().map(|p| p.native.clone()).collect(),
            returns: self.returns.native.clone(),
        }
    }
    pub(crate) fn position(&self, selector: i32) -> Option<&Position> {
        if selector == -1 {
            self.receiver.as_ref()
        } else if selector == -2 {
            Some(&self.returns)
        } else {
            usize::try_from(selector)
                .ok()
                .and_then(|i| self.parameters.get(i))
        }
    }
    pub(crate) fn layout(&self, selector: i32) -> Result<&RecordLayout, String> {
        self.position(selector)
            .and_then(|p| p.instance)
            .and_then(|i| self.instances.get(i))
            .map(|i| &i.record)
            .ok_or("record position unavailable".into())
    }
    pub(crate) fn borrowed(&self) -> bool {
        self.receiver
            .iter()
            .chain(&self.parameters)
            .any(|p| p.record() || p.hidden())
    }
    /// Only called for the PublicV2 origin. This is a lowering, not a trusted ABI override.
    pub(crate) fn public_wire(&self) -> serde_json::Value {
        let parameter = |p: &Position| {
            let mut v = json!({"name":p.name,"native":p.native,"projection":p.projection,"mutable":p.mutable});
            if let Some(o) = &p.ownership {
                v["ownership"] = o.clone().into();
            }
            v
        };
        let mut ret = json!({"native":self.returns.native,"projection":self.returns.projection});
        if let Some(o) = &self.returns.ownership {
            ret["ownership"] = o.clone().into();
        }
        json!({"platform":self.platform,"receiver":if self.member_receiver {"entity"} else {"none"},
            "fingerprint":self.fingerprint,"stackCopyBytes":self.stack_copy_bytes,
            "parameters":self.parameters.iter().map(parameter).collect::<Vec<_>>(),"returns":ret})
    }
}
#[derive(Clone, Debug)]
pub(crate) enum Origin {
    PublicV2,
    Trusted(HostInstanceGrant),
}
/// Not deserializable: created only from the already verified package source receipt.
#[derive(Clone, Debug)]
pub(crate) struct HostInstanceGrant {
    owner: OwnerKey,
    selected_hash: String,
    selected_bytes: std::sync::Arc<str>,
    sealed_contract: String,
    target_hash: String,
}
#[derive(Clone, Debug)]
pub(crate) struct Function {
    pub local_name: String,
    pub canonical_id: String,
    pub contract_hash: String,
    pub target: NormalizedTarget,
    pub abi: Signature,
    pub policy: Policy,
    pub requirement: String,
    pub origin: Origin,
}
impl Function {
    pub(crate) fn from_public(f: NormalizedFunction) -> Result<Self, String> {
        let position = |p: Parameter| Position {
            name: p.name,
            native: p.native,
            projection: p.projection,
            ownership: p.ownership,
            mutable: p.mutable,
            instance: None,
            nullable: false,
        };
        let receiver = (f.abi.receiver == "entity").then(|| Position {
            name: "self".into(),
            native: "ptr".into(),
            projection: Projection {
                id: "entity".into(),
                version: 1,
            },
            ownership: None,
            mutable: vec![],
            instance: None,
            nullable: false,
        });
        let abi = Signature {
            platform: f.abi.platform,
            member_receiver: receiver.is_some(),
            receiver,
            parameters: f.abi.parameters.into_iter().map(position).collect(),
            returns: Position {
                name: String::new(),
                native: f.abi.returns.native,
                projection: f.abi.returns.projection,
                ownership: f.abi.returns.ownership,
                mutable: vec![],
                instance: None,
                nullable: false,
            },
            fingerprint: f.abi.fingerprint,
            stack_copy_bytes: f.abi.stack_copy_bytes,
            instances: vec![],
            scratch: vec![],
        };
        if abi.physical().fingerprint()? != abi.fingerprint
            || abi.physical().stack_bytes()? != abi.stack_copy_bytes
        {
            return Err("physical ABI fingerprint/stack mismatch".into());
        }
        Ok(Self {
            local_name: f.local_name,
            canonical_id: f.canonical_id,
            contract_hash: f.contract_hash,
            target: f.target,
            abi,
            policy: f.policy,
            requirement: f.requirement,
            origin: Origin::PublicV2,
        })
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        // Include native immutable copies, decoded metadata and canonical temporaries.
        let selected = match &self.origin {
            Origin::Trusted(g) => g.selected_bytes.len(),
            _ => 0,
        };
        (selected + serde_json::to_vec(&self.abi).unwrap().len()).saturating_mul(8)
    }
    pub(crate) fn trusted(&self) -> bool {
        matches!(self.origin, Origin::Trusted(_))
    }
    pub(crate) fn trusted_wire(&self) -> Result<String, String> {
        let Origin::Trusted(grant) = &self.origin else {
            return Err("missing host instance authority".into());
        };
        let mut value = json!({"version":1,"signature":self.abi,"policy":self.policy,"selectedDataHash":grant.selected_hash});
        if contract::hash(&value) != grant.sealed_contract
            || self.contract_hash != grant.sealed_contract
            || contract::hash(&serde_json::to_value(&self.target).unwrap()) != grant.target_hash
        {
            return Err("host instance contract/layout/target changed after admission".into());
        }
        // Scratch is sealed into the host contract but is never a native fact: the
        // native wire omits it and carries its own hash over exactly what it receives.
        if let Some(signature) = value["signature"].as_object_mut() {
            if signature.remove("scratch").is_some() {
                let native = contract::hash(&value);
                value["contractHash"] = native.into();
                return Ok(value.to_string());
            }
        }
        value["contractHash"] = grant.sealed_contract.clone().into();
        Ok(value.to_string())
    }
    pub(crate) fn host_owner(&self) -> Option<&OwnerKey> {
        match &self.origin {
            Origin::Trusted(g) => Some(&g.owner),
            _ => None,
        }
    }
}

/// Native caller promises that all record positions are alive, aligned, bounded,
/// and mutable in PRE throughout synchronous dispatch, without concurrent access
/// requiring transactional visibility. This is not established by hashes/probes.
pub(crate) struct SynchronousRecordLifetime {
    _private: (),
}
impl SynchronousRecordLifetime {
    /// # Safety
    /// The verified native target must fulfill the lifetime contract above for
    /// every invocation. The host owns that proof, not the package's JSON.
    pub(crate) unsafe fn registered_native_target() -> Self {
        Self { _private: () }
    }
}
#[derive(Clone)]
pub(crate) struct TrustedFunctionInput {
    pub local_name: String,
    pub target: NormalizedTarget,
    pub signature: Signature,
    pub policy: Policy,
    pub requirement: String,
}
/// This input is captured by the verified host selector, not a community member.
/// JSON is an owned selected offset-key -> u32 map. It is hashed before decoding.
pub(crate) struct SelectedLayoutData {
    pub bytes: std::sync::Arc<str>,
    pub sha256: String,
}

struct SelectedOffsets(std::collections::BTreeMap<String, u32>);
impl<'de> Deserialize<'de> for SelectedOffsets {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = SelectedOffsets;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unique selected offset-key map")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut values = std::collections::BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, u32>()? {
                    if key.is_empty()
                        || key.len() > 256
                        || key.contains('\0')
                        || values.insert(key, value).is_some()
                    {
                        return Err(serde::de::Error::custom(
                            "invalid/duplicate selected offset key",
                        ));
                    }
                }
                Ok(SelectedOffsets(values))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

/// Only the verified receipt can call this bridge. No JS constructor/native exists.
pub(crate) fn prepare_verified_package(
    receipt: &crate::v8host::function_adapter::PreparedPackageReceipt,
    inputs: Vec<TrustedFunctionInput>,
    selected: SelectedLayoutData,
    _lifetime: SynchronousRecordLifetime,
) -> Result<super::provenance::PreparedCandidate, String> {
    use super::provenance::*;
    let (owner, manifest) = receipt.instance_authority()?;
    if inputs.len() > 256
        || selected.bytes.len() > 1024 * 1024
        || contract::hash_bytes(selected.bytes.as_bytes()) != selected.sha256
    {
        return Err("invalid/oversized selected layout data hash".into());
    }
    let SelectedOffsets(offsets) =
        serde_json::from_str(&selected.bytes).map_err(|e| format!("selected offsets: {e}"))?;
    let mut functions = Vec::with_capacity(inputs.len());
    let mut names = BTreeSet::new();
    for input in inputs {
        let canonical = format!("{}::{}", owner.id, input.local_name);
        if !contract::identifier(&input.local_name)
            || !names.insert(input.local_name.clone())
            || !matches!(input.requirement.as_str(), "required" | "optional")
        {
            return Err("invalid trusted function identity/requirement".into());
        }
        let mut signature = input.signature;
        let checked = (|| {
            input.target.validate()?;
            contract::validate_policy(&input.policy)?;
            if input.policy.surfaces.iter().any(|s| s == "call") {
                return Err("record/hidden native input cannot be manufactured".into());
            }
            if signature.member_receiver != signature.receiver.is_some()
                || signature.instances.len() > 33
            {
                return Err("invalid physical receiver/instance count".into());
            }
            let physical = signature.physical();
            if signature.fingerprint != physical.fingerprint()?
                || signature.stack_copy_bytes != physical.stack_bytes()?
            {
                return Err("physical fingerprint/stack mismatch".into());
            }
            for instance in &mut signature.instances {
                if instance.codec_id != "borrowed-record"
                    || instance.codec_version != 1
                    || instance.kind.is_some()
                {
                    return Err("unsupported registered kind/version".into());
                }
                for field in &mut instance.record.fields {
                    let selected = offsets
                        .get(&field.offset_key)
                        .ok_or("missing selected offsetKey")?;
                    if field.offset != *selected {
                        return Err("stale selected field offset".into());
                    }
                    if !field.write.is_empty() && !input.policy.surfaces.iter().any(|s| s == "pre")
                    {
                        return Err("record writes require PRE surface".into());
                    }
                }
                instance.record.validate()?;
                if instance.layout_hash != instance.record.hash() {
                    return Err("record layout hash mismatch".into());
                }
            }
            let mut exposed = BTreeSet::new();
            for (index, p) in signature
                .receiver
                .iter()
                .map(|p| (-1, p))
                .chain(
                    signature
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(i, p)| (i as i32, p)),
                )
                .chain(std::iter::once((-2, &signature.returns)))
            {
                if index != -2
                    && !p.hidden()
                    && (!(contract::identifier(&p.name) || (index == -1 && p.name == "self"))
                        || RESERVED_FRAME_NAMES.contains(&p.name.as_str())
                        || !exposed.insert(&p.name))
                {
                    return Err("invalid/duplicate exposed position name".into());
                }
                if p.record() || p.hidden() {
                    if index == -2
                        || p.native != "ptr"
                        || p.projection.version != 1
                        || !p.mutable.is_empty()
                    {
                        return Err("record/hidden direction or replacement unsupported".into());
                    }
                    if p.record() {
                        if p.ownership.as_deref() != Some("synchronous-record")
                            || p.instance.is_none_or(|i| i >= signature.instances.len())
                        {
                            return Err("record lifetime/instance unavailable".into());
                        }
                    } else if p.ownership.as_deref() != Some("invocation-passthrough")
                        || p.instance.is_some()
                        || p.nullable
                    {
                        return Err("hidden lifetime/instance invalid".into());
                    }
                } else {
                    if p.instance.is_some() || p.nullable {
                        return Err("unexpected instance/nullability".into());
                    }
                    if p.projection.id == "string-indirect" {
                        // Trusted-only: ptr to an object whose first word is a char*.
                        if index < 0 || p.native != "ptr" || p.projection.version != 1
                            || p.ownership.as_deref() != Some("native-observed") || !p.mutable.is_empty() {
                            return Err("string-indirect is a readonly native-observed parameter".into());
                        }
                    } else {
                        contract::projection(&p.native, &p.projection, index == -2)?;
                        contract::copied_ownership(
                            &p.projection.id,
                            p.ownership.as_deref(),
                            index == -2,
                            &canonical,
                        )?;
                    }
                    if !(p.mutable.is_empty() || p.mutable == ["pre"])
                        || (!p.mutable.is_empty()
                            && !input.policy.surfaces.iter().any(|s| s == "pre"))
                    {
                        return Err("invalid position mutation".into());
                    }
                    if index == -1
                        && (p.native != "ptr"
                            || !matches!(p.projection.id.as_str(), "entity" | "entity?")
                            || !p.mutable.is_empty())
                    {
                        return Err("invalid receiver projection".into());
                    }
                    if p.ownership.as_deref() == Some("native-observed") && !p.mutable.is_empty() {
                        return Err("native-observed position is readonly".into());
                    }
                    if p.ownership.as_deref() == Some("callee-borrowed")
                        && !p.mutable.is_empty()
                        && signature.returns.native == "ptr"
                    {
                        return Err(
                            "mutable copied input with pointer return requires callee-retained"
                                .into(),
                        );
                    }
                }
            }
            if signature.scratch.len() > MAX_SCRATCH {
                return Err("scratch slot limit".into());
            }
            if !signature.scratch.is_empty() && !input.policy.surfaces.iter().any(|s| s == "pre") {
                return Err("scratch slots require PRE surface".into());
            }
            for slot in &signature.scratch {
                if !contract::identifier(&slot.name)
                    || slot.name.len() > 128
                    || RESERVED_FRAME_NAMES.contains(&slot.name.as_str())
                    || !exposed.insert(&slot.name)
                {
                    return Err("invalid/duplicate scratch slot name".into());
                }
                slot.projection()?;
            }
            if signature.returns.ownership.as_deref() == Some("native-observed")
                && input.policy.suppression != "none"
            {
                return Err("native-observed return requires suppression:none".into());
            }
            Ok::<_, String>(())
        })();
        if input.requirement == "required" {
            checked.as_ref().map_err(|e| format!("{canonical}: {e}"))?;
        }
        let target_hash = contract::hash(&serde_json::to_value(&input.target).unwrap());
        let hash = contract::hash(
            &json!({"version":1,"signature":signature,"policy":input.policy,"selectedDataHash":selected.sha256}),
        );
        let f = Function {
            local_name: input.local_name,
            canonical_id: canonical,
            contract_hash: hash.clone(),
            target: input.target,
            abi: signature,
            policy: input.policy,
            requirement: input.requirement.clone(),
            origin: Origin::Trusted(HostInstanceGrant {
                owner: owner.clone(),
                selected_hash: selected.sha256.clone(),
                selected_bytes: selected.bytes.clone(),
                sealed_contract: hash.clone(),
                target_hash: target_hash.clone(),
            }),
        };
        let unavailable = checked.err();
        let validation = unavailable.as_ref().map_or(ValidationResult::Pending, |e| {
            ValidationResult::Unavailable(e.clone())
        });
        let instances = Some(InstanceProvenance {
            manifest_hash: manifest.clone(),
            selected_data_hash: selected.sha256.clone(),
            layouts: f
                .abi
                .instances
                .iter()
                .map(|i| LayoutProvenance {
                    codec_id: i.codec_id.clone(),
                    codec_version: i.codec_version,
                    layout_hash: i.layout_hash.clone(),
                })
                .collect(),
        });
        functions.push(PreparedFunction {
            function: f,
            unavailable,
            provenance: Provenance {
                archive_hash: String::new(),
                instances,
                base_contract_hash: hash,
                overrides: vec![],
                final_target_hash: target_hash,
                resolver_result: validation.clone(),
                validator_result: validation,
                required: input.requirement == "required",
            },
        });
    }
    Ok(PreparedCandidate {
        base: None,
        owner_id: owner.id,
        functions,
    })
}

//! Untrusted archive boundary. Host-owned adapters/codecs cannot be authorized by archive text.
use super::abi;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn hash(value: &Value) -> String {
    hash_bytes(&serde_json_canonicalizer::to_vec(value).expect("JSON value"))
}
pub(crate) fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedBundle {
    pub schema_version: u32,
    pub owner_id: String,
    pub bundle_hash: String,
    pub functions: Vec<NormalizedFunction>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedFunction {
    pub local_name: String,
    pub canonical_id: String,
    pub contract_hash: String,
    pub target: NormalizedTarget,
    pub abi: Abi,
    pub policy: Policy,
    pub requirement: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Abi {
    pub platform: String,
    pub receiver: String,
    pub fingerprint: String,
    pub stack_copy_bytes: usize,
    pub parameters: Vec<Parameter>,
    pub returns: Returns,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    pub name: String,
    pub native: String,
    pub projection: Projection,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub ownership: Option<String>,
    pub mutable: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Returns {
    pub native: String,
    pub projection: Projection,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub ownership: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub id: String,
    pub version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub id: String,
    pub version: u32,
    pub contract_hash: String,
    pub surfaces: Vec<String>,
    pub self_call: String,
    pub suppression: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum NormalizedTarget {
    Signature {
        module: String,
        pattern: String,
        resolve: String,
        derivation: String,
        #[serde(rename = "candidateValidate")]
        candidate_validate: Validator,
        #[serde(rename = "targetValidate")]
        target_validate: Validator,
    },
    Virtual {
        module: String,
        class: String,
        index: u32,
        resolve: String,
        derivation: String,
        #[serde(rename = "candidateValidate")]
        candidate_validate: Validator,
        #[serde(rename = "targetValidate")]
        target_validate: Validator,
    },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validator {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub prologue: Option<String>,
    #[serde(
        rename = "string-xref",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub string_xref: Option<StringXref>,
    #[serde(
        rename = "vtable-member",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub vtable_member: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StringXref {
    pub at: u32,
    pub disp_off: u32,
    pub instr_len: u32,
    pub expect: String,
}
pub fn nonempty(s: &str) -> bool {
    !s.is_empty() && !s.contains('\0')
}
pub fn identifier(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(),Some(x) if x.is_ascii_alphabetic()||x=='_'||x=='$')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_' || x == '$')
        && !matches!(
            s,
            "self" | "returnValue" | "constructor" | "prototype" | "__proto__"
        )
}
fn require(ok: bool, why: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(why.into())
    }
}
impl Validator {
    pub fn empty(&self) -> bool {
        self.prologue.is_none() && self.string_xref.is_none() && self.vtable_member.is_none()
    }
    fn validate(&self) -> Result<(), String> {
        for s in [&self.prologue, &self.vtable_member].into_iter().flatten() {
            require(nonempty(s), "empty/NUL validator")?;
        }
        if let Some(x) = &self.string_xref {
            require(
                x.at <= i32::MAX as u32
                    && x.instr_len > 0
                    && x.instr_len <= i32::MAX as u32
                    && u64::from(x.disp_off) + 4 <= u64::from(x.instr_len)
                    && nonempty(&x.expect)
                    && x.expect.len() <= 256,
                "invalid string-xref bounds",
            )?;
        }
        Ok(())
    }
}
impl NormalizedTarget {
    pub fn validate(&self) -> Result<(), String> {
        let (module, candidate, target) = match self {
            Self::Signature {
                module,
                pattern,
                resolve,
                derivation,
                candidate_validate,
                target_validate,
            } => {
                require(nonempty(pattern), "invalid pattern")?;
                let expected = match resolve.as_str() {
                    "direct" => "identity",
                    "ctor-body-xref" => "ctor-body-xref",
                    "lea-disp" => "lea-disp",
                    "validated-call" => "e8-rel32",
                    _ => return Err("unsupported resolver".into()),
                };
                require(derivation == expected, "resolver/derivation mismatch")?;
                if resolve == "validated-call" {
                    require(
                        candidate_validate.string_xref.is_some(),
                        "validated-call requires candidate string-xref",
                    )?;
                } else {
                    require(
                        candidate_validate.empty() && !target_validate.empty(),
                        "invalid validator stage",
                    )?;
                }
                (module, candidate_validate, target_validate)
            }
            Self::Virtual {
                module,
                class,
                index,
                resolve,
                derivation,
                candidate_validate,
                target_validate,
            } => {
                require(
                    nonempty(class)
                        && *index < 512
                        && resolve == "direct"
                        && derivation == "virtual-slot"
                        && candidate_validate.empty()
                        && target_validate.prologue.is_some(),
                    "invalid virtual target",
                )?;
                (module, candidate_validate, target_validate)
            }
        };
        require(nonempty(module), "invalid module")?;
        candidate.validate()?;
        target.validate()
    }
}
pub(crate) fn projection(native: &str, p: &Projection, ret: bool) -> Result<(), String> {
    let valid = match p.id.as_str() {
        "void" => ret && native == "void",
        "bool" => native == "u8",
        "entity" | "entity?" | "string" | "vector" => native == "ptr",
        "i32" | "u32" | "i64" | "u64" | "f32" | "f64" => native == p.id,
        _ => false,
    };
    require(
        p.version == 1 && valid,
        "unsupported ABI/projection pair (host authority required for custom codecs)",
    )
}
pub(crate) fn copied_ownership(id: &str, owner: Option<&str>, ret: bool, field: &str) -> Result<(), String> {
    if matches!(id, "string" | "vector") {
        let allowed = if ret { &["caller-borrowed", "native-observed"][..] } else { &["callee-borrowed", "callee-retained", "native-observed"][..] };
        require(owner.is_some_and(|value| allowed.contains(&value)),
            &format!("{field}: copied ownership is required and must match its direction; rebuild old copied declarations"))
    } else {
        require(owner.is_none(), &format!("{field}: ownership is only valid for copied string/vector positions"))
    }
}
pub fn parse(
    text: &str,
    owner: &str,
    summary: &Value,
    permissions: &[String],
) -> Result<NormalizedBundle, String> {
    let b: NormalizedBundle =
        serde_json::from_str(text).map_err(|e| format!("engine-functions.json: {e}"))?;
    require(
        b.schema_version == 2 && b.owner_id == owner && nonempty(owner),
        "schema/owner mismatch",
    )?;
    let mut previous = "";
    for f in &b.functions {
        require(
            identifier(&f.local_name)
                && f.local_name.as_str() > previous
                && f.canonical_id == format!("{owner}::{}", f.local_name),
            "invalid/unsorted/duplicate function identity",
        )?;
        previous = &f.local_name;
        require(
            matches!(f.requirement.as_str(), "optional" | "required"),
            "invalid requirement",
        )?;
        f.target.validate()?;
        let a = &f.abi;
        require(
            a.platform == abi::PLATFORM && abi::RECEIVERS.contains(&a.receiver.as_str()),
            "unsupported ABI platform/receiver",
        )?;
        require(
            a.parameters.len() <= abi::MAX_PARAMETERS
                && a.parameters.len() + usize::from(a.receiver == "entity")
                    <= abi::MAX_CIF_ARGUMENTS,
            "ABI argument limit",
        )?;
        let mut names = HashSet::new();
        let mut gp = usize::from(a.receiver == "entity");
        let mut sse = 0;
        let mut spill = 0;
        for p in &a.parameters {
            require(
                identifier(&p.name) && names.insert(&p.name),
                "invalid/duplicate parameter name",
            )?;
            projection(&p.native, &p.projection, false)?;
            copied_ownership(&p.projection.id, p.ownership.as_deref(), false, &format!("{} parameter {}", f.canonical_id, p.name))?;
            require(
                p.mutable.is_empty() || p.mutable == ["pre"],
                "invalid mutability",
            )?;
            require(
                abi::ATOMS.iter().any(|(name, _)| *name == p.native),
                "unsupported atom",
            )?;
            if matches!(p.native.as_str(), "f32" | "f64") {
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
        projection(&a.returns.native, &a.returns.projection, true)?;
        copied_ownership(&a.returns.projection.id, a.returns.ownership.as_deref(), true, &format!("{} returns", f.canonical_id))?;
        let stack = abi::STACK_SAFETY_BUFFER.max((spill + 15) / 16 * 16);
        require(
            stack <= abi::MAX_STACK_BYTES && a.stack_copy_bytes == stack,
            "stack-copy mismatch",
        )?;
        require(
            a.fingerprint
                == format!(
                    "{}:{}:{}({})",
                    abi::PLATFORM,
                    a.receiver,
                    a.returns.native,
                    a.parameters
                        .iter()
                        .map(|p| p.native.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            "ABI fingerprint mismatch",
        )?;
        let p = &f.policy;
        validate_policy(p)?;
        let pre=p.surfaces.iter().any(|s|s=="pre");
        require(
            pre || a.parameters.iter().all(|p| p.mutable.is_empty()),
            "mutation requires pre",
        )?;
        for parameter in &a.parameters {
            require(!(parameter.ownership.as_deref() == Some("native-observed") && p.surfaces.iter().any(|s| s == "call")),
                &format!("{} parameter {}: native-observed disallows call", f.canonical_id, parameter.name))?;
            require(!(parameter.ownership.as_deref() == Some("native-observed") && !parameter.mutable.is_empty()),
                &format!("{} parameter {}: native-observed disallows PRE edits", f.canonical_id, parameter.name))?;
            require(!(parameter.ownership.as_deref() == Some("callee-borrowed") && !parameter.mutable.is_empty() && a.returns.native == "ptr"),
                &format!("{} parameter {}: mutable copied pointer-return input requires callee-retained", f.canonical_id, parameter.name))?;
        }
        require(!(a.returns.ownership.as_deref() == Some("native-observed") && p.surfaces.iter().any(|s| s == "call")),
            &format!("{} returns: native-observed disallows call", f.canonical_id))?;
        require(!(a.returns.ownership.as_deref() == Some("native-observed") && pre && p.suppression != "none"),
            &format!("{} returns: native-observed requires suppression:none", f.canonical_id))?;
        let mut policy = serde_json::to_value(p).unwrap();
        policy.as_object_mut().unwrap().remove("contractHash");
        require(hash(&policy) == p.contract_hash, "policy hash mismatch")?;
        require(
            hash(&json!({"abi":a,"policy":p})) == f.contract_hash,
            "contract hash mismatch",
        )?;
    }
    let mut value = serde_json::to_value(&b).unwrap();
    value.as_object_mut().unwrap().remove("bundleHash");
    require(hash(&value) == b.bundle_hash, "bundle hash mismatch")?;
    let expected = json!({"schemaVersion":2,"bundleHash":b.bundle_hash,"functions":b.functions.iter().map(|f|json!({"canonicalId":f.canonical_id,"contractHash":f.contract_hash,"surfaces":f.policy.surfaces,"mutates":f.abi.parameters.iter().any(|p|!p.mutable.is_empty()),"suppresses":f.policy.suppression!="none","requirement":f.requirement})).collect::<Vec<_>>()});
    require(*summary == expected, "manifest/member summary mismatch")?;
    for (surface, permission) in [
        ("call", "engine:calls"),
        ("pre", "engine:hooks"),
        ("post", "engine:hooks"),
    ] {
        if b.functions
            .iter()
            .any(|f| f.policy.surfaces.iter().any(|s| s == surface))
        {
            require(
                permissions.iter().any(|p| p == permission),
                "missing derived permission",
            )?;
        }
    }
    Ok(b)
}

/// Host identities are never deserialized from community archive text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum OwnerKind { Plugin, GamePackage }
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OwnerKey { pub id: String, pub generation: u64, pub kind: OwnerKind }
impl OwnerKey {
    pub(crate) fn plugin(id: &str, generation: u64) -> Self {
        Self { id: id.into(), generation, kind: OwnerKind::Plugin }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PackageInstanceKey { pub parent: OwnerKey, pub package_owner: OwnerKey }
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImplementationManifestHash(String);
impl ImplementationManifestHash {
    /// The host caller must have verified the actual manifest bytes for this source.
    /// This constructor checks encoding only; it does not validate a package manifest.
    pub(crate) fn new(hash: String) -> Result<Self, String> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err("implementation manifest hash must be lowercase SHA256".into());
        }
        Ok(Self(hash))
    }
    pub(crate) fn as_str(&self) -> &str { &self.0 }
}
/// Possession of this capability, rather than the serializable key, grants bootstrap authority.
#[derive(Clone)]
pub(crate) struct HostPackageOwner(std::rc::Rc<PackageFunctionLifetime>);
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackageFunctionPhase {
    Unactivated,
    Active,
    Retired,
}
pub(crate) struct PackageFunctionLifetime {
    pub(super) key: OwnerKey,
    pub(super) phase: std::cell::Cell<PackageFunctionPhase>,
    pub(super) source: std::cell::Cell<PackageFunctionPhase>,
}
impl HostPackageOwner {
    pub(crate) fn mint(id: &str) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        if id.is_empty() || id.contains('\0') {
            return Err("invalid host package id".into());
        }
        let generation = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "package generation exhausted")?;
        Ok(Self(std::rc::Rc::new(PackageFunctionLifetime {
            key: OwnerKey {
                id: id.into(),
                generation,
                kind: OwnerKind::GamePackage,
            },
            phase: std::cell::Cell::new(PackageFunctionPhase::Unactivated),
            source: std::cell::Cell::new(PackageFunctionPhase::Unactivated),
        })))
    }
    pub(crate) fn key(&self) -> &OwnerKey {
        &self.0.key
    }
    pub(crate) fn claim_source(&self) -> Result<(), String> {
        if self.0.source.get() != PackageFunctionPhase::Unactivated || self.is_retired() {
            return Err("package source already registered or retired".into());
        }
        self.0.source.set(PackageFunctionPhase::Active);
        Ok(())
    }
    pub(crate) fn retire_source(&self) {
        self.0.source.set(PackageFunctionPhase::Retired);
    }
    pub(crate) fn is_retired(&self) -> bool {
        self.0.phase.get() == PackageFunctionPhase::Retired
            || self.0.source.get() == PackageFunctionPhase::Retired
    }
    pub(super) fn lifetime(&self) -> std::rc::Rc<PackageFunctionLifetime> {
        self.0.clone()
    }
}

pub(crate) fn validate_policy(p: &Policy) -> Result<(), String> {
        let surfaces: Vec<String> = ["call", "pre", "post"]
            .into_iter()
            .filter(|s| p.surfaces.iter().any(|v| v == s))
            .map(String::from)
            .collect();
        let pre = p.surfaces.iter().any(|s| s == "pre");
        require(
            !surfaces.is_empty()
                && surfaces == p.surfaces
                && p.id == "generic.v2"
                && p.version == 1
                && p.self_call == "bypass-own-hooks"
                && (if pre { matches!(p.suppression.as_str(), "generic" | "none") } else { p.suppression == "none" }),
            "invalid/host-only policy",
        )?;
    let mut value=serde_json::to_value(p).unwrap();
    value.as_object_mut().unwrap().remove("contractHash");
    require(hash(&value)==p.contract_hash,"policy hash mismatch")
}

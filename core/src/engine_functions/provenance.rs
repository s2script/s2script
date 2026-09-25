//! Immutable input to the later resolver/registry. "Pending" is never successful resolution.
use super::contract::NormalizedBundle;
use super::instance::Function;
#[derive(Clone, Debug)]
pub struct OverrideProvenance {
    pub relative_path: String,
    pub sha256: String,
}
#[derive(Clone, Debug)]
pub enum ValidationResult {
    Pending,
    Unavailable(String),
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all="camelCase")]
pub struct LayoutProvenance { pub codec_id:String, pub codec_version:u32, pub layout_hash:String }
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all="camelCase")]
pub struct InstanceProvenance { pub manifest_hash:String, pub selected_data_hash:String, pub layouts:Vec<LayoutProvenance> }
#[derive(Clone, Debug)]
pub struct Provenance {
    pub archive_hash: String,
    pub instances: Option<InstanceProvenance>,
    pub base_contract_hash: String,
    pub overrides: Vec<OverrideProvenance>,
    pub final_target_hash: String,
    pub resolver_result: ValidationResult,
    pub validator_result: ValidationResult,
    pub required: bool,
}
#[derive(Clone, Debug)]
pub struct PreparedFunction {
    pub(super) function: Function,
    pub(super) provenance: Provenance,
    pub(super) unavailable: Option<String>,
}
impl PreparedFunction {
    pub fn function(&self) -> &Function {
        &self.function
    }
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }
}
#[derive(Clone, Debug)]
pub struct PreparedCandidate {
    pub(super) base: Option<NormalizedBundle>,
    pub(super) owner_id: String,
    pub(super) functions: Vec<PreparedFunction>,
}
impl PreparedCandidate {
    pub(crate) fn owner_id(&self) -> &str { &self.owner_id }
    pub fn base(&self) -> &NormalizedBundle {
        self.base.as_ref().expect("public archive candidate")
    }
    pub fn functions(&self) -> &[PreparedFunction] {
        &self.functions
    }
}

//! Immutable input to the later resolver/registry. "Pending" is never successful resolution.
use super::contract::{NormalizedBundle, NormalizedFunction};
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
#[derive(Clone, Debug)]
pub struct Provenance {
    pub archive_hash: String,
    pub base_contract_hash: String,
    pub overrides: Vec<OverrideProvenance>,
    pub final_target_hash: String,
    pub resolver_result: ValidationResult,
    pub validator_result: ValidationResult,
    pub required: bool,
}
#[derive(Clone, Debug)]
pub struct PreparedFunction {
    pub(super) function: NormalizedFunction,
    pub(super) provenance: Provenance,
    pub(super) unavailable: Option<String>,
}
impl PreparedFunction {
    pub fn function(&self) -> &NormalizedFunction {
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
    pub(super) base: NormalizedBundle,
    pub(super) functions: Vec<PreparedFunction>,
}
impl PreparedCandidate {
    pub fn base(&self) -> &NormalizedBundle {
        &self.base
    }
    pub fn functions(&self) -> &[PreparedFunction] {
        &self.functions
    }
}

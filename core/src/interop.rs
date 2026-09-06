//! Versioned protocol 2 wire algebra. No JavaScript or engine references cross this layer.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Schema {
    Null,
    Boolean,
    Number,
    String,
    Void,
    EntityRef,
    Literal { value: Value },
    Array { item: Box<Schema> },
    Object { fields: BTreeMap<String, Field> },
    Union { variants: Vec<Schema> },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub schema: Schema,
    pub optional: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Method {
    pub args: Vec<Field>,
    pub result: Schema,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    pub kind: String,
    pub payload: Schema,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub version: u32,
    pub methods: BTreeMap<String, Method>,
    pub forwards: BTreeMap<String, Forward>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub metadata: Metadata,
    pub sha256: String,
}
impl Schema {
    fn well_formed(&self, allow_void: bool, depth: usize) -> bool {
        if depth > 64 {
            return false;
        }
        match self {
            Self::Void => allow_void,
            Self::Literal { value } => value.is_boolean() || value.is_string() || value.is_number(),
            Self::Array { item } => item.well_formed(false, depth + 1),
            Self::Object { fields } => {
                !fields.contains_key("__s2ref")
                    && fields
                        .values()
                        .all(|f| f.schema.well_formed(false, depth + 1))
            }
            Self::Union { variants } => {
                !variants.is_empty() && variants.iter().all(|s| s.well_formed(false, depth + 1))
            }
            _ => true,
        }
    }
    pub fn accepts(&self, value: &Value) -> bool {
        match self {
            Self::Null => value.is_null(),
            Self::Boolean => value.is_boolean(),
            Self::Number => value.is_number(),
            Self::String => value.is_string(),
            Self::Void => false,
            Self::Literal { value: literal } => {
                value == literal
                    || (value.is_number()
                        && literal.is_number()
                        && value.as_f64() == literal.as_f64())
            }
            Self::Array { item } => value
                .as_array()
                .map_or(false, |a| a.iter().all(|v| item.accepts(v))),
            Self::Object { fields } => value.as_object().map_or(false, |o| {
                o.keys().all(|k| fields.contains_key(k))
                    && fields
                        .iter()
                        .all(|(k, f)| o.get(k).map_or(f.optional, |v| f.schema.accepts(v)))
            }),
            Self::Union { variants } => variants.iter().any(|s| s.accepts(value)),
            Self::EntityRef => value
                .as_object()
                .filter(|o| o.len() == 1)
                .and_then(|o| o.get("__s2ref"))
                .and_then(Value::as_array)
                .map_or(false, |a| {
                    a.len() == 2 && a.iter().all(|v| v.as_u64().is_some())
                }),
        }
    }
}
impl Method {
    pub fn accepts_args(&self, value: &Value) -> bool {
        value.as_array().map_or(false, |a| {
            a.len() <= self.args.len()
                && self
                    .args
                    .iter()
                    .enumerate()
                    .all(|(i, f)| a.get(i).map_or(f.optional, |v| f.schema.accepts(v)))
        })
    }
}
impl Contract {
    pub fn validate(&self) -> Result<(), String> {
        let m = &self.metadata;
        if m.version != 1
            || !m.methods.values().all(|v| {
                v.args.iter().all(|a| a.schema.well_formed(false, 0))
                    && v.result.well_formed(true, 0)
            })
            || !m
                .forwards
                .values()
                .all(|f| f.kind == "notification" && f.payload.well_formed(false, 0))
        {
            return Err("InterfaceContractError: unsupported metadata".into());
        }
        // RFC 8785 JCS matches the SDK: ECMAScript numbers and UTF-16 code-unit key order.
        let bytes = serde_json_canonicalizer::to_vec(m).map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(bytes)) != self.sha256 {
            return Err("InterfaceContractError: metadata digest mismatch".into());
        }
        Ok(())
    }
}
pub fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn validate_manifest(m: &crate::loader_worker::Manifest) -> Result<(), String> {
    if m.interface_protocol != 1 && m.interface_protocol != 2 {
        return Err("InterfaceContractError: unsupported interfaceProtocol".into());
    }
    if m.interface_protocol == 1 {
        if !m.interface_contracts.is_empty() || m.publishes.values().any(|p| p.contract.is_some()) {
            return Err("InterfaceContractError: metadata requires interfaceProtocol 2".into());
        }
        return Ok(());
    }
    if !m.api_version.starts_with("3.") {
        return Err("InterfaceContractError: protocol 2 requires host API 3".into());
    }
    for (name, p) in &m.publishes {
        if !valid_hash(&p.types_sha256) {
            return Err(format!(
                "InterfaceContractError: {name} requires typesSha256"
            ));
        }
        p.contract
            .as_ref()
            .ok_or_else(|| format!("InterfaceContractError: {name} missing metadata"))?
            .validate()?;
    }
    for name in m
        .plugin_dependencies
        .keys()
        .chain(m.optional_plugin_dependencies.keys())
    {
        if !m
            .compiled_against
            .get(name)
            .map_or(false, |h| valid_hash(h))
        {
            return Err(format!(
                "InterfaceContractError: {name} requires compiledAgainst"
            ));
        }
        m.interface_contracts
            .get(name)
            .ok_or_else(|| format!("InterfaceContractError: {name} missing verified metadata"))?
            .validate()?;
    }
    if m.interface_contracts.keys().any(|k| {
        !m.plugin_dependencies.contains_key(k) && !m.optional_plugin_dependencies.contains_key(k)
    }) {
        return Err("InterfaceContractError: metadata for undeclared dependency".into());
    }
    Ok(())
}

#[cfg(test)]
mod canonical_tests {
    #[test]
    fn sdk_numeric_and_utf16_canonical_contract_is_accepted() {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../packages/sdk/test/fixtures/interop/canonical.json"
        ))
        .unwrap();
        let contract: super::Contract = serde_json::from_value(vector["contract"].clone()).unwrap();
        assert_eq!(contract.validate(), Ok(()));
        assert_eq!(
            serde_json_canonicalizer::to_string(&contract.metadata).unwrap(),
            vector["canonical"].as_str().unwrap()
        );
    }
}

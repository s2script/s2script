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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_writable"
    )]
    pub writable: Option<Vec<String>>,
}
// Missing is valid for notifications/hooks; present null must not vanish before hashing.
fn present_writable<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(deserializer).map(Some)
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
impl Forward {
    fn well_formed(&self) -> bool {
        if !self.payload.well_formed(false, 0) {
            return false;
        }
        match (self.kind.as_str(), &self.writable, &self.payload) {
            ("notification" | "hook", None, _) => true,
            ("transform", Some(keys), Schema::Object { fields }) => {
                keys.iter().all(|k| fields.contains_key(k))
                    && keys
                        .windows(2)
                        .all(|k| k[0].encode_utf16().cmp(k[1].encode_utf16()).is_lt())
            }
            _ => false,
        }
    }
    /// Validate the entire response before any patch can be applied. No numeric coercion.
    pub fn response<'a>(
        &self,
        value: &'a Value,
    ) -> Option<(
        crate::multiplexer::HookResult,
        Option<&'a serde_json::Map<String, Value>>,
    )> {
        use crate::multiplexer::HookResult;
        let action = |value: &Value| match value.as_f64()? {
            0.0 => Some(HookResult::Continue),
            1.0 => Some(HookResult::Changed),
            2.0 => Some(HookResult::Handled),
            3.0 => Some(HookResult::Stop),
            _ => None,
        };
        if self.kind == "hook" {
            return Some((action(value)?, None));
        }
        if self.kind != "transform" {
            return None;
        }
        let object = value.as_object()?;
        if object.keys().any(|key| key != "result" && key != "patch") {
            return None;
        }
        let result = action(object.get("result")?)?;
        let patch = if let Some(patch) = object.get("patch") {
            if result != HookResult::Changed {
                return None;
            }
            let patch = patch.as_object()?;
            let Schema::Object { fields } = &self.payload else {
                return None;
            };
            let writable = self.writable.as_ref()?;
            if !patch.iter().all(|(k, v)| {
                writable.contains(k) && fields.get(k).is_some_and(|f| f.schema.accepts(v))
            }) {
                return None;
            }
            Some(patch)
        } else {
            None
        };
        Some((result, patch))
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
            || !m.forwards.values().all(Forward::well_formed)
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
    fn null_writable_cannot_disappear_before_metadata_digest_validation() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../packages/sdk/test/fixtures/interop/decisions.json"
        ))
        .unwrap();
        let mut missing = value.clone();
        missing["metadata"]["forwards"]["OnFormat"]
            .as_object_mut()
            .unwrap()
            .remove("writable");
        assert!(
            serde_json::from_value::<super::Contract>(missing)
                .unwrap()
                .validate()
                .is_err(),
            "transforms require an explicit writable array"
        );
        for forward in ["OnRequest", "OnCountChanged", "OnFormat"] {
            let mut changed = value.clone();
            changed["metadata"]["forwards"][forward]["writable"] = serde_json::Value::Null;
            assert!(
                serde_json::from_value::<super::Contract>(changed).is_err(),
                "{forward}: explicitly null writable must not normalize to an absent field"
            );
        }
    }
    #[test]
    fn decision_contract_accepts_sdk_digest_and_rejects_mode_or_writable_drift() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../packages/sdk/test/fixtures/interop/decisions.json"
        ))
        .unwrap();
        let contract: super::Contract = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(contract.validate(), Ok(()));
        let mut changed = value.clone();
        changed["metadata"]["forwards"]["OnRequest"]["kind"] = "notification".into();
        let changed: super::Contract = serde_json::from_value(changed).unwrap();
        assert!(changed.validate().unwrap_err().contains("digest mismatch"));
        for writable in [
            serde_json::json!(["identity"]),
            serde_json::json!(["text", "text"]),
            serde_json::json!(["absent"]),
            serde_json::Value::Null,
        ] {
            let mut changed = value.clone();
            changed["metadata"]["forwards"]["OnFormat"]["writable"] = writable;
            match serde_json::from_value::<super::Contract>(changed) {
                Ok(changed) => assert!(changed.validate().is_err()),
                Err(_) => {} // Explicit null is rejected while decoding, before digest validation.
            }
        }
    }
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

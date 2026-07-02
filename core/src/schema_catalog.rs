//! Pure, engine-generic schema catalog builder (V8-free, no CS2 identifiers). The live SDK walk lives
//! in the shim; it streams classes/fields here via v8host's C-ABI callbacks. This module only
//! assembles + serializes — so it is fully unit-testable without an engine.

use serde::Serialize;
use std::collections::BTreeMap;

/// A field's type. `kind` ∈ atomic | handle | class | ptr | enum | unknown (the shim maps the
/// CSchemaType category → this string). `name` = the type name for atomic/class/enum; `inner` = the
/// referenced class for handle/ptr. Absent fields are omitted from JSON.
#[derive(Serialize)]
pub struct FieldType {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner: Option<String>,
}

#[derive(Serialize)]
pub struct Field {
    pub name: String,
    pub offset: i32,
    #[serde(rename = "type")]
    pub ty: FieldType,
}

#[derive(Serialize)]
pub struct Class {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub fields: Vec<Field>,
}

/// The catalog. Classes are keyed in a BTreeMap for deterministic (sorted) output; fields keep
/// insertion order (the shim emits them in schema order, which is stable per binary).
pub struct Catalog {
    classes: BTreeMap<String, Class>,
}

impl Catalog {
    pub fn new() -> Self {
        Self { classes: BTreeMap::new() }
    }

    /// Record a class (idempotent: a repeat keeps the first, so a duplicate emit is harmless).
    pub fn add_class(&mut self, name: &str, parent: Option<&str>) {
        self.classes.entry(name.to_string()).or_insert_with(|| Class {
            parent: parent.map(|p| p.to_string()),
            fields: Vec::new(),
        });
    }

    /// Append a field to its class. If the class was never added, the field is dropped (degrade,
    /// never panic) — the shim always emits the class before its fields.
    pub fn add_field(&mut self, class: &str, name: &str, offset: i32, kind: &str, type_name: Option<&str>, inner: Option<&str>) {
        if let Some(c) = self.classes.get_mut(class) {
            c.fields.push(Field {
                name: name.to_string(),
                offset,
                ty: FieldType {
                    kind: kind.to_string(),
                    name: type_name.map(|s| s.to_string()),
                    inner: inner.map(|s| s.to_string()),
                },
            });
        }
    }

    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// Serialize to pretty JSON (stable order). Returns "{}" on the (impossible) serialization error.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.classes).unwrap_or_else(|_| "{}".to_string())
    }
}

impl Default for Catalog {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn built() -> Catalog {
        let mut c = Catalog::new();
        c.add_class("CEntity", Some("CBaseType"));
        c.add_field("CEntity", "m_iValue", 844, "atomic", Some("int32"), None);
        c.add_field("CEntity", "m_hReference", 812, "handle", None, Some("CEntityRef"));
        c.add_class("CBaseType", None); // root: no parent
        c.add_field("CBaseType", "m_vPosition", 300, "class", Some("Vector"), None);
        c
    }

    #[test]
    fn serializes_classes_fields_and_types() {
        let v: Value = serde_json::from_str(&built().to_json()).unwrap();
        assert_eq!(v["CEntity"]["parent"], "CBaseType");
        let f0 = &v["CEntity"]["fields"][0];
        assert_eq!(f0["name"], "m_iValue");
        assert_eq!(f0["offset"], 844);
        assert_eq!(f0["type"]["kind"], "atomic");
        assert_eq!(f0["type"]["name"], "int32");
        assert!(f0["type"].get("inner").is_none(), "atomic has no inner");
        let f1 = &v["CEntity"]["fields"][1];
        assert_eq!(f1["type"]["kind"], "handle");
        assert_eq!(f1["type"]["inner"], "CEntityRef");
        assert!(f1["type"].get("name").is_none(), "handle has no name");
    }

    #[test]
    fn root_class_omits_parent() {
        let v: Value = serde_json::from_str(&built().to_json()).unwrap();
        assert!(v["CBaseType"].get("parent").is_none(), "root class has no parent key");
    }

    #[test]
    fn output_is_deterministic_across_identical_builds() {
        // classes sorted (BTreeMap); fields in insertion order — a stable committed file.
        assert_eq!(built().to_json(), built().to_json());
    }

    #[test]
    fn add_field_to_unknown_class_is_defensive_no_panic() {
        let mut c = Catalog::new();
        c.add_field("CNeverAdded", "x", 0, "atomic", Some("int32"), None); // must not panic
        // The field is dropped (no class) — degrade, not crash.
        assert_eq!(c.class_count(), 0);
    }

    #[test]
    fn unknown_kind_round_trips() {
        let mut c = Catalog::new();
        c.add_class("C", None);
        c.add_field("C", "weird", 4, "unknown", Some("SomeExoticType"), None);
        let v: Value = serde_json::from_str(&c.to_json()).unwrap();
        assert_eq!(v["C"]["fields"][0]["type"]["kind"], "unknown");
        assert_eq!(v["C"]["fields"][0]["type"]["name"], "SomeExoticType");
    }
}

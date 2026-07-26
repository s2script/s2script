//! Pure, engine-generic schema catalog builder (V8-free, no CS2 identifiers). The live SDK walk lives
//! in the shim; it streams classes/fields here via v8host's C-ABI callbacks. This module only
//! assembles + serializes — so it is fully unit-testable without an engine.

use serde::Serialize;
use std::collections::BTreeMap;

/// A field's type. `kind` ∈ atomic | handle | class | ptr | enum | unknown (the shim maps the
/// CSchemaType category → this string). `name` = the type name for atomic/class/enum; `inner` = the
/// referenced class for handle/ptr; `size` = byte width where the SchemaSystem states it (enums —
/// the category names the type but not its width, and codegen cannot pick a reader without one).
/// Absent fields are omitted from JSON.
#[derive(Serialize)]
pub struct FieldType {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u8>,
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

/// One declared enum: its byte width plus its enumerators.
///
/// Kept SEPARATE from `Catalog`'s class map rather than folded into it. The catalog serializes as a
/// bare `{ className: {...} }` object, so adding a sibling section would change that shape and break
/// every existing consumer; enums get their own file instead.
#[derive(Serialize)]
pub struct EnumDef {
    pub size: u8,
    /// Enumerator name -> value. BTreeMap for deterministic output; values are i64 because the
    /// schema stores them as int64 and flag enums legitimately use the high bit.
    pub values: BTreeMap<String, i64>,
}

/// The catalog. Classes are keyed in a BTreeMap for deterministic (sorted) output; fields keep
/// insertion order (the shim emits them in schema order, which is stable per binary).
pub struct Catalog {
    classes: BTreeMap<String, Class>,
    enums: BTreeMap<String, EnumDef>,
}

impl Catalog {
    pub fn new() -> Self {
        Self { classes: BTreeMap::new(), enums: BTreeMap::new() }
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
    pub fn add_field(&mut self, class: &str, name: &str, offset: i32, kind: &str, type_name: Option<&str>, inner: Option<&str>, size: i32) {
        if let Some(c) = self.classes.get_mut(class) {
            c.fields.push(Field {
                name: name.to_string(),
                offset,
                ty: FieldType {
                    kind: kind.to_string(),
                    name: type_name.map(|s| s.to_string()),
                    inner: inner.map(|s| s.to_string()),
                    // 0 = the SchemaSystem did not state a width; omitted rather than recorded as
                    // zero, so a consumer cannot mistake "unknown" for "zero-sized".
                    size: if (1..=8).contains(&size) { Some(size as u8) } else { None },
                },
            });
        }
    }

    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// Serialize to pretty JSON (stable order). Returns "{}" on the (impossible) serialization error.
    /// Record one enumerator. Idempotent per (enum, enumerator): the shim walks two type scopes and
    /// an enum registered in both would otherwise be merged twice — harmless for values, but it would
    /// hide a genuine disagreement, so the first write wins and a repeat is ignored.
    pub fn add_enum(&mut self, name: &str, size: i32, enumerator: &str, value: i64) {
        if name.is_empty() || enumerator.is_empty() { return; }
        // Same rule as a field's width: only a stated 1/2/4/8 is trusted.
        if !(1..=8).contains(&size) { return; }
        self.enums
            .entry(name.to_string())
            .or_insert_with(|| EnumDef { size: size as u8, values: BTreeMap::new() })
            .values
            .entry(enumerator.to_string())
            .or_insert(value);
    }

    /// The enum table, serialized. Separate artifact from {@link Catalog::to_json}.
    pub fn enums_json(&self) -> String {
        serde_json::to_string_pretty(&self.enums).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn enum_count(&self) -> usize { self.enums.len() }

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
        c.add_field("CEntity", "m_iValue", 844, "atomic", Some("int32"), None, 0);
        c.add_field("CEntity", "m_hReference", 812, "handle", None, Some("CEntityRef"), 0);
        c.add_class("CBaseType", None); // root: no parent
        c.add_field("CBaseType", "m_vPosition", 300, "class", Some("Vector"), None, 0);
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
    fn enum_table_dedupes_and_rejects_an_unstated_width() {
        let mut c = Catalog::new();
        c.add_enum("MoveType_t", 1, "MOVETYPE_NONE", 0);
        c.add_enum("MoveType_t", 1, "MOVETYPE_FLY", 5);
        // The shim walks two type scopes; an enum registered in both must not merge twice. First
        // write wins, so a genuine disagreement stays visible rather than being silently overwritten.
        c.add_enum("MoveType_t", 1, "MOVETYPE_FLY", 999);
        // Flag enums legitimately use the high bit, so a negative value must survive.
        c.add_enum("Flags_t", 4, "FLAG_SIGNBIT", -2147483648);
        // Unstated / out-of-range width is not trusted, exactly as for a field.
        c.add_enum("Bogus_t", 0, "X", 1);
        c.add_enum("Bogus_t", 99, "Y", 2);
        c.add_enum("", 1, "Anon", 1);
        c.add_enum("Named_t", 1, "", 1);

        let j = c.enums_json();
        assert!(j.contains("MOVETYPE_FLY"), "{j}");
        assert!(j.contains("5"), "{j}");
        assert!(!j.contains("999"), "a repeat enumerator must not overwrite: {j}");
        assert!(j.contains("-2147483648"), "negative enumerator lost: {j}");
        assert!(!j.contains("Bogus_t"), "enum with no stated width must be dropped: {j}");
        assert!(!j.contains("Anon"), "{j}");
        assert!(!j.contains("Named_t"), "{j}");
        assert_eq!(c.enum_count(), 2);
    }

    #[test]
    fn enum_width_is_recorded_and_only_when_stated() {
        let mut c = Catalog::new();
        c.add_class("CEntity", None);
        // A stated width is carried through so codegen can pick a reader.
        c.add_field("CEntity", "m_iTeamNum", 8, "enum", Some("Team_t"), None, 1);
        c.add_field("CEntity", "m_nMoveType", 12, "enum", Some("MoveType_t"), None, 4);
        // 0 means the SchemaSystem did not state one; it must be OMITTED, not recorded as zero, so
        // a consumer cannot read "unknown" as "zero-sized".
        c.add_field("CEntity", "m_nUnbound", 16, "enum", Some("Mystery_t"), None, 0);
        // Out-of-range is treated the same way rather than trusted.
        c.add_field("CEntity", "m_nBogus", 20, "enum", Some("Bogus_t"), None, 99);
        let j = c.to_json();
        assert!(j.contains("\"size\": 1"), "1-byte enum width missing: {j}");
        assert!(j.contains("\"size\": 4"), "4-byte enum width missing: {j}");
        assert!(!j.contains("\"size\": 0"), "unstated width must be omitted: {j}");
        assert!(!j.contains("\"size\": 99"), "out-of-range width must be omitted: {j}");
    }

    #[test]
    fn add_field_to_unknown_class_is_defensive_no_panic() {
        let mut c = Catalog::new();
        c.add_field("CNeverAdded", "x", 0, "atomic", Some("int32"), None, 0); // must not panic
        // The field is dropped (no class) — degrade, not crash.
        assert_eq!(c.class_count(), 0);
    }

    #[test]
    fn unknown_kind_round_trips() {
        let mut c = Catalog::new();
        c.add_class("C", None);
        c.add_field("C", "weird", 4, "unknown", Some("SomeExoticType"), None, 0);
        let v: Value = serde_json::from_str(&c.to_json()).unwrap();
        assert_eq!(v["C"]["fields"][0]["type"]["kind"], "unknown");
        assert_eq!(v["C"]["fields"][0]["type"]["name"], "SomeExoticType");
    }
}

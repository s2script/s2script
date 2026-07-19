//! Config materialization (Slice 5E.2 + sections): merge declared defaults with an admin override
//! JSON.  Engine-generic, V8-free, pure — no CS2 / no I/O (the caller reads the override file via a
//! shim op).
//!
//! A manifest `config` block is a tree of entries.  An entry is a value-DECL iff it has a `type` key
//! whose value is one of `string|int|float|bool`; otherwise it is a SECTION (a nested map of
//! entries, recursed).  `.` is banned in a decl key name — dotted access is a section walk, so a
//! literal `.` in a key would be unreachable from the getters.
use serde::Deserialize;
use std::collections::HashMap;

/// One config entry: either a value declaration or a nested section of further entries.
/// `#[serde(untagged)]` tries `Decl` first — it requires BOTH a `type` and a `default` key, so any
/// object lacking a string-valued `type` (or lacking `default`) falls through to `Section` and is
/// recursed.  Edge: a section whose only child is literally named `"type"` has that child as an
/// *object* value, so the `Decl` match fails on the string field and the parent classifies as a
/// section — the intended behavior.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ConfigEntry {
    Decl(ConfigDecl),
    Section(HashMap<String, ConfigEntry>),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigDecl {
    pub r#type: String,
    pub default: serde_json::Value,
    #[serde(default)]
    pub description: Option<String>,
    /// Numeric range bounds (int/float only; ignored for string/bool). Mutually exclusive with `enum`.
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// Allowed values (string/int only). Mutually exclusive with `min`/`max`.
    #[serde(default, rename = "enum")]
    pub r#enum: Option<Vec<serde_json::Value>>,
    /// Presentation hints for a future registry UI (never affect materialization).
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// Masked in registry display but STILL written verbatim to the operator's config file.
    #[serde(default)]
    pub sensitive: bool,
}

pub struct MaterializeResult {
    pub values: serde_json::Map<String, serde_json::Value>,
    pub warnings: Vec<String>,
}

fn value_matches_type(v: &serde_json::Value, ty: &str) -> bool {
    match ty {
        "string" => v.is_string(),
        "int" => v.as_i64().map_or(false, |_| v.is_i64() || v.is_u64()) && !v.is_f64(),
        "float" => v.is_number(),
        "bool" => v.is_boolean(),
        _ => false,
    }
}

fn zero_value(ty: &str) -> serde_json::Value {
    match ty {
        "string" => serde_json::json!(""),
        "int" | "float" => serde_json::json!(0),
        "bool" => serde_json::json!(false),
        _ => serde_json::Value::Null,
    }
}

/// Does `v` satisfy the decl's range / enum constraints?  `enum` takes precedence (it is mutually
/// exclusive with min/max by contract; if both are somehow present, enum wins).  A non-numeric value
/// under a min/max decl passes the range check (type-mismatch is caught separately).
fn satisfies_constraints(v: &serde_json::Value, decl: &ConfigDecl) -> bool {
    if let Some(allowed) = &decl.r#enum {
        return allowed.iter().any(|a| a == v);
    }
    if decl.min.is_some() || decl.max.is_some() {
        if let Some(n) = v.as_f64() {
            if let Some(mn) = decl.min {
                if n < mn {
                    return false;
                }
            }
            if let Some(mx) = decl.max {
                if n > mx {
                    return false;
                }
            }
        }
    }
    true
}

/// Strip `//`-to-end-of-line comments (our auto-generated files use them). Quote-aware: a `//`
/// inside a JSON string (e.g. a `"http://..."` endpoint value) is NOT treated as a comment start —
/// only bare `//` outside of any string literal ends the line. Handles `\"` escapes.
pub(crate) fn strip_line_comments(s: &str) -> String {
    s.lines()
        .map(|l| {
            let bytes = l.as_bytes();
            let mut in_string = false;
            let mut escaped = false;
            let mut i = 0;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        in_string = false;
                    }
                } else if c == '"' {
                    in_string = true;
                } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '/' {
                    return &l[..i];
                }
                i += 1;
            }
            l
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Merge declared defaults with the override JSON (per-key, type + range/enum-checked, recursing
/// into sections). Never fails: a malformed override → all defaults; a wrong-typed / out-of-range /
/// not-in-enum override key → the default + a WARN; a bad default → the default / a zero-value + a
/// WARN; a decl key containing `.` → skipped + a WARN.  Sections produce a nested object of values.
pub fn materialize_config(entries: &HashMap<String, ConfigEntry>, override_json: Option<&str>) -> MaterializeResult {
    let overrides: serde_json::Map<String, serde_json::Value> = override_json
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&strip_line_comments(s)).ok())
        .and_then(|v| if let serde_json::Value::Object(m) = v { Some(m) } else { None })
        .unwrap_or_default();

    let mut values = serde_json::Map::new();
    let mut warnings = Vec::new();
    materialize_entries(entries, &overrides, "", &mut values, &mut warnings);
    MaterializeResult { values, warnings }
}

fn materialize_entries(
    entries: &HashMap<String, ConfigEntry>,
    overrides: &serde_json::Map<String, serde_json::Value>,
    prefix: &str,
    values: &mut serde_json::Map<String, serde_json::Value>,
    warnings: &mut Vec<String>,
) {
    for (key, entry) in entries {
        let fullkey = if prefix.is_empty() { key.clone() } else { format!("{}.{}", prefix, key) };
        if key.contains('.') {
            warnings.push(format!("config '{}': key contains '.' — skipped (dotted names are reserved for section access)", fullkey));
            continue;
        }
        match entry {
            ConfigEntry::Section(children) => {
                // A non-object override for a section is ignored — its children fall to defaults.
                let child_over = overrides.get(key).and_then(|v| v.as_object()).cloned().unwrap_or_default();
                let mut child_values = serde_json::Map::new();
                materialize_entries(children, &child_over, &fullkey, &mut child_values, warnings);
                values.insert(key.clone(), serde_json::Value::Object(child_values));
            }
            ConfigEntry::Decl(decl) => {
                let default_val = if value_matches_type(&decl.default, &decl.r#type) {
                    decl.default.clone()
                } else {
                    warnings.push(format!("config '{}': default does not match type '{}' — using zero-value", fullkey, decl.r#type));
                    zero_value(&decl.r#type)
                };
                let val = match overrides.get(key) {
                    Some(ov) if value_matches_type(ov, &decl.r#type) && satisfies_constraints(ov, decl) => ov.clone(),
                    Some(ov) if value_matches_type(ov, &decl.r#type) => {
                        warnings.push(format!("config '{}': override {} out of range / not in enum — using default", fullkey, ov));
                        default_val
                    }
                    Some(_) => {
                        warnings.push(format!("config '{}': override wrong type — using default", fullkey));
                        default_val
                    }
                    None => default_val,
                };
                values.insert(key.clone(), val);
            }
        }
    }
}

/// The auto-generated override file content: each declared key at its default, with a `//` comment
/// carrying its type + description; sections nest as `{ … }` blocks.  Deterministic (sorted keys) so
/// the file is stable across runs.  Dotted keys are skipped (they can never round-trip).
pub fn generate_default_jsonc(entries: &HashMap<String, ConfigEntry>) -> String {
    let mut out = String::from("{\n");
    gen_entries(entries, 1, &mut out);
    out.push_str("}\n");
    out
}

fn gen_entries(entries: &HashMap<String, ConfigEntry>, indent: usize, out: &mut String) {
    // Skip dotted keys BEFORE computing comma positions so the emitted JSONC stays valid.
    let mut keys: Vec<&String> = entries.keys().filter(|k| !k.contains('.')).collect();
    keys.sort();
    let pad = "  ".repeat(indent);
    for (i, key) in keys.iter().enumerate() {
        let comma = if i + 1 < keys.len() { "," } else { "" };
        match &entries[*key] {
            ConfigEntry::Decl(decl) => {
                let desc = decl.description.as_deref().unwrap_or("");
                out.push_str(&format!(
                    "{}// {}{}\n",
                    pad,
                    decl.r#type,
                    if desc.is_empty() { String::new() } else { format!(" — {}", desc) }
                ));
                out.push_str(&format!(
                    "{}{}: {}{}\n",
                    pad,
                    serde_json::to_string(key).unwrap(),
                    serde_json::to_string(&decl.default).unwrap(),
                    comma
                ));
            }
            ConfigEntry::Section(children) => {
                out.push_str(&format!("{}{}: {{\n", pad, serde_json::to_string(key).unwrap()));
                gen_entries(children, indent + 1, out);
                out.push_str(&format!("{}}}{}\n", pad, comma));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    fn decl(t: &str, d: serde_json::Value) -> ConfigDecl {
        ConfigDecl { r#type: t.into(), default: d, ..Default::default() }
    }
    fn dentry(t: &str, d: serde_json::Value) -> ConfigEntry {
        ConfigEntry::Decl(decl(t, d))
    }

    #[test]
    fn defaults_only_when_no_override() {
        let mut d = HashMap::new();
        d.insert("g".into(), dentry("string", "hi".into()));
        d.insert("n".into(), dentry("int", 3.into()));
        let r = materialize_config(&d, None);
        assert_eq!(r.values["g"], serde_json::json!("hi"));
        assert_eq!(r.values["n"], serde_json::json!(3));
        assert!(r.warnings.is_empty());
    }
    #[test]
    fn override_merges_and_wrong_type_falls_back() {
        let mut d = HashMap::new();
        d.insert("g".into(), dentry("string", "hi".into()));
        d.insert("n".into(), dentry("int", 3.into()));
        let r = materialize_config(&d, Some(r#"{ "g": "bye", "n": "notanint", "extra": 1 }"#));
        assert_eq!(r.values["g"], serde_json::json!("bye")); // override wins
        assert_eq!(r.values["n"], serde_json::json!(3)); // wrong type → default
        assert!(!r.values.contains_key("extra")); // undeclared ignored
        assert_eq!(r.warnings.len(), 1); // one WARN for n
    }
    #[test]
    fn malformed_override_uses_all_defaults() {
        let mut d = HashMap::new();
        d.insert("g".into(), dentry("string", "hi".into()));
        let r = materialize_config(&d, Some("{ this is not json"));
        assert_eq!(r.values["g"], serde_json::json!("hi"));
    }
    #[test]
    fn bad_default_degrades_to_zero_value_with_warn() {
        let mut d = HashMap::new();
        d.insert("n".into(), dentry("int", "notanint".into()));
        let r = materialize_config(&d, None);
        assert_eq!(r.values["n"], serde_json::json!(0));
        assert_eq!(r.warnings.len(), 1);
    }
    #[test]
    fn jsonc_comments_are_stripped() {
        let mut d = HashMap::new();
        d.insert("g".into(), dentry("string", "hi".into()));
        let r = materialize_config(&d, Some("{ // a comment\n \"g\": \"bye\" }"));
        assert_eq!(r.values["g"], serde_json::json!("bye"));
    }

    // --- sections + range/enum ---

    #[test]
    fn flat_manifest_parses_and_materializes_unchanged() {
        // A flat block deserializes into all-Decl entries — the pre-sections shape is unchanged.
        let j = serde_json::json!({
            "greeting": { "type": "string", "default": "hi" },
            "n": { "type": "int", "default": 3 }
        });
        let entries: HashMap<String, ConfigEntry> = serde_json::from_value(j).unwrap();
        assert!(matches!(entries["greeting"], ConfigEntry::Decl(_)));
        let r = materialize_config(&entries, None);
        assert_eq!(r.values["greeting"], serde_json::json!("hi"));
        assert_eq!(r.values["n"], serde_json::json!(3));
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn sectioned_manifest_materializes_nested() {
        let j = serde_json::json!({
            "top": { "type": "bool", "default": true },
            "sect": {
                "inner": { "type": "int", "default": 5 },
                "deeper": { "leaf": { "type": "string", "default": "x" } }
            }
        });
        let entries: HashMap<String, ConfigEntry> = serde_json::from_value(j).unwrap();
        assert!(matches!(entries["sect"], ConfigEntry::Section(_)));
        let r = materialize_config(&entries, None);
        assert_eq!(r.values["top"], serde_json::json!(true));
        assert_eq!(r.values["sect"]["inner"], serde_json::json!(5));
        assert_eq!(r.values["sect"]["deeper"]["leaf"], serde_json::json!("x"));
    }

    #[test]
    fn section_override_merges_nested() {
        let j = serde_json::json!({
            "sect": { "inner": { "type": "int", "default": 5 } }
        });
        let entries: HashMap<String, ConfigEntry> = serde_json::from_value(j).unwrap();
        let r = materialize_config(&entries, Some(r#"{ "sect": { "inner": 42 } }"#));
        assert_eq!(r.values["sect"]["inner"], serde_json::json!(42));
    }

    #[test]
    fn section_child_named_type_is_a_section() {
        // The section's only child is literally named "type" — the untagged match must NOT read the
        // parent as a Decl (its `type` value is an object, not a string).
        let j = serde_json::json!({
            "sect": { "type": { "type": "string", "default": "hello" } }
        });
        let entries: HashMap<String, ConfigEntry> = serde_json::from_value(j).unwrap();
        match &entries["sect"] {
            ConfigEntry::Section(children) => match &children["type"] {
                ConfigEntry::Decl(d) => assert_eq!(d.default, serde_json::json!("hello")),
                _ => panic!("child 'type' should be a Decl"),
            },
            _ => panic!("sect should be a Section"),
        }
        let r = materialize_config(&entries, None);
        assert_eq!(r.values["sect"]["type"], serde_json::json!("hello"));
    }

    #[test]
    fn out_of_range_override_falls_back_to_default_with_warn() {
        let mut d = HashMap::new();
        d.insert(
            "n".into(),
            ConfigEntry::Decl(ConfigDecl { r#type: "int".into(), default: serde_json::json!(5), min: Some(0.0), max: Some(10.0), ..Default::default() }),
        );
        let r = materialize_config(&d, Some(r#"{ "n": 99 }"#));
        assert_eq!(r.values["n"], serde_json::json!(5)); // out of range → default
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn in_range_override_is_kept() {
        let mut d = HashMap::new();
        d.insert(
            "n".into(),
            ConfigEntry::Decl(ConfigDecl { r#type: "int".into(), default: serde_json::json!(5), min: Some(0.0), max: Some(10.0), ..Default::default() }),
        );
        let r = materialize_config(&d, Some(r#"{ "n": 7 }"#));
        assert_eq!(r.values["n"], serde_json::json!(7));
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn enum_override_not_in_set_falls_back() {
        let mut d = HashMap::new();
        d.insert(
            "mode".into(),
            ConfigEntry::Decl(ConfigDecl {
                r#type: "string".into(),
                default: serde_json::json!("easy"),
                r#enum: Some(vec![serde_json::json!("easy"), serde_json::json!("hard")]),
                ..Default::default()
            }),
        );
        let ok = materialize_config(&d, Some(r#"{ "mode": "hard" }"#));
        assert_eq!(ok.values["mode"], serde_json::json!("hard"));
        let bad = materialize_config(&d, Some(r#"{ "mode": "insane" }"#));
        assert_eq!(bad.values["mode"], serde_json::json!("easy"));
        assert_eq!(bad.warnings.len(), 1);
    }

    #[test]
    fn dotted_key_decl_is_skipped_with_warn() {
        let mut d = HashMap::new();
        d.insert("a.b".to_string(), dentry("string", serde_json::json!("x")));
        d.insert("ok".to_string(), dentry("int", serde_json::json!(1)));
        let r = materialize_config(&d, None);
        assert!(!r.values.contains_key("a.b"));
        assert_eq!(r.values["ok"], serde_json::json!(1));
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn generate_default_jsonc_nested_sections_reparse() {
        let j = serde_json::json!({
            "top": { "type": "bool", "default": true, "description": "toggle" },
            "sect": { "inner": { "type": "int", "default": 5 } }
        });
        let entries: HashMap<String, ConfigEntry> = serde_json::from_value(j).unwrap();
        let jsonc = generate_default_jsonc(&entries);
        // The generated JSONC round-trips through the comment-stripper + materialize back to defaults.
        let r = materialize_config(&entries, Some(&jsonc));
        assert_eq!(r.values["top"], serde_json::json!(true));
        assert_eq!(r.values["sect"]["inner"], serde_json::json!(5));
        assert!(r.warnings.is_empty());
    }
}

//! Config materialization (Slice 5E.2): merge declared defaults with an admin override JSON.
//! Engine-generic, V8-free, pure — no CS2 / no I/O (the caller reads the override file via a shim op).
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigDecl {
    pub r#type: String,
    pub default: serde_json::Value,
    #[serde(default)]
    pub description: Option<String>,
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

/// Strip `//`-to-end-of-line comments (our auto-generated files use them). Naive but safe here — our
/// values never contain `//` (string values could; a `//` inside a JSON string would be mis-stripped,
/// which is acceptable for a config file and matches the shim's gamedata JSONC handling).
fn strip_line_comments(s: &str) -> String {
    s.lines().map(|l| match l.find("//") { Some(i) => &l[..i], None => l }).collect::<Vec<_>>().join("\n")
}

/// Merge declared defaults with the override JSON (per-key, type-checked). Never fails: a malformed
/// override → all defaults; a wrong-typed override key or a bad default → the default / a zero-value + a WARN.
pub fn materialize_config(decls: &HashMap<String, ConfigDecl>, override_json: Option<&str>) -> MaterializeResult {
    let overrides: serde_json::Map<String, serde_json::Value> = override_json
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&strip_line_comments(s)).ok())
        .and_then(|v| if let serde_json::Value::Object(m) = v { Some(m) } else { None })
        .unwrap_or_default();

    let mut values = serde_json::Map::new();
    let mut warnings = Vec::new();
    for (key, decl) in decls {
        let default_val = if value_matches_type(&decl.default, &decl.r#type) {
            decl.default.clone()
        } else {
            warnings.push(format!("config '{}': default does not match type '{}' — using zero-value", key, decl.r#type));
            zero_value(&decl.r#type)
        };
        let val = match overrides.get(key) {
            Some(ov) if value_matches_type(ov, &decl.r#type) => ov.clone(),
            Some(_) => { warnings.push(format!("config '{}': override wrong type — using default", key)); default_val }
            None => default_val,
        };
        values.insert(key.clone(), val);
    }
    MaterializeResult { values, warnings }
}

/// The auto-generated override file content: each declared key at its default, with a `//` comment
/// carrying its type + description. Deterministic (sorted keys) so the file is stable across runs.
pub fn generate_default_jsonc(decls: &HashMap<String, ConfigDecl>) -> String {
    let mut keys: Vec<&String> = decls.keys().collect();
    keys.sort();
    let mut out = String::from("{\n");
    for (i, key) in keys.iter().enumerate() {
        let decl = &decls[*key];
        let desc = decl.description.as_deref().unwrap_or("");
        out.push_str(&format!("  // {}{}\n", decl.r#type, if desc.is_empty() { String::new() } else { format!(" — {}", desc) }));
        let comma = if i + 1 < keys.len() { "," } else { "" };
        out.push_str(&format!("  {}: {}{}\n", serde_json::to_string(key).unwrap(),
            serde_json::to_string(&decl.default).unwrap(), comma));
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    fn decl(t: &str, d: serde_json::Value) -> ConfigDecl { ConfigDecl { r#type: t.into(), default: d, description: None } }

    #[test]
    fn defaults_only_when_no_override() {
        let mut d = HashMap::new();
        d.insert("g".into(), decl("string", "hi".into()));
        d.insert("n".into(), decl("int", 3.into()));
        let r = materialize_config(&d, None);
        assert_eq!(r.values["g"], serde_json::json!("hi"));
        assert_eq!(r.values["n"], serde_json::json!(3));
        assert!(r.warnings.is_empty());
    }
    #[test]
    fn override_merges_and_wrong_type_falls_back() {
        let mut d = HashMap::new();
        d.insert("g".into(), decl("string", "hi".into()));
        d.insert("n".into(), decl("int", 3.into()));
        let r = materialize_config(&d, Some(r#"{ "g": "bye", "n": "notanint", "extra": 1 }"#));
        assert_eq!(r.values["g"], serde_json::json!("bye"));   // override wins
        assert_eq!(r.values["n"], serde_json::json!(3));        // wrong type → default
        assert!(!r.values.contains_key("extra"));              // undeclared ignored
        assert_eq!(r.warnings.len(), 1);                       // one WARN for n
    }
    #[test]
    fn malformed_override_uses_all_defaults() {
        let mut d = HashMap::new();
        d.insert("g".into(), decl("string", "hi".into()));
        let r = materialize_config(&d, Some("{ this is not json"));
        assert_eq!(r.values["g"], serde_json::json!("hi"));
    }
    #[test]
    fn bad_default_degrades_to_zero_value_with_warn() {
        let mut d = HashMap::new();
        d.insert("n".into(), decl("int", "notanint".into()));
        let r = materialize_config(&d, None);
        assert_eq!(r.values["n"], serde_json::json!(0));
        assert_eq!(r.warnings.len(), 1);
    }
    #[test]
    fn jsonc_comments_are_stripped() {
        let mut d = HashMap::new();
        d.insert("g".into(), decl("string", "hi".into()));
        let r = materialize_config(&d, Some("{ // a comment\n \"g\": \"bye\" }"));
        assert_eq!(r.values["g"], serde_json::json!("bye"));
    }
}

use super::contract::{self, NormalizedBundle, NormalizedTarget, Validator};
use super::provenance::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
pub const MAX_FILES: usize = 64;
pub const MAX_FILE_BYTES: usize = 256 * 1024;
pub const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRecord {
    pub relative_path: String,
    pub sha256: String,
    pub content: String,
}
/// An owned snapshot. No live filesystem references survive preparation.
#[derive(Clone, Debug)]
pub struct OverrideSet {
    records: Vec<SnapshotRecord>,
}
impl From<Vec<SnapshotRecord>> for OverrideSet {
    fn from(records: Vec<SnapshotRecord>) -> Self {
        Self { records }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    records: Vec<SnapshotRecord>,
    error: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OverrideFile {
    schema_version: u32,
    owner_id: String,
    #[serde(deserialize_with = "unique_functions")]
    functions: BTreeMap<String, CheckedValue>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    contract_hash: String,
    target: Value,
    #[serde(default, deserialize_with = "contract::present")]
    resolve: Option<String>,
    #[serde(default, deserialize_with = "contract::present")]
    supersedes: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Signature {
    #[serde(default, deserialize_with = "contract::present")]
    kind: Option<String>,
    module: String,
    pattern: String,
    validate: Validator,
    #[serde(default, deserialize_with = "contract::present")]
    target_validate: Option<Validator>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Virtual {
    kind: String,
    module: String,
    class: String,
    index: u32,
    validate: Validator,
}
fn target(value: Value, resolve: Option<String>) -> Result<NormalizedTarget, String> {
    let resolve = resolve.unwrap_or_else(|| "direct".into());
    let t = if value.get("kind").and_then(Value::as_str) == Some("virtual") {
        let t: Virtual = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if t.kind != "virtual" {
            return Err("invalid kind".into());
        }
        NormalizedTarget::Virtual {
            module: t.module,
            class: t.class,
            index: t.index,
            resolve,
            derivation: "virtual-slot".into(),
            candidate_validate: Validator::default(),
            target_validate: t.validate,
        }
    } else {
        let t: Signature = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if t.kind.as_deref().is_some_and(|k| k != "signature") || t.validate.empty() {
            return Err("invalid signature/empty validator".into());
        }
        let derivation = match resolve.as_str() {
            "direct" => "identity",
            "ctor-body-xref" => "ctor-body-xref",
            "lea-disp" => "lea-disp",
            "validated-call" => "e8-rel32",
            _ => return Err("unsupported resolver".into()),
        };
        if resolve != "validated-call" && t.target_validate.is_some() {
            return Err("targetValidate requires validated-call".into());
        }
        if t.target_validate.as_ref().is_some_and(Validator::empty) {
            return Err("empty targetValidate".into());
        }
        let (candidate_validate, target_validate) = if resolve == "validated-call" {
            (t.validate, t.target_validate.unwrap_or_default())
        } else {
            (Validator::default(), t.validate)
        };
        NormalizedTarget::Signature {
            module: t.module,
            pattern: t.pattern,
            resolve,
            derivation: derivation.into(),
            candidate_validate,
            target_validate,
        }
    };
    t.validate()?;
    Ok(t)
}
// Strip both JSONC comment forms without changing quoted strings or swallowing invalid syntax.
fn jsonc(text: &str) -> Result<String, String> {
    let mut out = text.as_bytes().to_vec();
    let b = text.as_bytes();
    let mut i = 0;
    let mut string = false;
    while i < b.len() {
        if string {
            if b[i] == b'\\' {
                i += 2;
                continue;
            }
            if b[i] == b'"' {
                string = false;
            }
            i += 1;
            continue;
        }
        if b[i] == b'"' {
            string = true;
            i += 1;
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
            continue;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            let mut closed = false;
            while i < b.len() {
                if i + 1 < b.len() && b[i] == b'*' && b[i + 1] == b'/' {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    closed = true;
                    break;
                }
                if b[i] != b'\n' && b[i] != b'\r' {
                    out[i] = b' ';
                }
                i += 1;
            }
            if !closed {
                return Err("unterminated JSONC comment".into());
            }
            continue;
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}
// Root fields are checked by OverrideFile's derive. Function names need their own
// duplicate gate because a normal BTreeMap deserializer silently replaces entries.
fn unique_functions<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, CheckedValue>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = BTreeMap<String, CheckedValue>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an object with unique function names")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut a: A,
        ) -> Result<Self::Value, A::Error> {
            let mut functions = BTreeMap::new();
            while let Some((name, value)) = a.next_entry::<String, CheckedValue>()? {
                match functions.entry(name) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(value);
                    }
                    std::collections::btree_map::Entry::Occupied(entry) => {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate function name {}",
                            entry.key()
                        )));
                    }
                }
            }
            Ok(functions)
        }
    }
    d.deserialize_map(Visitor)
}

// A syntactically valid subtree belongs to one function once the enclosing unique
// name and owner have been validated. Carry duplicate errors to that function's
// failure handling rather than refusing unrelated optional entries. The first
// value is retained internally, but no value from a duplicate-bearing subtree may
// enter target parsing or be applied.
struct CheckedValue {
    value: Value,
    duplicate: Option<String>,
}
impl CheckedValue {
    fn new(value: Value) -> Self {
        Self {
            value,
            duplicate: None,
        }
    }
    fn into_result(self) -> Result<Value, String> {
        match self.duplicate {
            Some(reason) => Err(reason),
            None => Ok(self.value),
        }
    }
}
impl<'de> Deserialize<'de> for CheckedValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = CheckedValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON with duplicate-key diagnostics")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<CheckedValue, E> {
                Ok(CheckedValue::new(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<CheckedValue, E> {
                Ok(CheckedValue::new(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<CheckedValue, E> {
                Ok(CheckedValue::new(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<CheckedValue, E> {
                Ok(CheckedValue::new(serde_json::json!(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<CheckedValue, E> {
                Ok(CheckedValue::new(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<CheckedValue, E> {
                Ok(CheckedValue::new(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<CheckedValue, A::Error> {
                let mut values = vec![];
                let mut duplicate = None;
                while let Some(child) = a.next_element::<CheckedValue>()? {
                    duplicate = duplicate.or(child.duplicate);
                    values.push(child.value);
                }
                Ok(CheckedValue {
                    value: Value::Array(values),
                    duplicate,
                })
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<CheckedValue, A::Error> {
                let mut values = serde_json::Map::new();
                let mut duplicate = None;
                while let Some((key, child)) = a.next_entry::<String, CheckedValue>()? {
                    duplicate = duplicate.or(child.duplicate);
                    match values.entry(key) {
                        serde_json::map::Entry::Vacant(entry) => {
                            entry.insert(child.value);
                        }
                        serde_json::map::Entry::Occupied(entry) => {
                            duplicate
                                .get_or_insert_with(|| format!("duplicate key {}", entry.key()));
                        }
                    }
                }
                Ok(CheckedValue {
                    value: Value::Object(values),
                    duplicate,
                })
            }
        }
        d.deserialize_any(Visitor)
    }
}
pub fn snapshot(owner: &str) -> Result<OverrideSet, String> {
    let op = crate::v8host::engine_ops()
        .and_then(|o| o.plugin_function_overrides)
        .ok_or("engine function override snapshot op unavailable")?;
    let owner = std::ffi::CString::new(owner).map_err(|_| "NUL owner")?;
    let ptr = op(owner.as_ptr());
    if ptr.is_null() {
        return Err("override snapshot failed".into());
    }
    // Shim owns this transient main-thread buffer. Copy before another engine operation.
    let text = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| "invalid UTF-8 snapshot")?;
    let result: Snapshot = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if let Some(error) = result.error {
        return Err(error);
    }
    Ok(result.records.into())
}
fn plugin_directory(owner: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::from("id-");
    let mut value = 0u32;
    let mut bits = 0;
    for byte in owner.bytes() {
        value = (value << 8) | u32::from(byte);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            encoded.push(ALPHABET[((value >> bits) & 63) as usize] as char);
        }
    }
    if bits > 0 {
        encoded.push(ALPHABET[((value << (6 - bits)) & 63) as usize] as char);
    }
    encoded
}

pub fn prepare(
    base: NormalizedBundle,
    archive_hash: &str,
    records: impl Into<OverrideSet>,
) -> Result<PreparedCandidate, String> {
    let records = records.into().records;
    if records.len() > MAX_FILES {
        return Err("override file count limit".into());
    }
    let mut functions: Vec<PreparedFunction> = base
        .functions
        .iter()
        .cloned()
        .map(|function| PreparedFunction {
            provenance: Provenance {
                archive_hash: archive_hash.into(),
                base_contract_hash: function.contract_hash.clone(),
                overrides: vec![],
                final_target_hash: contract::hash(&serde_json::to_value(&function.target).unwrap()),
                resolver_result: ValidationResult::Pending,
                validator_result: ValidationResult::Pending,
                required: function.requirement == "required",
            },
            function,
            unavailable: None,
        })
        .collect();
    let by_name: BTreeMap<_, _> = functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.function.local_name.clone(), i))
        .collect();
    let prefix = format!(
        "gamedata/plugins/{}/custom/",
        plugin_directory(&base.owner_id)
    );
    let mut previous = "";
    let mut total = 0;
    let mut touched = BTreeMap::<String, String>::new();
    for record in &records {
        total += record.content.len();
        let components: Vec<_> = record.relative_path.split('/').collect();
        if record.relative_path.as_str() <= previous
            || !record.relative_path.starts_with(&prefix)
            || components.len() != 5
            || components.iter().any(|c| {
                c.is_empty() || *c == "." || *c == ".." || c.contains('\\') || c.contains('\0')
            })
            || !record.relative_path.ends_with(".jsonc")
            || record.content.len() > MAX_FILE_BYTES
            || total > MAX_TOTAL_BYTES
            || contract::hash_bytes(record.content.as_bytes()) != record.sha256
        {
            return Err("invalid/unsorted/oversized override snapshot or hash".into());
        }
        previous = &record.relative_path;
        let file: OverrideFile = serde_json::from_str(&jsonc(&record.content)?)
            .map_err(|e| format!("{}: unattributable override: {e}", record.relative_path))?;
        if file.schema_version != 2
            || file.owner_id != base.owner_id
            || file
                .functions
                .keys()
                .any(|name| !by_name.contains_key(name))
        {
            return Err(format!("{}: unattributable override", record.relative_path));
        }
        for (name, value) in file.functions {
            let f = &mut functions[by_name[&name]];
            let result = (|| {
                let entry: Entry =
                    serde_json::from_value(value.into_result()?).map_err(|e| e.to_string())?;
                if entry.contract_hash != f.function.contract_hash {
                    return Err("stale base contract hash".into());
                }
                if entry.supersedes.as_ref() != touched.get(&name) {
                    return Err("conflicting override: exact supersedes required".into());
                }
                target(entry.target, entry.resolve)
            })();
            touched.insert(name, record.relative_path.clone());
            match result {
                Ok(target) => {
                    f.function.target = target;
                    f.provenance.overrides.push(OverrideProvenance {
                        relative_path: record.relative_path.clone(),
                        sha256: record.sha256.clone(),
                    });
                    f.provenance.final_target_hash =
                        contract::hash(&serde_json::to_value(&f.function.target).unwrap());
                }
                Err(reason) => {
                    f.unavailable = Some(format!("{}: {reason}", record.relative_path));
                }
            }
        }
    }
    for f in &mut functions {
        if let Some(reason) = &f.unavailable {
            if f.provenance.required {
                return Err(format!(
                    "{}: required function unavailable: {reason}",
                    f.function.canonical_id
                ));
            }
            f.provenance.resolver_result = ValidationResult::Unavailable(reason.clone());
            f.provenance.validator_result = ValidationResult::Unavailable(reason.clone());
        }
    }
    Ok(PreparedCandidate { base, functions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_functions::tests::{fixture, seal, summary};
    use serde_json::json;

    fn bundle(required: bool) -> NormalizedBundle {
        let mut value = fixture();
        let mut other = value["functions"][0].clone();
        other["localName"] = "other".into();
        other["canonicalId"] = "@demo/fire::other".into();
        value["functions"].as_array_mut().unwrap().push(other);
        if required {
            value["functions"][0]["requirement"] = "required".into();
        }
        seal(&mut value);
        contract::parse(
            &value.to_string(),
            "@demo/fire",
            &summary(&value),
            &["engine:calls".into()],
        )
        .unwrap()
    }

    fn record(content: String) -> SnapshotRecord {
        SnapshotRecord {
            relative_path: "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc".into(),
            sha256: contract::hash_bytes(content.as_bytes()),
            content,
        }
    }

    fn override_text(base: &NormalizedBundle) -> String {
        json!({
            "schemaVersion": 2,
            "ownerId": "@demo/fire",
            "functions": {
                "fire": {"contractHash": base.functions[0].contract_hash,
                    "target": {"module":"server", "pattern":"55", "validate":{"prologue":"55"}}},
                "other": {"contractHash": base.functions[1].contract_hash,
                    "target": {"module":"server", "pattern":"66", "validate":{"prologue":"66"}}}
            }
        })
        .to_string()
    }

    #[test]
    fn nested_duplicate_key_disables_only_attributed_optional_function() {
        let base = bundle(false);
        let content = override_text(&base).replacen(
            "\"prologue\":\"55\"",
            "\"prologue\":\"55\",\"prologue\":\"56\"",
            1,
        );
        let candidate = prepare(base, "archive", vec![record(content)])
            .expect("unique optional entry is attributable");
        let fire = &candidate.functions()[0];
        assert!(fire
            .unavailable()
            .unwrap()
            .contains("duplicate key prologue"));
        assert!(
            fire.provenance().overrides.is_empty(),
            "malformed entry must never apply either duplicate value"
        );
        let other = &candidate.functions()[1];
        assert!(other.unavailable().is_none());
        assert_eq!(other.provenance().overrides.len(), 1);
        match &other.function().target {
            NormalizedTarget::Signature { pattern, .. } => assert_eq!(pattern, "66"),
            _ => panic!("expected signature"),
        }
    }

    #[test]
    fn nested_duplicate_key_in_required_function_refuses_candidate_by_name() {
        let base = bundle(true);
        let content = override_text(&base).replacen(
            "\"prologue\":\"55\"",
            "\"prologue\":\"55\",\"prologue\":\"56\"",
            1,
        );
        let reason = prepare(base, "archive", vec![record(content)]).unwrap_err();
        assert!(
            reason.contains("@demo/fire::fire: required function unavailable"),
            "{reason}"
        );
        assert!(reason.contains("duplicate key prologue"), "{reason}");
    }

    #[test]
    fn duplicate_attribution_boundaries_still_refuse_candidate() {
        let base = bundle(false);
        let content = override_text(&base);
        for (before, after) in [
            (
                "\"schemaVersion\":2",
                "\"schemaVersion\":2,\"schemaVersion\":2",
            ),
            (
                "\"ownerId\":\"@demo/fire\"",
                "\"ownerId\":\"@demo/fire\",\"ownerId\":\"@demo/fire\"",
            ),
            ("\"functions\":{", "\"functions\":{},\"functions\":{"),
            ("\"fire\":{", "\"fire\":{},\"fire\":{"),
        ] {
            let malformed = content.replacen(before, after, 1);
            assert_ne!(content, malformed, "fixture must contain {before}");
            let reason = prepare(base.clone(), "archive", vec![record(malformed)]).unwrap_err();
            assert!(reason.contains("duplicate"), "{before}: {reason}");
        }
    }
}

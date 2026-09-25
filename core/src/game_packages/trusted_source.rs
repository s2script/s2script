//! Materializes a selected package's sealed trusted-function source into the activation artifact
//! `engine_functions::trusted::decode` accepts, at package commit.
//!
//! Engine-generic: nothing here knows a game. The package names each function's target by its
//! gamedata signature NAME and each record field by an opaque offset key bound to a schema
//! (class, field) pair. The host chooses the layout: the target comes from the MERGED shipped +
//! operator-custom gamedata (so an owner `custom/` signature repair applies, and is recorded as
//! provenance), and each offset comes from the live schema, falling back to the schema-catalog
//! value the package was built from. A function whose signature is missing or malformed in the
//! merged view degrades by name (optional) instead of failing the package.
use super::repair_snapshot;
use crate::engine_functions::contract::{self, NormalizedTarget};
use crate::engine_functions::instance::{RecordField, RecordLayout};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const GAMEDATA_PLATFORM: &str = "linuxsteamrt64";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Source {
    schema_version: u32,
    owner_id: String,
    offsets: BTreeMap<String, OffsetSource>,
    functions: Vec<FunctionSource>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OffsetSource {
    class: String,
    field: String,
    catalog: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FunctionSource {
    local_name: String,
    requirement: String,
    signature_name: String,
    signature: Map<String, Value>,
    policy: Value,
    #[serde(default, deserialize_with = "contract::present")]
    adapter: Option<Value>,
}

pub(super) struct Materialized {
    /// The exact `trusted::decode` wire bytes.
    pub bytes: Vec<u8>,
    /// Package-status provenance: selected offsets and per-function target provenance.
    pub status: Value,
}

fn fail(why: impl std::fmt::Display) -> String {
    format!("trusted functions source: {why}")
}

/// v1 gamedata signature entry -> normalized signature target (the SDK's `target()` mapping).
fn target(entry: &Value) -> Result<NormalizedTarget, String> {
    let spec = entry.as_object().ok_or("signature entry is not an object")?;
    for key in spec.keys() {
        if !["module", "pattern", "resolve", "validate", "targetValidate"].contains(&key.as_str()) {
            return Err(format!("unsupported signature field {key}"));
        }
    }
    let resolve = spec.get("resolve").and_then(Value::as_str).unwrap_or("direct");
    let derivation = match resolve {
        "direct" => "identity",
        "ctor-body-xref" => "ctor-body-xref",
        "lea-disp" => "lea-disp",
        "validated-call" => "e8-rel32",
        other => return Err(format!("unsupported resolver {other}")),
    };
    let validate = spec.get("validate").cloned().unwrap_or_else(|| json!({}));
    let (candidate, target_validate) = if resolve == "validated-call" {
        (validate, spec.get("targetValidate").cloned().unwrap_or_else(|| json!({})))
    } else {
        if spec.contains_key("targetValidate") {
            return Err("targetValidate is only supported with validated-call".into());
        }
        (json!({}), validate)
    };
    let value = json!({"kind":"signature","module":spec.get("module"),"pattern":spec.get("pattern"),
        "resolve":resolve,"derivation":derivation,"candidateValidate":candidate,"targetValidate":target_validate});
    let normalized: NormalizedTarget = serde_json::from_value(value).map_err(|e| e.to_string())?;
    normalized.validate()?;
    Ok(normalized)
}

/// Fills every record instance's field offsets from the selected map and derives its minimal
/// aligned extent and layout hash. Offsets are never declared by the package source.
fn fill_instances(signature: &mut Map<String, Value>, selected: &BTreeMap<String, u32>) -> Result<(), String> {
    let Some(instances) = signature.get_mut("instances").and_then(Value::as_array_mut) else {
        return Err("signature.instances must be an array".into());
    };
    for instance in instances {
        let record = instance
            .get_mut("record")
            .and_then(Value::as_object_mut)
            .ok_or("instance.record must be an object")?;
        if record.keys().any(|k| k != "fields") {
            return Err("record extent/alignment/offsets are host-selected".into());
        }
        let mut fields = Vec::new();
        for raw in record.get("fields").and_then(Value::as_array).ok_or("record.fields must be an array")? {
            let mut raw = raw.clone();
            let object = raw.as_object_mut().ok_or("record field must be an object")?;
            if object.contains_key("offset") {
                return Err("record field offsets are host-selected".into());
            }
            let key = object.get("offsetKey").and_then(Value::as_str).ok_or("record field offsetKey required")?;
            let offset = *selected.get(key).ok_or_else(|| format!("unknown offsetKey {key}"))?;
            object.insert("offset".into(), offset.into());
            fields.push(serde_json::from_value::<RecordField>(raw).map_err(|e| e.to_string())?);
        }
        let mut alignment = 1u32;
        let mut end = 0u32;
        for field in &fields {
            let width = field.width()?;
            alignment = alignment.max(width);
            end = end.max(field.offset.checked_add(width).ok_or("record extent overflow")?);
        }
        let extent = end.div_ceil(alignment).checked_mul(alignment).ok_or("record extent overflow")?;
        let layout = RecordLayout { extent, alignment, fields };
        layout.validate()?;
        let hash = layout.hash();
        let object = instance.as_object_mut().ok_or("instance must be an object")?;
        object.insert("record".into(), serde_json::to_value(&layout).map_err(|e| e.to_string())?);
        object.insert("layoutHash".into(), hash.into());
    }
    Ok(())
}

pub(super) fn materialize(
    bytes: &[u8],
    owner_id: &str,
    merged: &Value,
    repairs: &repair_snapshot::Snapshot,
    live_offset: &dyn Fn(&str, &str) -> i32,
) -> Result<Materialized, String> {
    let source: Source = serde_json::from_slice(bytes).map_err(fail)?;
    if source.schema_version != 1 || source.owner_id != owner_id {
        return Err(fail("schemaVersion/ownerId mismatch"));
    }
    let mut selected = BTreeMap::new();
    let mut offset_status = Vec::new();
    for (key, entry) in &source.offsets {
        let live = live_offset(&entry.class, &entry.field);
        let (offset, from) = if live >= 0 { (live as u32, "live-schema") } else { (entry.catalog, "schema-catalog") };
        selected.insert(key.clone(), offset);
        offset_status.push(json!({"key":key,"class":entry.class,"field":entry.field,"offset":offset,
            "source":from,"catalog":entry.catalog}));
    }
    let selected_text = serde_json::to_string(&selected).map_err(fail)?;
    let signatures = merged.get("signatures").and_then(Value::as_object);
    let mut functions = Vec::new();
    let mut function_status = Vec::new();
    for f in source.functions {
        let entry = signatures
            .and_then(|s| s.get(&f.signature_name))
            .and_then(|s| s.get(GAMEDATA_PLATFORM));
        let resolved = entry
            .ok_or_else(|| format!("gamedata signature '{}' has no {GAMEDATA_PLATFORM} entry", f.signature_name))
            .and_then(|entry| target(entry).map_err(|e| format!("gamedata signature '{}': {e}", f.signature_name)));
        let mut status = json!({"localName":f.local_name,"signature":f.signature_name,
            "customRepairs":repairs.signature_repairs(&f.signature_name)});
        let target = match resolved {
            Ok(target) => target,
            Err(reason) if f.requirement == "optional" => {
                // Degrade this descriptor by name; the package and its other functions proceed.
                status["unavailable"] = reason.into();
                function_status.push(status);
                continue;
            }
            Err(reason) => return Err(fail(format!("{}: {reason}", f.local_name))),
        };
        status["targetSha256"] = contract::hash(&serde_json::to_value(&target).map_err(fail)?).into();
        let mut signature = f.signature;
        fill_instances(&mut signature, &selected).map_err(|e| fail(format!("{}: {e}", f.local_name)))?;
        let mut wire = json!({"localName":f.local_name,"requirement":f.requirement,"target":target,
            "signature":signature,"policy":f.policy});
        if let Some(adapter) = f.adapter {
            wire["adapter"] = adapter;
        }
        functions.push(wire);
        function_status.push(status);
    }
    let wire = json!({"schemaVersion":1,"ownerId":owner_id,"selectedOffsetsSha256":contract::hash_bytes(selected_text.as_bytes()),
        "selectedOffsets":selected_text,"functions":functions});
    Ok(Materialized {
        bytes: serde_json::to_vec(&wire).map_err(fail)?,
        status: json!({"offsets":offset_status,"functions":function_status}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_functions::trusted;

    fn repairs(effects: &[(&str, &str)]) -> repair_snapshot::Snapshot {
        // GCR1 with one applied custom file carrying the given (section, name) effects.
        fn word(out: &mut Vec<u8>, n: usize) { out.extend_from_slice(&(n as u32).to_le_bytes()); }
        fn field(out: &mut Vec<u8>, v: &[u8]) { word(out, v.len()); out.extend_from_slice(v); }
        let mut out = b"GCR1".to_vec();
        if effects.is_empty() {
            word(&mut out, 0);
            word(&mut out, 0);
        } else {
            word(&mut out, 1);
            for v in ["custom/10-fix.jsonc".as_bytes(), b"applied", b"", b"{\"signatures\":{}}"] { field(&mut out, v); }
            word(&mut out, effects.len());
            for (section, name) in effects {
                for v in [*section, name, GAMEDATA_PLATFORM, "applied", "carried", ""] { field(&mut out, v.as_bytes()); }
            }
            word(&mut out, 1);
            field(&mut out, b"custom/10-fix.jsonc");
        }
        repair_snapshot::decode(&out).unwrap()
    }
    fn source() -> Value {
        let policy = |surfaces: Value, suppression: &str| {
            let mut p = json!({"id":"generic.v2","version":1,"surfaces":surfaces,"selfCall":"bypass-own-hooks","suppression":suppression});
            p["contractHash"] = contract::hash(&p).into();
            p
        };
        let hidden = |name: &str| json!({"name":name,"native":"ptr","projection":{"id":"native-only","version":1},
            "ownership":"invocation-passthrough","mutable":[],"nullable":false});
        json!({"schemaVersion":1,"ownerId":"@proof/game","offsets":{"Item::m_def":{"class":"Item","field":"m_def","catalog":56}},
          "functions":[
            {"localName":"gate","requirement":"optional","signatureName":"Gate",
             "signature":{"platform":"linux-x86_64-sysv","memberReceiver":false,"receiver":null,
               "parameters":[hidden("services"),
                 {"name":"item","native":"ptr","projection":{"id":"borrowed-record","version":1},"ownership":"synchronous-record","mutable":[],"instance":0,"nullable":false},
                 {"name":"method","native":"i32","projection":{"id":"i32","version":1},"mutable":[],"nullable":false}],
               "returns":{"name":"","native":"i32","projection":{"id":"i32","version":1},"mutable":[],"nullable":false},
               "fingerprint":"linux-x86_64-sysv:none:i32(ptr,ptr,i32)","stackCopyBytes":128,
               "instances":[{"codecId":"borrowed-record","codecVersion":1,"kind":null,"record":{"fields":[
                 {"name":"defIndex","offsetKey":"Item::m_def","storage":"u16","nullable":false,"read":["pre","post"],"write":[]}]}}],
               "scratch":[{"name":"result","storage":"i32"}]},
             "policy":policy(json!(["pre","post"]),"generic"),
             "adapter":{"id":"proof.gate.v1","contractHash":"a".repeat(64),"postOverride":true}},
            {"localName":"missing","requirement":"optional","signatureName":"Absent",
             "signature":{"platform":"linux-x86_64-sysv","memberReceiver":false,"receiver":null,"parameters":[],
               "returns":{"name":"","native":"void","projection":{"id":"void","version":1},"mutable":[],"nullable":false},
               "fingerprint":"linux-x86_64-sysv:none:void()","stackCopyBytes":128,"instances":[]},
             "policy":policy(json!(["pre"]),"none")}
          ]})
    }
    fn merged(pattern: &str) -> Value {
        json!({"signatures":{"Gate":{"linuxsteamrt64":{"module":"libserver.so","pattern":pattern,"resolve":"direct",
            "validate":{"prologue":"55 48"}}}}})
    }

    #[test]
    fn fills_target_from_merged_gamedata_and_offsets_live_first() {
        let bytes = source().to_string();
        let out = materialize(bytes.as_bytes(), "@proof/game", &merged("55 48 89 E5"), &repairs(&[("signatures", "Gate")]),
            &|class, field| if (class, field) == ("Item", "m_def") { 0x40 } else { -1 }).unwrap();
        let decoded = trusted::decode(&out.bytes, "@proof/game").unwrap();
        assert_eq!(decoded.inputs.len(), 1, "the unresolvable optional function is left out by name");
        let gate = &decoded.inputs[0];
        match &gate.target {
            NormalizedTarget::Signature { pattern, derivation, target_validate, candidate_validate, .. } => {
                assert_eq!((pattern.as_str(), derivation.as_str()), ("55 48 89 E5", "identity"));
                assert_eq!(target_validate.prologue.as_deref(), Some("55 48"));
                assert!(candidate_validate.empty());
            }
            _ => panic!("signature target"),
        }
        let record = &gate.signature.instances[0].record;
        assert_eq!((record.fields[0].offset, record.alignment, record.extent), (0x40, 2, 0x42));
        assert_eq!(gate.signature.instances[0].layout_hash, record.hash());
        assert_eq!(&*decoded.selected.bytes, r#"{"Item::m_def":64}"#);
        assert_eq!(decoded.adapters.len(), 1);
        assert!(decoded.adapters[0].post_override);
        let status = &out.status;
        assert_eq!(status["offsets"][0]["source"], "live-schema");
        assert_eq!(status["functions"][0]["customRepairs"][0]["path"], "custom/10-fix.jsonc");
        assert_eq!(status["functions"][0]["customRepairs"][0]["validator"], "carried");
        assert!(status["functions"][1]["unavailable"].as_str().unwrap().contains("'Absent' has no linuxsteamrt64 entry"));
    }

    #[test]
    fn catalog_fallback_malformed_targets_and_required_failures_are_named() {
        let bytes = source().to_string();
        let out = materialize(bytes.as_bytes(), "@proof/game", &merged("55"), &repairs(&[]), &|_, _| -1).unwrap();
        let decoded = trusted::decode(&out.bytes, "@proof/game").unwrap();
        assert_eq!(decoded.inputs[0].signature.instances[0].record.fields[0].offset, 56);
        assert_eq!(out.status["offsets"][0]["source"], "schema-catalog");
        assert_eq!(out.status["functions"][0]["customRepairs"], json!([]));
        // A repair that breaks the entry's shape degrades the optional function by name.
        let mut bad = merged("55");
        bad["signatures"]["Gate"]["linuxsteamrt64"]["resolve"] = "sideways".into();
        let out = materialize(bytes.as_bytes(), "@proof/game", &bad, &repairs(&[]), &|_, _| -1).unwrap();
        assert!(out.status["functions"][0]["unavailable"].as_str().unwrap().contains("unsupported resolver"));
        // A required function's missing signature fails the whole package by name.
        let mut required = source();
        required["functions"][1]["requirement"] = "required".into();
        let error = materialize(required.to_string().as_bytes(), "@proof/game", &merged("55"), &repairs(&[]), &|_, _| -1)
            .err().unwrap();
        assert!(error.contains("missing") && error.contains("Absent"), "{error}");
        // Owner, unknown fields and declared offsets are refused.
        assert!(materialize(bytes.as_bytes(), "@proof/other", &merged("55"), &repairs(&[]), &|_, _| -1).is_err());
        let mut extra = source();
        extra["functions"][0]["target"] = json!({});
        assert!(materialize(extra.to_string().as_bytes(), "@proof/game", &merged("55"), &repairs(&[]), &|_, _| -1).is_err());
        let mut declared = source();
        declared["functions"][0]["signature"]["instances"][0]["record"]["fields"][0]["offset"] = 8.into();
        assert!(materialize(declared.to_string().as_bytes(), "@proof/game", &merged("55"), &repairs(&[]), &|_, _| -1)
            .err().unwrap().contains("host-selected"));
    }
}

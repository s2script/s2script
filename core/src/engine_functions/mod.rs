pub(crate) mod policy;
pub(crate) mod package_adapter;
pub(crate) mod projection;
pub(crate) mod copied;
pub(crate) mod runtime;
pub(crate) mod registry;
pub(crate) mod contract;
pub(crate) mod overrides;
pub(crate) mod provenance;
mod abi {
    include!("abi.generated.rs");
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::{json, Value};
    const SDK_ARCHIVE: &str = r#"{"bundleHash":"96758183e14e65e3c9e905b4b4069cdb893680bfab4e921044957d2fed881936","functions":[{"abi":{"fingerprint":"linux-x86_64-sysv:none:void()","parameters":[],"platform":"linux-x86_64-sysv","receiver":"none","returns":{"native":"void","projection":{"id":"void","version":1}},"stackCopyBytes":128},"canonicalId":"@demo/fire::fire","contractHash":"400ea0b2c7a1a739ec26b1997e56ec8fb41cdf8ab7e58f77ef62ab31c29db4e0","localName":"fire","policy":{"contractHash":"0e62b39ca77029a170b6b56c199c827cd6c0db7c4b0908cefaaca46513c5e1b5","id":"generic.v2","selfCall":"bypass-own-hooks","suppression":"none","surfaces":["call"],"version":1},"requirement":"optional","target":{"candidateValidate":{},"derivation":"identity","kind":"signature","module":"server","pattern":"55","resolve":"direct","targetValidate":{"prologue":"55"}}}],"ownerId":"@demo/fire","schemaVersion":2}"#;
    pub(crate) fn fixture() -> Value {
        let mut b = json!({"schemaVersion":2,"ownerId":"@demo/fire","bundleHash":"", "functions":[{
            "localName":"fire","canonicalId":"@demo/fire::fire","contractHash":"",
            "target":{"kind":"signature","module":"server","pattern":"55","resolve":"direct","derivation":"identity","candidateValidate":{},"targetValidate":{"prologue":"55"}},
            "abi":{"platform":"linux-x86_64-sysv","receiver":"none","fingerprint":"linux-x86_64-sysv:none:void()","stackCopyBytes":128,"parameters":[],"returns":{"native":"void","projection":{"id":"void","version":1}}},
            "policy":{"id":"generic.v2","version":1,"contractHash":"","surfaces":["call"],"selfCall":"bypass-own-hooks","suppression":"none"},"requirement":"optional"}]});
        seal(&mut b);
        b
    }
    pub(crate) fn seal(b: &mut Value) {
        for f in b["functions"].as_array_mut().unwrap() {
            let mut p = f["policy"].clone();
            p.as_object_mut().unwrap().remove("contractHash");
            f["policy"]["contractHash"] = contract::hash(&p).into();
            f["contractHash"] =
                contract::hash(&json!({"abi":f["abi"],"policy":f["policy"]})).into();
        }
        let mut base = b.clone();
        base.as_object_mut().unwrap().remove("bundleHash");
        b["bundleHash"] = contract::hash(&base).into();
    }
    fn parse(b: &Value) -> Result<contract::NormalizedBundle, String> {
        contract::parse(
            &b.to_string(),
            "@demo/fire",
            &summary(b),
            &["engine:calls".into()],
        )
    }
    pub(crate) fn summary(b: &Value) -> Value {
        json!({"schemaVersion":2,"bundleHash":b["bundleHash"],"functions":b["functions"].as_array().unwrap().iter().map(|f|json!({"canonicalId":f["canonicalId"],"contractHash":f["contractHash"],"surfaces":f["policy"]["surfaces"],"mutates":false,"suppresses":false,"requirement":f["requirement"]})).collect::<Vec<_>>()})
    }
    #[test]
    fn archive_safety_even_with_recomputed_hashes() {
        assert!(parse(&fixture()).is_ok());
        for pointer in [
            "/extra",
            "/functions/0/abi/extra",
            "/functions/0/target/extra",
            "/functions/0/target/targetValidate/extra",
        ] {
            let mut b = fixture();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            b.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), json!(true));
            seal(&mut b);
            assert!(parse(&b).is_err(), "{pointer}");
        }
        for (pointer, value) in [
            ("/schemaVersion", json!(3)),
            ("/functions/0/policy/id", json!("legacy.acquire.v1")),
            ("/functions/0/abi/returns/native", json!("u16")),
            (
                "/functions/0/abi/returns/projection/id",
                json!("raw-pointer"),
            ),
            ("/functions/0/abi/stackCopyBytes", json!(0)),
            ("/functions/0/target/targetValidate/prologue", Value::Null),
        ] {
            let mut b = fixture();
            *b.pointer_mut(pointer).unwrap() = value;
            seal(&mut b);
            assert!(parse(&b).is_err(), "{pointer}");
        }
        let mut b = fixture();
        b["bundleHash"] = "bad".into();
        assert!(parse(&b).is_err());
        let b = fixture();
        assert!(contract::parse(&b.to_string(), "other", &summary(&b), &[]).is_err());
        assert!(contract::parse(&b.to_string(), "@demo/fire", &json!({}), &[]).is_err());
    }
    fn record(path: &str, content: Value) -> overrides::SnapshotRecord {
        let content = content.to_string();
        overrides::SnapshotRecord {
            relative_path: path.into(),
            sha256: contract::hash_bytes(content.as_bytes()),
            content,
        }
    }
    fn patch(b: &Value) -> Value {
        json!({"schemaVersion":2,"ownerId":"@demo/fire","functions":{"fire":{"contractHash":b["functions"][0]["contractHash"],"target":{"module":"server","pattern":"55","validate":{"prologue":"55"}}}}})
    }
    #[test]
    fn target_only_overrides_conflicts_and_provenance() {
        let b = fixture();
        let bundle = parse(&b).unwrap();
        let mut p = patch(&b);
        p["functions"]["fire"]["target"]["pattern"] = "56".into();
        let a = record(
            "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
            p.clone(),
        );
        let candidate = overrides::prepare(bundle.clone(), "archive", vec![a.clone()]).unwrap();
        assert_eq!(
            candidate.functions()[0].provenance().overrides[0].sha256,
            a.sha256
        );
        assert_eq!(
            candidate.functions()[0].provenance().overrides[0].relative_path,
            a.relative_path
        );
        assert_eq!(
            candidate.functions()[0].provenance().base_contract_hash,
            bundle.functions[0].contract_hash
        );
        assert_ne!(
            candidate.functions()[0].provenance().final_target_hash,
            contract::hash(&b["functions"][0]["target"])
        );
        let conflict = overrides::prepare(
            bundle.clone(),
            "archive",
            vec![
                a.clone(),
                record(
                    "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/b.jsonc",
                    p.clone(),
                ),
            ],
        )
        .unwrap();
        assert!(conflict.functions()[0].unavailable().is_some());
        match &conflict.functions()[0].provenance().resolver_result {
            provenance::ValidationResult::Unavailable(reason) => {
                assert!(reason.contains("supersedes"))
            }
            _ => panic!("conflict must record unavailability"),
        }
        p["functions"]["fire"]["supersedes"] =
            "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc".into();
        assert!(overrides::prepare(
            bundle.clone(),
            "archive",
            vec![
                a.clone(),
                record(
                    "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/b.jsonc",
                    p.clone()
                )
            ]
        )
        .unwrap()
        .functions()[0]
            .unavailable()
            .is_none());
        for (key, value) in [
            ("contractHash", json!("stale")),
            ("abi", json!({})),
            ("projection", json!({})),
            ("policy", json!({})),
            ("surfaces", json!([])),
            ("requirement", json!("required")),
            ("target", json!({})),
        ] {
            let mut p = patch(&b);
            p["functions"]["fire"][key] = value;
            assert!(
                overrides::prepare(
                    bundle.clone(),
                    "archive",
                    vec![record(
                        "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
                        p
                    )]
                )
                .unwrap()
                .functions()[0]
                    .unavailable()
                    .is_some(),
                "{key}"
            );
        }
        let mut required = b.clone();
        required["functions"][0]["requirement"] = "required".into();
        seal(&mut required);
        let mut bad = patch(&required);
        bad["functions"]["fire"]["contractHash"] = "stale".into();
        assert!(overrides::prepare(
            parse(&required).unwrap(),
            "archive",
            vec![record(
                "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
                bad
            )]
        )
        .is_err());
        assert!(overrides::prepare(
            bundle,
            "archive",
            vec![record(
                "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
                json!({"functions":{"unknown":{}}})
            )]
        )
        .is_err());
    }
    #[test]
    fn approved_sdk_complex_abi_and_projection_parity() {
        let text = r#"{"bundleHash":"da0aeda9d417db7934a25ee6920c0cf281d721c40c3314ad6b9a034ecd34aa92","functions":[{"abi":{"fingerprint":"linux-x86_64-sysv:entity:ptr(u8,i32,u32,i64,u64,f32,f64,ptr,ptr,ptr,ptr)","parameters":[{"mutable":["pre"],"name":"a0","native":"u8","projection":{"id":"bool","version":1}},{"mutable":[],"name":"a1","native":"i32","projection":{"id":"i32","version":1}},{"mutable":[],"name":"a2","native":"u32","projection":{"id":"u32","version":1}},{"mutable":[],"name":"a3","native":"i64","projection":{"id":"i64","version":1}},{"mutable":[],"name":"a4","native":"u64","projection":{"id":"u64","version":1}},{"mutable":[],"name":"a5","native":"f32","projection":{"id":"f32","version":1}},{"mutable":[],"name":"a6","native":"f64","projection":{"id":"f64","version":1}},{"mutable":[],"name":"a7","native":"ptr","projection":{"id":"entity","version":1}},{"mutable":[],"name":"a8","native":"ptr","projection":{"id":"entity?","version":1}},{"mutable":[],"name":"a9","native":"ptr","projection":{"id":"string","version":1}},{"mutable":[],"name":"a10","native":"ptr","projection":{"id":"vector","version":1}}],"platform":"linux-x86_64-sysv","receiver":"entity","returns":{"native":"ptr","projection":{"id":"entity?","version":1}},"stackCopyBytes":128},"canonicalId":"@demo/fire::mixed","contractHash":"095ece0601c4e8468fd96db948521955c507889c3d036ff88a497144c6e53fd3","localName":"mixed","policy":{"contractHash":"b4ed916980ec0d251c18f1cc26b17e927fb0343741ef56dae12162adf14aac06","id":"generic.v2","selfCall":"bypass-own-hooks","suppression":"generic","surfaces":["call","pre","post"],"version":1},"requirement":"required","target":{"candidateValidate":{},"class":"Entity","derivation":"virtual-slot","index":511,"kind":"virtual","module":"server","resolve":"direct","targetValidate":{"prologue":"55"}}},{"abi":{"fingerprint":"linux-x86_64-sysv:none:u8()","parameters":[],"platform":"linux-x86_64-sysv","receiver":"none","returns":{"native":"u8","projection":{"id":"bool","version":1}},"stackCopyBytes":128},"canonicalId":"@demo/fire::site","contractHash":"6d632e61411a2f3e7a67adf38c17c272953b6ce51e932cc213f5a71e013592de","localName":"site","policy":{"contractHash":"0e62b39ca77029a170b6b56c199c827cd6c0db7c4b0908cefaaca46513c5e1b5","id":"generic.v2","selfCall":"bypass-own-hooks","suppression":"none","surfaces":["call"],"version":1},"requirement":"optional","target":{"candidateValidate":{"string-xref":{"at":0,"dispOff":1,"expect":"é🔥","instrLen":5}},"derivation":"e8-rel32","kind":"signature","module":"server","pattern":"E8 ?? ?? ?? ??","resolve":"validated-call","targetValidate":{"prologue":"55"}}}],"ownerId":"@demo/fire","schemaVersion":2}"#;
        let mut b: Value = serde_json::from_str(text).unwrap();
        let mut summary = summary(&b);
        summary["functions"][0]["mutates"] = true.into();
        summary["functions"][0]["suppresses"] = true.into();
        assert!(contract::parse(text, "@demo/fire", &summary, &["engine:calls".into(), "engine:hooks".into()]).unwrap_err().contains("ownership"));
        b["functions"][0]["abi"]["parameters"][9]["ownership"] = "callee-borrowed".into();
        b["functions"][0]["abi"]["parameters"][10]["ownership"] = "callee-retained".into();
        seal(&mut b);
        summary["bundleHash"] = b["bundleHash"].clone();
        summary["functions"][0]["contractHash"] = b["functions"][0]["contractHash"].clone();
        let text = b.to_string();
        assert!(contract::parse(
            &text,
            "@demo/fire",
            &summary,
            &["engine:calls".into(), "engine:hooks".into()]
        )
        .is_ok());
        assert!(contract::parse(&text, "@demo/fire", &summary, &["engine:calls".into()]).is_err());
    }
    #[test]
    fn copied_ownership_is_required_and_capabilities_are_checked_after_rehash() {
        let make = |input: Option<&str>, result: Option<&str>, mutable: bool, surfaces: &[&str], suppression: &str| {
            let mut b = fixture();
            let f = &mut b["functions"][0];
            f["abi"]["parameters"] = json!([{"name":"text","native":"ptr","projection":{"id":"string","version":1},"mutable":if mutable { json!(["pre"]) } else { json!([]) }}]);
            if let Some(owner) = input { f["abi"]["parameters"][0]["ownership"] = owner.into(); }
            f["abi"]["returns"] = json!({"native":"ptr","projection":{"id":"vector","version":1}});
            if let Some(owner) = result { f["abi"]["returns"]["ownership"] = owner.into(); }
            f["abi"]["fingerprint"] = "linux-x86_64-sysv:none:ptr(ptr)".into();
            f["policy"]["surfaces"] = json!(surfaces);
            f["policy"]["suppression"] = suppression.into();
            seal(&mut b);
            b
        };
        let check = |b: &Value| {
            let mut s = summary(b);
            s["functions"][0]["mutates"] = (!b["functions"][0]["abi"]["parameters"][0]["mutable"].as_array().unwrap().is_empty()).into();
            s["functions"][0]["suppresses"] = (b["functions"][0]["policy"]["suppression"] != "none").into();
            contract::parse(&b.to_string(), "@demo/fire", &s, &["engine:calls".into(), "engine:hooks".into()])
        };
        let valid = make(Some("callee-retained"), Some("caller-borrowed"), true, &["call", "pre", "post"], "generic");
        assert!(check(&valid).is_ok());
        assert!(check(&make(None, Some("caller-borrowed"), false, &["pre", "post"], "generic")).unwrap_err().contains("ownership"));
        assert!(check(&make(Some("callee-borrowed"), None, false, &["pre", "post"], "generic")).unwrap_err().contains("ownership"));
        assert!(check(&make(Some("caller-borrowed"), Some("caller-borrowed"), false, &["pre", "post"], "generic")).is_err());
        assert!(check(&make(Some("callee-borrowed"), Some("caller-borrowed"), true, &["pre", "post"], "generic")).is_err());
        assert!(check(&make(Some("callee-borrowed"), Some("caller-borrowed"), false, &["call"], "none")).is_ok());
        assert!(check(&make(Some("native-observed"), Some("caller-borrowed"), false, &["call", "pre"], "generic")).is_err());
        assert!(check(&make(Some("native-observed"), Some("caller-borrowed"), true, &["pre", "post"], "generic")).is_err());
        assert!(check(&make(Some("callee-borrowed"), Some("native-observed"), false, &["pre", "post"], "generic")).is_err());
        assert!(check(&make(Some("callee-borrowed"), Some("native-observed"), false, &["pre", "post"], "none")).is_ok());
        assert!(check(&make(Some("callee-borrowed"), Some("native-observed"), false, &["call", "post"], "none")).is_err());
        let mut null_owner = valid.clone();
        null_owner["functions"][0]["abi"]["parameters"][0]["ownership"] = Value::Null;
        seal(&mut null_owner);
        assert!(check(&null_owner).is_err());
        let mut irrelevant = fixture();
        irrelevant["functions"][0]["abi"]["returns"]["ownership"] = "caller-borrowed".into();
        seal(&mut irrelevant);
        assert!(parse(&irrelevant).unwrap_err().contains("ownership"));
    }
    #[test]
    fn exact_approved_sdk_fixture_hash_parity() {
        // Produced by the approved SDK normalizeFunctions + engineFunctionsArchive.
        let sdk: Value = serde_json::from_str(SDK_ARCHIVE).unwrap();
        assert_eq!(sdk, fixture());
        assert!(parse(&sdk).is_ok());
    }
    #[test]
    fn preserves_validation_stages_and_utf8_bounds() {
        let mut b = fixture();
        let t = &mut b["functions"][0]["target"];
        t["resolve"] = "validated-call".into();
        t["derivation"] = "e8-rel32".into();
        t["candidateValidate"] =
            json!({"string-xref":{"at":0,"dispOff":1,"instrLen":5,"expect":"é".repeat(128)}});
        t["targetValidate"] = json!({});
        seal(&mut b);
        assert!(parse(&b).is_ok());
        for (key, value) in [
            ("expect", json!("é".repeat(129))),
            ("at", json!(2147483648u64)),
            ("dispOff", json!(2)),
            ("instrLen", json!(0)),
        ] {
            let mut invalid = b.clone();
            invalid["functions"][0]["target"]["candidateValidate"]["string-xref"][key] = value;
            seal(&mut invalid);
            assert!(parse(&invalid).is_err(), "{key}");
        }
        b["functions"][0]["target"]["targetValidate"] =
            b["functions"][0]["target"]["candidateValidate"].clone();
        b["functions"][0]["target"]["candidateValidate"] = json!({});
        seal(&mut b);
        assert!(parse(&b).is_err());
    }
    #[test]
    fn attributed_failure_only_disables_named_optional_function() {
        let mut b = fixture();
        let mut f = b["functions"][0].clone();
        f["localName"] = "other".into();
        f["canonicalId"] = "@demo/fire::other".into();
        b["functions"].as_array_mut().unwrap().push(f);
        seal(&mut b);
        let mut p = patch(&b);
        p["functions"]["fire"]["abi"] = json!({});
        let candidate = overrides::prepare(
            parse(&b).unwrap(),
            "archive",
            vec![record(
                "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
                p,
            )],
        )
        .unwrap();
        assert!(candidate.functions()[0].unavailable().is_some());
        assert!(candidate.functions()[1].unavailable().is_none());
        assert_eq!(candidate.base().bundle_hash, b["bundleHash"]);
    }
    #[test]
    fn override_jsonc_duplicate_keys_unknown_root_and_snapshot_integrity() {
        let b = fixture();
        let bundle = parse(&b).unwrap();
        let mut r = record(
            "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
            patch(&b),
        );
        r.content = format!("/* operator */{} // trailing comment", r.content);
        r.sha256 = contract::hash_bytes(r.content.as_bytes());
        assert!(overrides::prepare(bundle.clone(), "archive", vec![r.clone()]).is_ok());
        let mut malformed = r.clone();
        malformed.content = "{broken".into();
        malformed.sha256 = contract::hash_bytes(malformed.content.as_bytes());
        assert!(overrides::prepare(bundle.clone(), "archive", vec![malformed]).is_err());
        let mut duplicate = r.clone();
        duplicate.content = duplicate.content.replace(
            "\"schemaVersion\":2",
            "\"schemaVersion\":2,\"schemaVersion\":2",
        );
        duplicate.sha256 = contract::hash_bytes(duplicate.content.as_bytes());
        assert!(overrides::prepare(bundle.clone(), "archive", vec![duplicate]).is_err());
        let mut mismatch = r.clone();
        mismatch.sha256 = "bad".into();
        assert!(overrides::prepare(bundle.clone(), "archive", vec![mismatch]).is_err());
        let mut escape = r.clone();
        escape.relative_path = "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/../a.jsonc".into();
        assert!(overrides::prepare(bundle.clone(), "archive", vec![escape]).is_err());
        let mut big = r.clone();
        big.content = " ".repeat(overrides::MAX_FILE_BYTES + 1);
        big.sha256 = contract::hash_bytes(big.content.as_bytes());
        assert!(overrides::prepare(bundle.clone(), "archive", vec![big]).is_err());
        assert!(
            overrides::prepare(bundle.clone(), "archive", vec![r; overrides::MAX_FILES + 1])
                .is_err()
        );
        let a = record(
            "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
            patch(&b),
        );
        let z = record(
            "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/z.jsonc",
            patch(&b),
        );
        assert!(overrides::prepare(bundle, "archive", vec![z, a]).is_err());
    }
    #[test]
    fn operator_author_target_conversion_keeps_callsite_validation() {
        let b = fixture();
        let mut p = patch(&b);
        let entry = &mut p["functions"]["fire"];
        entry["resolve"] = "validated-call".into();
        entry["target"]["validate"] =
            json!({"string-xref":{"at":0,"dispOff":1,"instrLen":5,"expect":"site"}});
        entry["target"]["targetValidate"] = json!({"prologue":"55"});
        let result = overrides::prepare(
            parse(&b).unwrap(),
            "archive",
            vec![record(
                "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc",
                p,
            )],
        )
        .unwrap();
        assert!(result.functions()[0].unavailable().is_none());
        match &result.functions()[0].function().target {
            contract::NormalizedTarget::Signature {
                candidate_validate,
                target_validate,
                derivation,
                ..
            } => {
                assert!(candidate_validate.string_xref.is_some());
                assert_eq!(target_validate.prologue.as_deref(), Some("55"));
                assert_eq!(derivation, "e8-rel32");
            }
            _ => panic!("signature required"),
        }
    }
}

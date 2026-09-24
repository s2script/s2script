use super::prepare_selection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
struct TestDir(PathBuf);
impl TestDir {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> TestDir {
    let path = std::env::temp_dir().join(format!(
        "s2-game-package-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    TestDir(path)
}
fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn record(id: &str, owner: &str, game: &str) -> Value {
    json!({"id":id,"match":{"engine":"source2","game":game},"gamedataOwner":owner,
      "bootstrap":{"path":format!("game-packages/{owner}/index.js"),"sha256":hash(b"boot")},
      "gamedata":{"path":format!("game-packages/{owner}/gamedata.json"),"sha256":hash(b"data")}})
}
fn manifest(packages: Vec<Value>) -> Value {
    json!({"schemaVersion":2,"packages":packages})
}
static SCALAR: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../games/fixture-source2/engine-functions.json")).unwrap()
});
extern "C" fn no_function_overrides(_: *const std::ffi::c_char) -> *const std::ffi::c_char {
    c"{\"records\":[],\"error\":null}".as_ptr()
}
extern "C" fn failed_function_overrides(_: *const std::ffi::c_char) -> *const std::ffi::c_char {
    c"{\"records\":[],\"error\":\"changed override source\"}".as_ptr()
}
extern "C" fn active_function_hook(_: i64, out: *mut crate::v8host::S2FunctionHookStatus,
    _: *mut i8, _: i32) -> i32 {
    unsafe { *out = crate::v8host::S2FunctionHookStatus { state: 2, reserved: 0, receipt: 1 }; }
    1
}
extern "C" fn unavailable_function_target(_: *const i8, _: *const i8, _: *const i8,
    _: *const i8, why: *mut i8, cap: i32) -> i64 {
    let reason = b"fixture target unavailable\0";
    if cap >= reason.len() as i32 {
        unsafe { std::ptr::copy_nonoverlapping(reason.as_ptr(), why.cast(), reason.len()); }
    }
    0
}
thread_local! { static RELEASED_PACKAGE_TARGETS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
extern "C" fn counted_target_release(_: i64) -> i32 {
    RELEASED_PACKAGE_TARGETS.with(|n| n.set(n.get() + 1));
    1
}
fn transport_with_snapshot() {
    crate::v8host::function_adapter::scalar_transport_tests::init_transport();
    let mut ops = crate::v8host::engine_ops().unwrap();
    ops.plugin_function_overrides = Some(no_function_overrides);
    ops.function_hook_status = Some(active_function_hook);
    crate::v8host::set_engine_ops(Some(ops));
}
fn stop_transport() {
    crate::v8host::shutdown();
    crate::v8host::set_engine_ops(None);
}
fn with_functions(mut package: Value) -> Value {
    let artifact: Value = serde_json::from_str(SCALAR.as_str()).unwrap();
    package["functions"] = json!({
        "path":"game-packages/a/engine-functions.json", "sha256":hash(SCALAR.as_bytes()),
        "summary":{"schemaVersion":2,"bundleHash":artifact["bundleHash"],
          "functions":[{"canonicalId":"@fixture/a::scalar","contractHash":artifact["functions"][0]["contractHash"],
            "surfaces":["call"],"mutates":false,"suppresses":false,"requirement":"optional"}]},
        "permissions":["engine:calls"]});
    package
}
fn complete_fixture(packages: Vec<Value>) -> TestDir {
    let root = fixture();
    for package in &packages {
        for (kind, bytes) in [
            ("bootstrap", b"boot".as_slice()),
            ("gamedata", b"data".as_slice()),
        ] {
            let path = package[kind]["path"].as_str().unwrap();
            write(&root.path().join(path), bytes);
        }
        if let Some(path) = package["functions"]["path"].as_str() {
            write(&root.path().join(path), SCALAR.as_bytes());
        }
    }
    write(
        &root.path().join("game-packages.json"),
        manifest(packages).to_string().as_bytes(),
    );
    root
}
fn function_fixture(bootstrap: &str, empty: bool) -> TestDir {
    let mut artifact = crate::engine_functions::tests::fixture();
    artifact["ownerId"] = "@fixture/a".into();
    let function = &mut artifact["functions"][0];
    function["localName"] = "fire".into();
    function["canonicalId"] = "@fixture/a::fire".into();
    function["requirement"] = "required".into();
    function["abi"]["fingerprint"] = "linux-x86_64-sysv:none:i32(i32)".into();
    function["abi"]["parameters"] = json!([{"name":"x","native":"i32","projection":{"id":"i32","version":1},"mutable":[]}]);
    function["abi"]["returns"] = json!({"native":"i32","projection":{"id":"i32","version":1}});
    function["policy"]["surfaces"] = json!(["call", "pre", "post"]);
    function["policy"]["suppression"] = "generic".into();
    if empty { artifact["functions"] = json!([]); }
    crate::engine_functions::tests::seal(&mut artifact);
    let mut summary = crate::engine_functions::tests::summary(&artifact);
    if !empty { summary["functions"][0]["suppresses"] = true.into(); }
    let bytes = artifact.to_string();
    let mut package = record("@fixture/a", "a", "csgo");
    package["bootstrap"]["sha256"] = hash(bootstrap.as_bytes()).into();
    package["functions"] = json!({"path":"game-packages/a/engine-functions.json",
        "sha256":hash(bytes.as_bytes()),"summary":summary,
        "permissions":if empty { json!([]) } else { json!(["engine:calls","engine:hooks"]) }});
    let root = complete_fixture(vec![package]);
    write(&root.path().join("game-packages/a/index.js"), bootstrap.as_bytes());
    write(&root.path().join("game-packages/a/engine-functions.json"), bytes.as_bytes());
    root
}
fn live_function_fixture(bootstrap: &str) -> TestDir { function_fixture(bootstrap, false) }

// A controlled S2 target lives in the test executable. Package bytes are always loaded from
// S2_TEST_GAME_PACKAGE_ROOT at runtime, so this executable can be frozen before they exist.
extern "C" fn test_target_twice(value: i32) -> i32 { value * 2 }
thread_local! { static TEST_TARGET_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
extern "C" fn test_target_prepare(
    _: *const i8, target: *const i8, abi: *const i8, fingerprint: *const i8,
    why: *mut i8, cap: i32,
) -> i64 {
    let target: Value = serde_json::from_str(unsafe { std::ffi::CStr::from_ptr(target) }.to_str().unwrap()).unwrap();
    let abi: Value = serde_json::from_str(unsafe { std::ffi::CStr::from_ptr(abi) }.to_str().unwrap()).unwrap();
    let fingerprint = unsafe { std::ffi::CStr::from_ptr(fingerprint) }.to_str().unwrap();
    let checked = target["kind"] == "signature"
        && target["module"] == "s2-test-target"
        && target["pattern"] == "54 57 49 43 45"
        && target["targetValidate"]["prologue"] == "54 57 49 43 45"
        && fingerprint == "linux-x86_64-sysv:none:i32(i32)"
        && abi["receiver"] == "none"
        && abi["parameters"][0]["native"] == "i32"
        && abi["returns"]["native"] == "i32";
    if checked { test_target_twice as *const () as usize as i64 } else {
        let reason = b"S2 test target declaration failed checked resolution\0";
        if !why.is_null() && cap >= reason.len() as i32 {
            unsafe { std::ptr::copy_nonoverlapping(reason.as_ptr(), why.cast(), reason.len()); }
        }
        0
    }
}
extern "C" fn test_target_call(
    target: i64, _: u64, args: *const crate::v8host::S2FunctionValue, count: i32,
    out: *mut crate::v8host::S2FunctionValue, _: *mut i8, _: i32,
) -> i32 {
    if target != test_target_twice as *const () as usize as i64 || count != 1 || args.is_null() || out.is_null() {
        return 0;
    }
    let input = unsafe { *args };
    if input.kind != 2 { return 0; }
    // This is a real native call through the resolved test target, not a JS calculation.
    let function: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(target as usize) };
    let result = function(input.bits as i32);
    unsafe { *out = crate::v8host::S2FunctionValue { kind: 2, bits: result as u32 as u64,
        flags: 0, reserved: 0, aux: 0 }; }
    TEST_TARGET_CALLS.with(|calls| calls.set(calls.get() + 1));
    1
}

#[test]
fn synthetic_package_selects_and_uses_an_ordinary_function() {
    let Ok(root) = std::env::var("S2_TEST_GAME_PACKAGE_ROOT") else { return; };
    transport_with_snapshot();
    let mut ops = crate::v8host::engine_ops().unwrap();
    ops.function_prepare = Some(test_target_prepare);
    ops.function_call = Some(test_target_call);
    crate::v8host::set_engine_ops(Some(ops));
    TEST_TARGET_CALLS.with(|calls| calls.set(0));
    let handle = super::select(Path::new(&root), "source2", "fixture", "linuxsteamrt64").unwrap();
    super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    assert_eq!(super::selected_id().as_deref(), Some("@fixture/source2"));
    assert_eq!(crate::gamedata_calls::game_package_owner().as_deref(), Some("game-package:@fixture/source2"));
    crate::v8host::frame_tests::load_body("fixture-consumer",
        "globalThis.fixture = require('@fixture/source2'); return {};", "{}");
    crate::v8host::eval_in_context("fixture-consumer", r#"
        if (fixture.twice(21) !== 42) throw Error('checked S2 target result');
        if (__s2_require('@s2script/cs2') !== null)
            throw Error('CS2 owner admitted on fixture selection');
    "#).unwrap();
    assert_eq!(TEST_TARGET_CALLS.with(std::cell::Cell::get), 1);
    crate::v8host::unload_plugin("fixture-consumer");
    super::clear().unwrap();
    stop_transport();
}
fn select(root: &TestDir, game: &str) -> Result<super::PreparedSelection, super::PackageError> {
    prepare_selection(root.path(), "source2", game, "linuxsteamrt64")
}
fn error_for(packages: Vec<Value>) -> String {
    let root = fixture();
    write(
        &root.path().join("game-packages.json"),
        manifest(packages).to_string().as_bytes(),
    );
    select(&root, "csgo").unwrap_err().code().to_owned()
}

#[test]
fn selects_verified_owned_bytes_and_provenance() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    let selected = select(&root, "csgo").unwrap();
    assert_eq!(selected.id, "@fixture/a");
    assert_eq!(selected.gamedata_owner, "a");
    assert_eq!(selected.bootstrap_bytes, b"boot");
    assert_eq!(selected.gamedata_bytes, b"data");
    assert_eq!(selected.bootstrap_sha256, hash(b"boot"));
    assert_eq!(selected.gamedata_sha256, hash(b"data"));
    assert_eq!(selected.provenance.engine, "source2");
    assert_eq!(selected.provenance.game, "csgo");
    assert_eq!(selected.provenance.platform, "linuxsteamrt64");
    std::fs::remove_file(root.path().join("game-packages/a/index.js")).unwrap();
    assert_eq!(selected.bootstrap_bytes, b"boot");
}

#[test]
fn matches_detected_game_token_without_an_unpublished_name_grammar() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "fixture_mod")]);
    assert_eq!(select(&root, "fixture_mod").unwrap().id, "@fixture/a");
}

#[test]
fn missing_manifest_and_zero_match_are_named_missing() {
    assert_eq!(select(&fixture(), "csgo").unwrap_err().code(), "missing");
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    assert_eq!(select(&root, "other").unwrap_err().code(), "missing");
}

#[test]
fn names_every_ambiguous_candidate_in_sorted_order() {
    let root = complete_fixture(vec![
        record("@fixture/b", "b", "csgo"),
        record("@fixture/a", "a", "csgo"),
    ]);
    let err = select(&root, "csgo").unwrap_err();
    assert_eq!(err.code(), "ambiguous");
    assert_eq!(err.candidates(), &["@fixture/a", "@fixture/b"]);
}

#[test]
fn rejects_invalid_schema_and_exact_field_sets() {
    let root = fixture();
    for value in [
        json!({"schemaVersion":1,"packages":[]}),
        json!({"schemaVersion":1,"packages":[],"extra":0}),
        json!({"schemaVersion":1,"packages":[{"id":"@fixture/a"}]}),
        manifest(vec![{
            let mut p = record("@fixture/a", "a", "csgo");
            p["match"]["platform"] = json!("linux");
            p
        }]),
    ] {
        write(
            &root.path().join("game-packages.json"),
            value.to_string().as_bytes(),
        );
        assert_eq!(
            select(&root, "csgo").unwrap_err().code(),
            "invalid-manifest"
        );
    }
}

#[test]
fn selects_shared_validated_function_bytes_and_retains_them_after_mutation() {
    let root = complete_fixture(vec![with_functions(record("@fixture/a", "a", "csgo"))]);
    let selected = select(&root, "csgo").unwrap();
    let product = selected.functions.as_ref().unwrap();
    assert_eq!(product.bytes, SCALAR.as_bytes());
    assert_eq!(product.sha256, hash(SCALAR.as_bytes()));
    assert_eq!(product.bundle_hash, "498db83833b306c91054f5b48abaf776fb88851cd3736638e7cf26bdd10829e4");
    write(&root.path().join("game-packages/a/engine-functions.json"), b"changed");
    assert_eq!(product.bytes, SCALAR.as_bytes());
}

#[test]
fn rejects_function_metadata_tamper_digest_and_path_collision() {
    for change in ["summary", "permissions", "digest", "path", "extra"] {
        let mut record = with_functions(record("@fixture/a", "a", "csgo"));
        match change {
            "summary" => record["functions"]["summary"]["bundleHash"] = json!("0".repeat(64)),
            "permissions" => record["functions"]["permissions"] = json!(["engine:hooks"]),
            "digest" => record["functions"]["sha256"] = json!("0".repeat(64)),
            "path" => record["functions"]["path"] = record["bootstrap"]["path"].clone(),
            _ => record["functions"]["extra"] = json!(true),
        }
        let root = complete_fixture(vec![record]);
        assert!(select(&root, "csgo").is_err(), "{change}");
    }
}

#[test]
fn rejects_rehashed_unsupported_function_abi_and_host_only_policy() {
    for change in ["abi", "policy"] {
        let mut bundle: Value = serde_json::from_str(SCALAR.as_str()).unwrap();
        let function = &mut bundle["functions"][0];
        if change == "abi" {
            function["abi"]["fingerprint"] = json!("linux-x86_64-sysv:none:void(ptr)");
        } else {
            function["policy"]["id"] = json!("host-only.v1");
            let mut policy = function["policy"].clone();
            policy.as_object_mut().unwrap().remove("contractHash");
            function["policy"]["contractHash"] = json!(crate::engine_functions::contract::hash(&policy));
        }
        function["contractHash"] = json!(crate::engine_functions::contract::hash(&json!({
            "abi":function["abi"], "policy":function["policy"]})));
        let mut unhashed = bundle.clone();
        unhashed.as_object_mut().unwrap().remove("bundleHash");
        bundle["bundleHash"] = json!(crate::engine_functions::contract::hash(&unhashed));
        let bytes = format!("{}\n", serde_json::to_string(&bundle).unwrap());
        let mut record = with_functions(record("@fixture/a", "a", "csgo"));
        record["functions"]["sha256"] = json!(hash(bytes.as_bytes()));
        record["functions"]["summary"]["bundleHash"] = bundle["bundleHash"].clone();
        record["functions"]["summary"]["functions"][0]["contractHash"] = bundle["functions"][0]["contractHash"].clone();
        let root = complete_fixture(vec![record]);
        write(&root.path().join("game-packages/a/engine-functions.json"), bytes.as_bytes());
        assert_eq!(select(&root, "csgo").unwrap_err().code(), "invalid-manifest", "{change}");
    }
}

#[test]
fn function_artifact_rejects_traversal_symlink_and_size_over_four_mib() {
    let mut item = with_functions(record("@fixture/a", "a", "csgo"));
    item["functions"]["path"] = json!("game-packages/../escape.json");
    assert_eq!(error_for(vec![item]), "invalid-path");
    let root = complete_fixture(vec![with_functions(record("@fixture/a", "a", "csgo"))]);
    let path = root.path().join("game-packages/a/engine-functions.json");
    write(&path, &vec![b' '; 4 * 1024 * 1024 + 1]);
    assert_eq!(select(&root, "csgo").unwrap_err().code(), "invalid-manifest");
    #[cfg(unix)] {
        std::fs::remove_file(&path).unwrap();
        write(&root.path().join("outside.json"), SCALAR.as_bytes());
        std::os::unix::fs::symlink(root.path().join("outside.json"), &path).unwrap();
        assert_eq!(select(&root, "csgo").unwrap_err().code(), "invalid-path");
    }
}

#[test]
fn selected_function_product_activates_from_verified_bytes() {
    use crate::ffi::*;
    transport_with_snapshot();
    let root = complete_fixture(vec![with_functions(record("@fixture/a", "a", "csgo"))]);
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    let metadata: Value = serde_json::from_slice(&super::copy(handle, 1).unwrap()).unwrap();
    assert_eq!(metadata["functionsSha256"], hash(SCALAR.as_bytes()));
    assert_eq!(metadata["functionsBundleHash"], "498db83833b306c91054f5b48abaf776fb88851cd3736638e7cf26bdd10829e4");
    assert_eq!(metadata["functionsPath"], root.path().join("game-packages/a/engine-functions.json").canonicalize().unwrap().to_str().unwrap());
    assert_eq!(super::copy(handle, 3).unwrap(), SCALAR.as_bytes());
    write(&root.path().join("game-packages/a/engine-functions.json"), b"changed");
    let mut ops = crate::v8host::engine_ops().unwrap();
    ops.plugin_function_overrides = Some(failed_function_overrides);
    crate::v8host::set_engine_ops(Some(ops));
    assert_eq!(s2script_core_commit_game_package(handle, b"{}".as_ptr(), 2, b"GCR1\0\0\0\0\0\0\0\0".as_ptr(), 12), 1);
    let status: Value = serde_json::from_slice(&super::status()).unwrap();
    assert_eq!(status["code"], "active");
    assert_eq!(super::selected_id().as_deref(), Some("@fixture/a"));
    super::clear().unwrap();
    stop_transport();
}

#[test]
fn selected_nonempty_function_bootstraps_two_live_parents_and_retires() {
    transport_with_snapshot();
    let root = live_function_fixture(r#"
        globalThis.bootCount=(globalThis.bootCount||0)+1;
        globalThis.packageEvents=[];
        const f=__s2_package_function('fire');
        globalThis.capturedPackageFunction=f;
        f.onPre(v=>packageEvents.push('pre:'+v.x));
        f.onPost(v=>packageEvents.push('post:'+v.returnValue));
        ({'.':{fire(n){return f.call(n)}}})
    "#);
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    write(&root.path().join("game-packages/a/index.js"), b"throw Error('changed bootstrap');");
    write(&root.path().join("game-packages/a/engine-functions.json"), b"changed functions");
    super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    super::REGISTERED.with(|registered| {
        let registered = registered.borrow();
        let package = registered.as_ref().unwrap();
        assert_eq!(package._functions.as_ref().unwrap().owner(), package._receipt.owner());
    });
    for id in ["fixture-parent-a", "fixture-parent-b"] {
        crate::v8host::frame_tests::load_body(id, "return {};", "{}");
        crate::v8host::eval_in_context(id,
            "if(bootCount!==1||__s2_require('@fixture/a').fire(17)!==17)throw Error('package call');")
            .unwrap();
    }
    crate::v8host::eval_in_context("fixture-parent-a",
        "packageEvents.length=0;if(__s2_require('@fixture/a').fire(29)!==29)throw Error('cross-parent call');")
        .unwrap();
    crate::v8host::eval_in_context("fixture-parent-b",
        "if(JSON.stringify(packageEvents)!=='[\"pre:29\",\"post:29\"]')throw Error(JSON.stringify(packageEvents));")
        .unwrap();
    crate::v8host::unload_plugin("fixture-parent-a");
    crate::v8host::eval_in_context("fixture-parent-b",
        "if(__s2_require('@fixture/a').fire(19)!==19)throw Error('surviving parent');")
        .unwrap();
    crate::v8host::frame_tests::load_body("fixture-parent-a", "return {};", "{}");
    crate::v8host::eval_in_context("fixture-parent-a",
        "if(__s2_require('@fixture/a').fire(23)!==23)throw Error('reloaded parent');")
        .unwrap();
    crate::v8host::unload_plugin("fixture-parent-a");
    crate::v8host::unload_plugin("fixture-parent-b");
    super::clear().unwrap();
    let released_budget = crate::loader::retain_game_package(64 << 20).unwrap();
    drop(released_budget);
    stop_transport();
}

#[test]
fn required_target_failure_rolls_back_selected_source_and_legacy_owner() {
    transport_with_snapshot();
    let mut ops = crate::v8host::engine_ops().unwrap();
    ops.function_prepare = Some(unavailable_function_target);
    crate::v8host::set_engine_ops(Some(ops));
    let root = live_function_fixture("({'.':{}})");
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    let error = super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap_err();
    assert!(error.contains("@fixture/a::fire"), "{error}");
    assert!(super::selected_id().is_none());
    assert!(crate::gamedata_calls::game_package_owner().is_none());
    super::abort(handle).unwrap();
    stop_transport();
}

#[test]
fn optional_selected_target_failure_is_per_function_unavailable() {
    transport_with_snapshot();
    let mut ops = crate::v8host::engine_ops().unwrap();
    ops.function_prepare = Some(unavailable_function_target);
    crate::v8host::set_engine_ops(Some(ops));
    let root = complete_fixture(vec![with_functions(record("@fixture/a", "a", "csgo"))]);
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    super::REGISTERED.with(|registered| {
        let registered = registered.borrow();
        let owner = registered.as_ref().unwrap()._functions.as_ref().unwrap().owner();
        let binding = crate::engine_functions::registry::named_binding(owner, "scalar").unwrap();
        assert!(binding.target.is_none());
        assert!(binding.unavailable.as_deref().unwrap().contains("fixture target unavailable"));
        assert_eq!(binding.provenance.archive_hash, hash(SCALAR.as_bytes()));
    });
    super::clear().unwrap();
    stop_transport();
}

#[test]
fn activation_failure_retires_prepared_source_and_native_target() {
    transport_with_snapshot();
    RELEASED_PACKAGE_TARGETS.with(|n| n.set(0));
    let mut ops = crate::v8host::engine_ops().unwrap();
    ops.function_target_release = Some(counted_target_release);
    crate::v8host::set_engine_ops(Some(ops));
    let root = live_function_fixture("({'.':{}})");
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    let _injection = crate::engine_functions::registry::fail_next_package_activation("@fixture/a");
    let error = super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap_err();
    assert!(error.contains("injected package activation failure"), "{error}");
    assert_eq!(RELEASED_PACKAGE_TARGETS.with(|n| n.get()), 1);
    assert!(super::selected_id().is_none());
    assert!(crate::gamedata_calls::game_package_owner().is_none());
    super::abort(handle).unwrap();
    stop_transport();
}

#[test]
fn selected_export_validation_failure_revokes_provisional_function_facade() {
    transport_with_snapshot();
    let root = live_function_fixture(
        "globalThis.failedFacade=__s2_package_function('fire');failedFacade.onPre(()=>{});({bad:{}})"
    );
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    crate::v8host::create_plugin_context("fixture-bad-export");
    assert!(crate::v8host::is_failed("fixture-bad-export"));
    crate::v8host::eval_in_context("fixture-bad-export", r#"
        let refused=0;
        for(const op of [()=>failedFacade.call(1),()=>failedFacade.status,
            ()=>failedFacade.onPre(()=>{})]) { try { op(); } catch (_) { refused++; } }
        if(refused!==3)throw Error('provisional facade remained live');
    "#).unwrap();
    crate::v8host::unload_plugin("fixture-bad-export");
    super::clear().unwrap();
    stop_transport();
}

#[test]
fn selection_admission_uses_shared_loader_retained_budget() {
    transport_with_snapshot();
    let root = live_function_fixture("({'.':{}})");
    let pressure = crate::loader::retain_game_package(64 << 20).unwrap();
    let error = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap_err();
    assert!(error.contains("retained-byte admission"), "{error}");
    drop(pressure);
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    super::abort(handle).unwrap();
    stop_transport();
}

#[test]
fn invalid_override_snapshot_fails_selection_by_package_name() {
    transport_with_snapshot();
    let mut ops = crate::v8host::engine_ops().unwrap();
    ops.plugin_function_overrides = Some(failed_function_overrides);
    crate::v8host::set_engine_ops(Some(ops));
    let root = live_function_fixture("({'.':{}})");
    let error = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap_err();
    assert!(error.contains("@fixture/a: override snapshot: changed override source"), "{error}");
    assert!(super::selected_id().is_none());
    stop_transport();
}

#[test]
fn authored_empty_function_product_gets_one_real_package_receipt() {
    transport_with_snapshot();
    let root = function_fixture("({'.':{}})", true);
    let handle = super::select(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap();
    super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    super::REGISTERED.with(|registered| {
        let registered = registered.borrow();
        let package = registered.as_ref().unwrap();
        assert!(package._functions.is_some());
        let owner = package._functions.as_ref().unwrap().owner();
        assert!(crate::engine_functions::registry::owner_bindings(owner).is_empty());
    });
    super::clear().unwrap();
    stop_transport();
}

#[test]
fn rejects_duplicate_ids_owners_and_artifact_paths() {
    assert_eq!(
        error_for(vec![
            record("@fixture/a", "a", "csgo"),
            record("@fixture/a", "b", "other")
        ]),
        "invalid-manifest"
    );
    assert_eq!(
        error_for(vec![
            record("@fixture/a", "a", "csgo"),
            record("@fixture/b", "a", "other")
        ]),
        "invalid-manifest"
    );
    let mut b = record("@fixture/b", "b", "other");
    b["bootstrap"]["path"] = json!("game-packages/a/index.js");
    assert_eq!(
        error_for(vec![record("@fixture/a", "a", "csgo"), b]),
        "invalid-manifest"
    );
}

#[test]
fn validates_every_record_metadata_before_selection() {
    let mut other = record("@fixture/b", "b", "other");
    other["gamedata"]["sha256"] = json!("A".repeat(64));
    assert_eq!(
        error_for(vec![record("@fixture/a", "a", "csgo"), other]),
        "invalid-manifest"
    );
    for bad in ["0".repeat(63), "G".repeat(64), "A".repeat(64)] {
        let mut p = record("@fixture/a", "a", "csgo");
        p["bootstrap"]["sha256"] = json!(bad);
        assert_eq!(error_for(vec![p]), "invalid-manifest");
    }
    assert_eq!(
        error_for(vec![record("@fixture/under_score", "a", "csgo")]),
        "invalid-manifest"
    );
    assert_eq!(
        error_for(vec![record("@fixture/a", "under_score", "csgo")]),
        "invalid-manifest"
    );
}

#[test]
fn rejects_parent_absolute_and_non_normal_paths_before_artifact_io() {
    for bad in [
        "../outside",
        "/tmp/outside",
        "game-packages/../outside",
        "game-packages//a",
        "game-packages/./a",
        "game-packages\\a",
    ] {
        let mut p = record("@fixture/a", "a", "csgo");
        p["bootstrap"]["path"] = json!(bad);
        assert_eq!(error_for(vec![p]), "invalid-path", "{bad}");
    }
}

#[test]
fn rejects_directory_and_escaping_symlink_artifacts() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    let artifact = root.path().join("game-packages/a/index.js");
    std::fs::remove_file(&artifact).unwrap();
    std::fs::create_dir(&artifact).unwrap();
    assert_eq!(select(&root, "csgo").unwrap_err().code(), "invalid-path");
    #[cfg(unix)]
    {
        std::fs::remove_dir(&artifact).unwrap();
        write(&root.path().join("outside.js"), b"boot");
        std::os::unix::fs::symlink(root.path().join("outside.js"), &artifact).unwrap();
        assert_eq!(select(&root, "csgo").unwrap_err().code(), "invalid-path");
    }
}

#[cfg(unix)]
#[test]
fn rejects_duplicate_resolved_selected_artifact_paths() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    let alias = root.path().join("game-packages/a/gamedata.json");
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(root.path().join("game-packages/a/index.js"), alias).unwrap();
    assert_eq!(
        select(&root, "csgo").unwrap_err().code(),
        "invalid-manifest"
    );
}

#[test]
fn unrelated_missing_artifacts_do_not_block_selection() {
    let root = complete_fixture(vec![
        record("@fixture/a", "a", "csgo"),
        record("@fixture/b", "b", "other"),
    ]);
    std::fs::remove_dir_all(root.path().join("game-packages/b")).unwrap();
    assert_eq!(select(&root, "csgo").unwrap().id, "@fixture/a");
}

#[test]
fn rejects_non_utf8_bootstrap_and_tampered_bytes() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    write(&root.path().join("game-packages/a/index.js"), b"tampered");
    assert_eq!(select(&root, "csgo").unwrap_err().code(), "hash-mismatch");
    write(&root.path().join("game-packages/a/index.js"), &[0xff]);
    assert_eq!(
        select(&root, "csgo").unwrap_err().code(),
        "invalid-bootstrap"
    );
}

#[test]
fn retained_selection_copy_and_atomic_commit() {
    let root = complete_fixture(vec![record("@fixture/two", "independent", "other")]);
    let handle = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    write(
        &root.path().join("game-packages/independent/index.js"),
        b"changed",
    );
    write(
        &root.path().join("game-packages/independent/gamedata.json"),
        b"changed",
    );
    assert_eq!(super::copy(handle, 0).unwrap(), b"data");
    assert_eq!(super::copy(handle, 0).unwrap(), b"data");
    assert!(super::commit(handle, "[]", b"GCR1\0\0\0\0\0\0\0\0").is_err());
    assert!(super::selected_id().is_none());
    assert!(super::select(root.path(), "source2", "other", "linuxsteamrt64").is_err());
    let merged = r#"{"calls":{"call":{}},"hooks":{"hook":{}}}"#;
    super::commit(handle, merged, b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    assert_eq!(super::selected_id().as_deref(), Some("@fixture/two"));
    let status: Value = serde_json::from_slice(&super::status()).unwrap();
    assert_eq!(status["bootstrapSha256"], hash(b"boot"));
    assert_eq!(status["mergedSha256"], hash(merged.as_bytes()));
    let owner = crate::gamedata_calls::reserved_owner_id("@fixture/two");
    assert!(!crate::gamedata_calls::status(&owner, "call").contains("not declared"));
    assert!(!crate::gamedata_hooks::status(&owner, "hook").contains("not declared"));
    assert!(super::copy(handle, 0).is_err());
    assert!(super::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").is_err());
    super::clear().unwrap();
    assert_eq!(crate::gamedata_calls::game_package_owner(), None);
    assert!(crate::gamedata_hooks::status(&owner, "hook").contains("not declared"));
}

#[test]
fn repair_snapshot_admission_precedes_owner_publication_and_retains_exact_bytes() {
    let root = complete_fixture(vec![record("@fixture/two", "independent", "other")]);
    let handle = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    let owner = crate::gamedata_calls::reserved_owner_id("@fixture/two");
    assert!(super::commit(handle, "{}", b"bad").is_err());
    assert!(super::selected_id().is_none());
    assert!(crate::gamedata_calls::game_package_owner().is_none());
    assert!(crate::gamedata_hooks::status(&owner, "hook").contains("not declared"));

    let exact = b"// operator note\n {\"keys\":{\"K\":\"v\"}}\n";
    let path = b"custom/10-fix.jsonc";
    let mut snapshot = b"GCR1".to_vec();
    let mut word = |n: usize| snapshot.extend_from_slice(&(n as u32).to_le_bytes());
    word(1);
    drop(word);
    for value in [path.as_slice(), b"applied", b"", exact.as_slice()] {
        snapshot.extend_from_slice(&(value.len() as u32).to_le_bytes());
        snapshot.extend_from_slice(value);
    }
    snapshot.extend_from_slice(&0u32.to_le_bytes()); // effects
    snapshot.extend_from_slice(&1u32.to_le_bytes()); // applied paths
    snapshot.extend_from_slice(&(path.len() as u32).to_le_bytes());
    snapshot.extend_from_slice(path);
    super::commit(handle, "{}", &snapshot).unwrap();
    let status: Value = serde_json::from_slice(&super::status()).unwrap();
    assert_eq!(status["operatorRepairs"][0]["sha256"], hash(exact));
    assert_eq!(status["operatorRepairs"][0]["path"], "custom/10-fix.jsonc");
    assert_eq!(status["customPaths"][0], "custom/10-fix.jsonc");
    assert!(!status.to_string().contains("operator note"));
    super::REGISTERED.with(|slot| {
        assert_eq!(slot.borrow().as_ref().unwrap()._repair_snapshot.repairs[0].bytes, exact);
    });
    super::clear().unwrap();
}

#[test]
fn long_malformed_operator_bytes_remain_nonfatal_and_private_at_commit() {
    let root = complete_fixture(vec![record("@fixture/two", "independent", "other")]);
    let handle = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    let raw = format!("{{\"keys\":\"{}", "Q".repeat(12000)).into_bytes();
    let path = b"custom/10-malformed.jsonc";
    let diagnostic = b"gamedata parse error in custom/10-malformed.jsonc: JSON category 101 at byte 12010";
    let mut packet = b"GCR1".to_vec();
    packet.extend_from_slice(&1u32.to_le_bytes());
    for value in [path.as_slice(), b"parse-error", diagnostic.as_slice(), raw.as_slice()] {
        packet.extend_from_slice(&(value.len() as u32).to_le_bytes());
        packet.extend_from_slice(value);
    }
    packet.extend_from_slice(&0u32.to_le_bytes()); // effects
    packet.extend_from_slice(&0u32.to_le_bytes()); // no applied custom paths
    super::commit(handle, "{}", &packet).unwrap();
    let status: Value = serde_json::from_slice(&super::status()).unwrap();
    assert_eq!(status["operatorRepairs"][0]["result"], "parse-error");
    assert_eq!(status["operatorRepairs"][0]["sha256"], hash(&raw));
    assert!(status["customPaths"].as_array().unwrap().is_empty());
    assert!(!status.to_string().contains(&"Q".repeat(64)));
    super::REGISTERED.with(|slot| {
        assert_eq!(slot.borrow().as_ref().unwrap()._repair_snapshot.repairs[0].bytes, raw);
    });
    super::clear().unwrap();
}

#[test]
fn aborted_handles_do_not_alias_the_next_selection() {
    let root = complete_fixture(vec![record("@fixture/two", "two", "other")]);
    let a = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    super::abort(a).unwrap();
    let after_abort: Value = serde_json::from_slice(&super::status()).unwrap();
    assert_eq!(after_abort["code"], "aborted");
    assert_eq!(after_abort["id"], "@fixture/two");
    let b = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    assert_ne!(a, b);
    assert!(super::copy(a, 0).is_err());
    assert!(super::abort(a).is_err());
    assert_eq!(super::copy(b, 0).unwrap(), b"data");
    super::abort(b).unwrap();
}

#[test]
fn native_failure_status_survives_abort_and_stale_failure_cannot_replace_active() {
    use crate::ffi::*;
    let root = complete_fixture(vec![record("@fixture/two", "two", "other")]);
    let a = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    let reason = b"selected shipped bundle failed: invalid layout";
    assert_eq!(s2script_core_fail_game_package(a, [0xffu8].as_ptr(), 1), 0);
    assert_eq!(s2script_core_fail_game_package(a, std::ptr::null(), 0), 0);
    assert_eq!(s2script_core_fail_game_package(a, reason.as_ptr(), reason.len()), 1);
    super::abort(a).unwrap();
    let failed: Value = serde_json::from_slice(&super::status()).unwrap();
    assert_eq!(failed["code"], "failed");
    assert_eq!(failed["error"], std::str::from_utf8(reason).unwrap());
    assert_eq!(s2script_core_fail_game_package(a, reason.as_ptr(), reason.len()), 0);
    let b = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    super::commit(b, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    assert_eq!(s2script_core_abort_game_package(b), 0);
    assert_eq!(s2script_core_fail_game_package(b, reason.as_ptr(), reason.len()), 0);
    let active: Value = serde_json::from_slice(&super::status()).unwrap();
    assert_eq!(active["code"], "active");
    super::clear().unwrap();
}

#[test]
fn ffi_copy_rejects_short_buffers_stale_handles_and_invalid_utf8() {
    use crate::ffi::*;
    use std::ffi::CString;
    let root = complete_fixture(vec![record("@fixture/two", "two", "other")]);
    let root_text = CString::new(root.path().to_str().unwrap()).unwrap();
    let engine = CString::new("source2").unwrap();
    let game = CString::new("other").unwrap();
    let platform = CString::new("linuxsteamrt64").unwrap();
    let invalid = [0xffu8, 0];
    assert_eq!(
        s2script_core_select_game_package(
            root_text.as_ptr(),
            engine.as_ptr(),
            invalid.as_ptr().cast(),
            platform.as_ptr()
        ),
        0
    );
    assert!(super::selected_id().is_none());
    let handle = s2script_core_select_game_package(
        root_text.as_ptr(),
        engine.as_ptr(),
        game.as_ptr(),
        platform.as_ptr(),
    );
    assert_ne!(handle, 0);
    assert_eq!(
        s2script_core_copy_game_package(handle, 0, std::ptr::null_mut(), 0),
        4
    );
    let mut bytes = [55; 4];
    assert_eq!(
        s2script_core_copy_game_package(handle, 0, bytes.as_mut_ptr(), 3),
        -2
    );
    assert_eq!(bytes, [55; 4]);
    assert_eq!(
        s2script_core_copy_game_package(handle, 0, bytes.as_mut_ptr(), 4),
        4
    );
    assert_eq!(&bytes, b"data");
    assert_eq!(
        s2script_core_commit_game_package(handle, invalid.as_ptr(), 1, b"GCR1\0\0\0\0\0\0\0\0".as_ptr(), 12),
        0
    );
    assert!(super::selected_id().is_none());
    assert_eq!(s2script_core_abort_game_package(handle), 1);
    assert_eq!(
        s2script_core_copy_game_package(handle, 0, bytes.as_mut_ptr(), 4),
        -1
    );
    assert_eq!(s2script_core_abort_game_package(handle), 0);
}

#[test]
fn layout_only_empty_legacy_merge_registers_without_a_function_owner() {
    let root = complete_fixture(vec![record("@fixture/two", "two", "other")]);
    let handle = super::select(root.path(), "source2", "other", "linuxsteamrt64").unwrap();
    super::commit(handle, "", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    assert_eq!(super::selected_id().as_deref(), Some("@fixture/two"));
    super::clear().unwrap();
}

//! Production selected-package bootstrap (compatibility preludes are not used).
use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};
fn install(source: &str) {
    install_as("@fixture/two", source);
}
fn install_as(id: &str, source: &str) {
    let root = std::env::temp_dir().join(format!("s2-bootstrap-{}", std::process::id()));
    std::fs::create_dir_all(root.join("game-packages/two")).unwrap();
    let data = b"{}";
    std::fs::write(root.join("game-packages/two/index.js"), source).unwrap();
    std::fs::write(root.join("game-packages/two/data.json"), data).unwrap();
    let manifest = json!({"schemaVersion":2,"packages":[{"id":id, "match":{"engine":"source2","game":"fixture"}, "gamedataOwner":"independent", "bootstrap":{"path":"game-packages/two/index.js","sha256":format!("{:x}",Sha256::digest(source.as_bytes()))},"gamedata":{"path":"game-packages/two/data.json","sha256":format!("{:x}",Sha256::digest(data))}}]});
    std::fs::write(root.join("game-packages.json"), manifest.to_string()).unwrap();
    let handle =
        crate::game_packages::select(&root, "source2", "fixture", "linuxsteamrt64").unwrap();
    std::fs::write(
        root.join("game-packages/two/index.js"),
        "throw Error('mutated')",
    )
    .unwrap();
    crate::game_packages::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
extern "C" fn logger(_: c_int, _: *const c_char) {}
#[test]
fn selected_package_exact_exports_context_reload_and_terminal_retirement() {
    init(logger).unwrap();
    install(
        r#"globalThis.runs=(globalThis.runs||0)+1;
        ({'.':{value:17, runs:globalThis.runs, entity:__s2require('@s2script/sdk/entity').EntityRef}, './ui':{value:23}})"#,
    );
    for id in ["selected-a", "selected-b"] {
        load_plugin_js(id, "exports.OnPluginStart=()=>{};", "{}");
        assert!(!is_failed(id));
        eval_in_context(id, r#"if(__s2require('@fixture/two').value!==17 || __s2require('@fixture/two').runs!==1 || __s2require('@fixture/two/ui').value!==23 || __s2require('@fixture/two/missing')!==null || __s2require('@fixture/twoish')!==null) throw Error('exports');"#).unwrap();
    }
    assert!(crate::game_packages::clear().is_err());
    unload_plugin("selected-a");
    eval_in_context(
        "selected-b",
        "if(__s2require('@fixture/two').value!==17)throw Error('peer');",
    )
    .unwrap();
    load_plugin_js("selected-a", "exports.OnPluginStart=()=>{};", "{}");
    eval_in_context(
        "selected-a",
        "if(__s2require('@fixture/two').runs!==1)throw Error('duplicate eval');",
    )
    .unwrap();
    shutdown();
    assert!(crate::game_packages::selected_id().is_none());
}
#[test]
fn selected_package_rejects_throw_accessors_proxies_async_and_invalid_subpaths() {
    for source in [
        "globalThis.partial=1; throw Error('bootstrap failed')",
        "({get '.'(){throw Error('GETTER EXECUTED')}})",
        "new Proxy({'.':{}},{ownKeys(){throw Error('TRAP EXECUTED')}})",
        "Promise.resolve({'.':{}})",
        "({'.':{get then(){throw Error('GETTER EXECUTED')}}})",
        "({'.':new Proxy({},{get(){throw Error('TRAP EXECUTED')}})})",
        "({'./../bad':{},'.':{}})",
        "({'./ui':{}})",
    ] {
        init(logger).unwrap();
        let source = format!(
            "__s2_function_adapter_register('fixture.cleanup.v1','{}',{{pre(){{return 0}}}});\n{}",
            "1".repeat(64),
            source
        );
        install(&source);
        load_plugin_js("selected-bad", "throw Error('plugin must not run');", "{}");
        assert!(is_failed("selected-bad"), "{source}");
        assert!(!PLUGINS.with(|p| p.borrow().contains_key("selected-bad")));
        assert_eq!(
            function_adapter::proof::owner_resources("selected-bad"),
            (0, 0)
        );
        let failure = FAILED_PLUGINS.with(|p| p.borrow().get("selected-bad").cloned().unwrap());
        assert!(
            !failure.contains("EXECUTED"),
            "host validation invoked user code: {failure}"
        );
        assert!(
            !failure.contains("plugin must not run"),
            "bootstrap failure was swallowed"
        );
        shutdown();
    }
}

#[test]
fn selected_real_package_bootstraps_root_and_runtime_ui_exports() {
    init(logger).unwrap();
    let root = std::env::temp_dir().join(format!("s2-real-package-{}", std::process::id()));
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let result = std::process::Command::new("node")
        .current_dir(repo)
        .args([
            "--experimental-strip-types",
            "--no-warnings",
            "scripts/build-game-packages.mjs",
            "--out",
        ])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let handle = crate::game_packages::select(&root, "source2", "csgo", "linuxsteamrt64").unwrap();
    crate::game_packages::commit(handle, "{}", b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    load_plugin_js("real-package", "exports.OnPluginStart=()=>{};", "{}");
    assert!(
        !is_failed("real-package"),
        "{:?}",
        FAILED_PLUGINS.with(|p| p.borrow().clone())
    );
    eval_in_context(
        "real-package",
        r#"
        const root = __s2require('@s2script/cs2');
        const ui = __s2require('@s2script/cs2/ui');
        if(typeof root.Player !== 'function' || typeof root.Pawn !== 'function')throw Error('root');
        for(const key of ['PROBE_LAYOUT','DEFAULT_HUD_DESCRIPTOR','CustomHudLayout','ui','hudkit'])
            if(ui[key] === undefined || ui[key] !== root[key])throw Error('ui export '+key);
        if(__s2require('@s2script/cs2/econ')!==null)throw Error('econ is type-only');
    "#,
    )
    .unwrap();
    shutdown();
    std::fs::remove_dir_all(root).unwrap();
}

fn build_real_package(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("s2-real-package-{tag}-{}", std::process::id()));
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let result = std::process::Command::new("node")
        .current_dir(repo)
        .args(["--experimental-strip-types", "--no-warnings", "scripts/build-game-packages.mjs", "--out"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    root
}
extern "C" fn trusted_instance_prepare(binding: u64, _: *const S2FunctionInstanceOwner, _: *const i8, _: *const i8,
    _: *const i8, out: *mut S2FunctionInstancePrepared, _: *mut i8, _: i32) -> i32 {
    // Distinct native targets per binding, as two different engine functions resolve to.
    unsafe { *out = S2FunctionInstancePrepared { version: 1, struct_size: 24, target: binding as i64, capability: binding } };
    1
}
extern "C" fn trusted_instance_activate(_: u64, _: *const S2FunctionInstanceOwner, _: *mut i8, _: i32) -> i32 { 1 }
extern "C" fn trusted_instance_release(_: u64) -> i32 { 1 }
extern "C" fn active_hook(_: i64, out: *mut S2FunctionHookStatus, _: *mut i8, _: i32) -> i32 {
    unsafe { *out = S2FunctionHookStatus { state: 2, reserved: 0, receipt: 1 }; }
    1
}
extern "C" fn no_read(_: *const S2FunctionInstanceAccess, _: i32, _: *mut S2FunctionValue, _: *mut i8, _: i32) -> i32 { 0 }
extern "C" fn no_field_read(_: *const S2FunctionInstanceAccess, _: i32, _: u32, _: *mut S2FunctionValue, _: *mut i8, _: i32) -> i32 { 0 }
extern "C" fn no_field_write(_: *const S2FunctionInstanceAccess, _: i32, _: u32, _: *const S2FunctionValue, _: *mut i8, _: i32) -> i32 { 0 }
extern "C" fn no_call_copy(_: i64, _: u64, _: *const S2FunctionValue, _: i32, _: *mut S2FunctionValue, _: *const S2FunctionCopyInput,
    _: *mut S2FunctionCopyOutput, _: *const S2FunctionCopyProducer, _: *mut i8, _: i32) -> i32 { 0 }
extern "C" fn no_read_copy(_: i64, _: u64, _: u64, _: *const i8, _: i32, _: *mut S2FunctionValue, _: *mut S2FunctionCopyOutput,
    _: *mut i8, _: i32) -> i32 { 0 }
extern "C" fn no_write_copy(_: i64, _: u64, _: u64, _: *const i8, _: i32, _: *const S2FunctionValue, _: *const S2FunctionCopyInput,
    _: *const S2FunctionCopyProducer, _: *mut i8, _: i32) -> i32 { 0 }
extern "C" fn no_override_copy(_: i64, _: u64, _: u64, _: *const i8, _: *const S2FunctionValue, _: *const S2FunctionCopyInput,
    _: *const S2FunctionCopyProducer, _: *mut S2FunctionValue, _: *mut S2FunctionCopyOutput, _: *mut i8, _: i32) -> i32 { 0 }

/// The shipped CS2 package: its sealed trusted source is materialized against the MERGED gamedata
/// signatures at commit, both adapter bindings are prepared, activated and authorized, and a
/// plugin context can subscribe through the package adapters (the natives themselves are gone).
#[test]
fn selected_real_package_activates_trusted_functions_from_merged_gamedata() {
    function_adapter::scalar_transport_tests::init_transport();
    let mut ops = engine_ops().unwrap();
    ops.function_prepare_instance = Some(trusted_instance_prepare);
    ops.function_instance_activate = Some(trusted_instance_activate);
    ops.function_instance_release = Some(trusted_instance_release);
    ops.function_hook_status = Some(active_hook);
    ops.function_frame_read_instance = Some(no_read);
    ops.function_frame_field_read = Some(no_field_read);
    ops.function_frame_field_write = Some(no_field_write);
    ops.function_call_copy = Some(no_call_copy);
    ops.function_frame_read_copy = Some(no_read_copy);
    ops.function_frame_write_copy = Some(no_write_copy);
    ops.function_frame_commit_copy = Some(no_write_copy);
    ops.function_frame_override_return_copy = Some(no_override_copy);
    set_engine_ops(Some(ops));
    let root = build_real_package("trusted");
    let bundle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("game-packages/cs2/gamedata.json")).unwrap()).unwrap();
    let mut signatures = serde_json::Map::new();
    for file in bundle["files"].as_array().unwrap() {
        if let Some(entries) = file["document"]["signatures"].as_object() {
            signatures.extend(entries.clone());
        }
    }
    let merged = json!({ "signatures": signatures }).to_string();
    let handle = crate::game_packages::select(&root, "source2", "csgo", "linuxsteamrt64").unwrap();
    crate::game_packages::commit(handle, &merged, b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    let status: serde_json::Value = serde_json::from_slice(&crate::game_packages::status()).unwrap();
    let trusted = &status["trustedFunctions"];
    assert_eq!(trusted["functions"].as_array().unwrap().len(), 2, "{status}");
    for f in trusted["functions"].as_array().unwrap() {
        assert!(f.get("unavailable").is_none() && f["targetSha256"].is_string(), "{f}");
        assert_eq!(f["binding"], "available", "{f}");
        assert_eq!(f["customRepairs"], json!([]));
    }
    // No live schema in this host: the build's schema-catalog value is selected and named.
    assert_eq!(trusted["offsets"][0]["source"], "schema-catalog");
    assert_eq!(trusted["offsets"][0]["offset"], 0x38);
    load_plugin_js("trusted-real", "exports.OnPluginStart=()=>{};", "{}");
    assert!(!is_failed("trusted-real"), "{:?}", FAILED_PLUGINS.with(|p| p.borrow().clone()));
    eval_in_context(
        "trusted-real",
        r#"
        if(typeof __s2_function_adapter_register!=='undefined'||typeof __s2_function_adapter_subscribe!=='undefined')
            throw Error('bootstrap natives leaked');
        const a=globalThis.__s2pkg_cs2_adapters;
        let warned='';console.log=m=>{warned+=m;};
        if(a.acquire.status()!=='available'||a.hudClick.status()!=='available')throw Error('adapters '+a.acquire.status());
        const pawns={maxPlayers:0,forSlot:()=>null};
        for(const r of [a.acquire.subscribe('pre',()=>0,pawns),a.acquire.subscribe('post',()=>{},pawns),a.hudClick.subscribe(()=>{})])
            if(!r||r.status!=='active')throw Error('subscription '+(r&&r.status)+' '+warned);
    "#,
    )
    .unwrap();
    unload_plugin("trusted-real");
    shutdown();
    set_engine_ops(None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn selected_exports_survive_callback_until_queued_unload_and_stale_native_closures_reject() {
    use std::io::Write;
    init(logger).unwrap();
    install(&format!(
        r#"(()=>{{
        const subscribe=__s2_function_adapter_subscribe;
        __s2_function_adapter_register('fixture.retirement.v1','{}',{{pre(){{return 0}}}});
        return {{'.':{{
            engineAccess:()=>subscribe(1n,'fixture.retirement.v1','pre',()=>{{}}),
            lookup:()=>__s2require('@fixture/two'),
            retire:()=>{{
                if(!__s2_plugin_unload('selected-retire'))throw Error('unload not queued');
                if(__s2require('@fixture/two')===null)throw Error('exports freed inside callback');
                return 71;
            }}
        }}}};
    }})()"#,
        "2".repeat(64)
    ));
    let root = std::env::temp_dir().join(format!("s2-retirement-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = std::fs::File::create(root.join("retire.s2sp")).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default();
    zip.start_file("manifest.json", options).unwrap();
    zip.write_all(br#"{"id":"selected-retire","version":"1.0.0","apiVersion":"2.x"}"#)
        .unwrap();
    zip.start_file("plugin.js", options).unwrap();
    zip.write_all(b"exports.OnPluginStart=()=>{};").unwrap();
    zip.finish().unwrap();
    crate::loader::set_plugins_dir(root.to_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while plugin_phase("selected-retire") != Some(plugin::Phase::Active)
        && Instant::now() < deadline
    {
        crate::loader::poll_plugins();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(plugin_phase("selected-retire"), Some(plugin::Phase::Active));
    load_plugin_js("selected-peer", "exports.OnPluginStart=()=>{};", "{}");
    // Retain an old-context JS closure in the peer only for this host lifetime test.
    let stale = HOST.with(|h| {
        let mut h = h.borrow_mut();
        let mut storage = v8::HandleScope::new(&mut h.as_mut().unwrap().isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let ctx = PLUGINS.with(|p| p.borrow()["selected-retire"].context.clone());
        let ctx = v8::Local::new(&mut hs, ctx);
        let scope = &mut v8::ContextScope::new(&mut hs, ctx);
        let root =
            PLUGINS.with(|p| p.borrow()["selected-retire"].package_exports[0].modules["."].clone());
        let root = v8::Local::new(scope, root);
        v8::Global::new(scope, root)
    });
    eval_in_context(
        "selected-retire",
        "if(__s2require('@fixture/two').retire()!==71)throw Error('callback');",
    )
    .unwrap();
    assert!(PLUGINS.with(|p| p.borrow().contains_key("selected-retire")));
    assert_eq!(
        function_adapter::proof::owner_resources("selected-retire"),
        (1, 0)
    );
    assert!(crate::game_packages::clear().is_err());
    crate::loader::poll_plugins();
    assert!(plugin_phase("selected-retire").is_none());
    assert_eq!(
        function_adapter::proof::owner_resources("selected-retire"),
        (0, 0)
    );
    assert_eq!(
        function_adapter::proof::owner_resources("selected-peer"),
        (1, 0)
    );
    load_plugin_js("selected-retire", "exports.OnPluginStart=()=>{};", "{}");
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let mut storage = v8::HandleScope::new(&mut h.as_mut().unwrap().isolate);
        let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
        let ctx = PLUGINS.with(|p| p.borrow()["selected-peer"].context.clone());
        let ctx = v8::Local::new(&mut hs, ctx);
        let scope = &mut v8::ContextScope::new(&mut hs, ctx);
        let global = ctx.global(scope);
        let key = v8::String::new(scope, "stalePackage").unwrap();
        let value = v8::Local::new(scope, &stale);
        global.set(scope, key.into(), value.into());
    });
    let stale_result = eval_in_context(
        "selected-peer",
        r#"let rejected=false;try{stalePackage.engineAccess()}catch(e){rejected=String(e).includes('owner generation unavailable')}if(!rejected)throw Error('stale closure');if(stalePackage.lookup()!==null)throw Error('stale lookup reached replacement');"#,
    );
    drop(stale);
    shutdown();
    std::fs::remove_dir_all(root).unwrap();
    stale_result.unwrap();
}

#[test]
fn selected_identity_cannot_publish_a_partial_global_or_mutate_the_host_export_map() {
    init(logger).unwrap();
    install_as(
        "@s2script/fixture",
        r#"
        globalThis.__s2pkg_fixture={value:99};
        if(__s2require('@s2script/fixture')!==null)throw Error('partial package published');
        globalThis.resultMap={'.':{value:17},'./ui':{value:23}};
        resultMap;
    "#,
    );
    load_plugin_js("selected-map", "exports.OnPluginStart=()=>{};", "{}");
    assert!(!is_failed("selected-map"));
    eval_in_context("selected-map", r#"
        resultMap['.']={value:88};resultMap['./extra']={};delete resultMap['./ui'];
        if(__s2require('@s2script/fixture').value!==17 || __s2require('@s2script/fixture/ui').value!==23 || __s2require('@s2script/fixture/extra')!==null)throw Error('export map not private');
    "#).unwrap();
    shutdown();
}

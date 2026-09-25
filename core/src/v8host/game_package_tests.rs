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
fn merged_signatures(root: &std::path::Path) -> String {
    let bundle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("game-packages/cs2/gamedata.json")).unwrap()).unwrap();
    let mut signatures = serde_json::Map::new();
    for file in bundle["files"].as_array().unwrap() {
        if let Some(entries) = file["document"]["signatures"].as_object() {
            signatures.extend(entries.clone());
        }
    }
    json!({ "signatures": signatures }).to_string()
}
thread_local! {
    static TRUSTED_TARGETS: std::cell::RefCell<std::collections::BTreeMap<String, i64>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}
extern "C" fn trusted_instance_prepare(binding: u64, _: *const S2FunctionInstanceOwner, name: *const i8, _: *const i8,
    _: *const i8, out: *mut S2FunctionInstancePrepared, _: *mut i8, _: i32) -> i32 {
    // Distinct native targets per binding, as two different engine functions resolve to.
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
    TRUSTED_TARGETS.with(|t| t.borrow_mut().insert(name, binding as i64));
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

fn install_trusted_ops() {
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
}
/// The shipped CS2 package: its sealed trusted source is materialized against the MERGED gamedata
/// signatures at commit, both adapter bindings are prepared, activated and authorized, and a
/// plugin context can subscribe through the package adapters (the natives themselves are gone).
#[test]
fn selected_real_package_activates_trusted_functions_from_merged_gamedata() {
    install_trusted_ops();
    let root = build_real_package("trusted");
    let merged = merged_signatures(&root);
    let handle = crate::game_packages::select(&root, "source2", "csgo", "linuxsteamrt64").unwrap();
    crate::game_packages::commit(handle, &merged, b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    let status: serde_json::Value = serde_json::from_slice(&crate::game_packages::status()).unwrap();
    let trusted = &status["trustedFunctions"];
    assert_eq!(trusted["functions"].as_array().unwrap().len(), 3, "{status}");
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

// A pointer-free native frame for the trusted canAcquire binding: (hidden services, item record,
// i32 method, hidden) -> i32. It models the shim's order: PRE commit decides whether the
// original runs; POST reads the current return (-2), the engine original (-3) and may override.
#[derive(Clone, Copy, Default)]
struct AcquireFrame {
    token: u64,
    method: i32,
    def_index: u32,
    item: bool,
    original: i32,
    current: i32,
    action: i32,
    committed: Option<i32>,
    overrides: u32,
}
thread_local! { static ACQUIRE_FRAME: std::cell::Cell<AcquireFrame> = std::cell::Cell::new(AcquireFrame::default()); }
fn i32_value(v: i32) -> S2FunctionValue {
    let mut out = crate::engine_functions::runtime::blank();
    out.kind = 2;
    out.bits = v as u32 as u64;
    out
}
extern "C" fn acquire_read(_: i64, token: u64, _: u64, _: *const i8, selector: i32, kind: u8, out: *mut S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    let f = ACQUIRE_FRAME.with(|c| c.get());
    let value = match selector { 2 => f.method, -2 => f.current, -3 => f.original, _ => return 0 };
    if token != f.token || kind != 2 { return 0; }
    unsafe { *out = i32_value(value) };
    1
}
extern "C" fn acquire_commit(_: i64, token: u64, _: u64, _: *const i8, action: i32, value: *const S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    ACQUIRE_FRAME.with(|c| {
        let mut f = c.get();
        if token != f.token { return 0; }
        f.action = action;
        f.committed = (!value.is_null()).then(|| unsafe { (*value).bits as i32 });
        c.set(f);
        1
    })
}
extern "C" fn acquire_override(_: i64, token: u64, _: u64, _: *const i8, value: *const S2FunctionValue,
    out: *mut S2FunctionValue, _: *mut i8, _: i32) -> i32 {
    ACQUIRE_FRAME.with(|c| {
        let mut f = c.get();
        if token != f.token { return 0; }
        f.current = unsafe { (*value).bits as i32 };
        f.overrides += 1;
        c.set(f);
        unsafe { *out = *value };
        1
    })
}
extern "C" fn acquire_presence(access: *const S2FunctionInstanceAccess, selector: i32, out: *mut S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    let f = ACQUIRE_FRAME.with(|c| c.get());
    if unsafe { (*access).frame_token } != f.token || selector != 1 { return 0; }
    let mut v = crate::engine_functions::runtime::blank();
    v.kind = 1;
    v.bits = f.item as u64;
    unsafe { *out = v };
    1
}
extern "C" fn acquire_field(access: *const S2FunctionInstanceAccess, selector: i32, field: u32, out: *mut S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    let f = ACQUIRE_FRAME.with(|c| c.get());
    if unsafe { (*access).frame_token } != f.token || selector != 1 || field != 0 { return 0; }
    let mut v = crate::engine_functions::runtime::blank();
    v.kind = 3;
    v.bits = f.def_index as u64;
    unsafe { *out = v };
    1
}
extern "C" fn acquire_unrelated(_: *const S2FunctionInstanceAccess, _: i32, _: *const S2FunctionValue, _: u32,
    out: *mut i32, _: *mut i8, _: i32) -> i32 {
    unsafe { *out = 0 };
    1
}
/// PRE, then (as the shim would) the original unless PRE suppressed, then POST. Returns the
/// final native return and whether the original was skipped.
fn acquire_dispatch(target: i64, engine: i32, item: bool) -> (i32, bool, u32) {
    let id = crate::engine_functions::registry::next_id().unwrap();
    ACQUIRE_FRAME.with(|c| c.set(AcquireFrame { token: id, method: 1, def_index: 42, item, ..Default::default() }));
    let mut info = S2FunctionFrameInfo { version: 1, struct_size: 48, frame_token: id, native_epoch: id,
        invocation_id: id, suppressed_owner: 0, parameter_count: 4, flags: 0 };
    assert_eq!(crate::ffi::s2script_core_dispatch_function(target, &info, 0), 1, "PRE");
    let skipped = ACQUIRE_FRAME.with(|c| {
        let mut f = c.get();
        let skipped = f.action >= 2;
        if skipped { f.current = f.committed.expect("a suppressing PRE commits its return"); }
        else { f.original = engine; f.current = engine; }
        c.set(f);
        skipped
    });
    info.flags = skipped as _;
    assert_eq!(crate::ffi::s2script_core_dispatch_function(target, &info, 1), 1, "POST");
    let f = ACQUIRE_FRAME.with(|c| c.get());
    (f.current, skipped, f.overrides)
}

/// The shipped package end to end on the real V8 facade: two plugins' `items` gates fold through
/// the packaged legacy.acquire.v1 adapter over the trusted canAcquire binding.
#[test]
fn shipped_items_gates_fold_through_the_trusted_acquire_adapter() {
    install_trusted_ops();
    let mut ops = engine_ops().unwrap();
    ops.function_frame_read = Some(acquire_read);
    ops.function_frame_commit = Some(acquire_commit);
    ops.function_frame_override_return = Some(acquire_override);
    ops.function_frame_read_instance = Some(acquire_presence);
    ops.function_frame_field_read = Some(acquire_field);
    ops.function_frame_hidden_referenced_by = Some(acquire_unrelated);
    set_engine_ops(Some(ops));
    let root = build_real_package("acquire");
    let handle = crate::game_packages::select(&root, "source2", "csgo", "linuxsteamrt64").unwrap();
    crate::game_packages::commit(handle, &merged_signatures(&root), b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    let target = TRUSTED_TARGETS.with(|t| t.borrow()["@s2script/cs2::canAcquire"]);
    let plugin = r#"const {items}=require("@s2script/cs2");
        globalThis.log=[];globalThis.mode=null;
        exports.OnPluginStart=()=>{
          items.onCanAcquire(v=>{
            log.push(`pre:${v.method}:${v.defIndex}:${v.result}:${v.player}:${v.skipped}`);
            if(!mode)return 0;
            if(mode.write!==undefined)v.result=mode.write;
            return mode.action;
          });
          items.onCanAcquirePost(v=>{log.push(`post:${v.result}:${v.skipped}`);v.result=99;});
        };"#;
    for id in ["acquire-a", "acquire-b"] {
        load_plugin_js(id, plugin, "{}");
        assert!(!is_failed(id), "{:?}", FAILED_PLUGINS.with(|p| p.borrow().clone()));
    }
    // Each step sets both plugins' mode and clears both logs.
    let set = |a: &str, b: &str| {
        eval_in_context("acquire-a", &format!("log.length=0;mode={a};")).unwrap();
        eval_in_context("acquire-b", &format!("log.length=0;mode={b};")).unwrap();
    };
    let log = |id: &str| frame_tests::eval_in_context_string(id, "log.join('|')");
    // No vote: the engine result stands, both POST observers see it unskipped.
    set("null", "null");
    assert_eq!(acquire_dispatch(target, 3, true), (3, false, 0));
    assert_eq!(log("acquire-a"), "pre:1:42:0:null:false|post:3:false");
    // Changed deny with engine Allow: carried to the forced POST and overridden once.
    set("{write:6,action:1}", "null");
    assert_eq!(acquire_dispatch(target, 0, true), (6, false, 1));
    assert_eq!(log("acquire-b"), "pre:1:42:6:null:false|post:6:false", "B sees A's shared write and the effective return");
    // Changed Allow with engine deny: no override.
    set("{write:0,action:1}", "null");
    assert_eq!(acquire_dispatch(target, 6, true), (6, false, 0));
    // A later Handled deny outranks an earlier Changed deny; the original is skipped.
    set("{write:6,action:1}", "{write:2,action:2}");
    assert_eq!(acquire_dispatch(target, 0, true), (2, true, 0));
    assert_eq!(log("acquire-a"), "pre:1:42:0:null:false|post:2:true");
    assert_eq!(log("acquire-b"), "pre:1:42:6:null:false|post:2:true");
    // Handled without a write: the implicit deny 1; a null item reads defIndex 0.
    set("{action:2}", "null");
    assert_eq!(acquire_dispatch(target, 0, false), (1, true, 0));
    assert_eq!(log("acquire-a"), "pre:1:0:0:null:false|post:1:true");
    // Stop ends delivery: B never runs.
    set("{write:4,action:3}", "{write:9,action:2}");
    assert_eq!(acquire_dispatch(target, 0, true), (4, true, 0));
    assert_eq!(log("acquire-b"), "post:4:true");
    // Outside the load window the module API refuses registration.
    assert!(frame_tests::eval_in_context_string("acquire-a",
        r#"(()=>{try{__s2require("@s2script/cs2").items.onCanAcquire(()=>0);return 'ok'}catch(e){return String(e.message)}})()"#)
        .contains("outside the load window"));
    for id in ["acquire-a", "acquire-b"] { unload_plugin(id); }
    shutdown();
    set_engine_ops(None);
    std::fs::remove_dir_all(root).unwrap();
}

thread_local! {
    static HUD_FRAME: std::cell::Cell<(u64, i32, bool)> = const { std::cell::Cell::new((0, -1, false)) };
    static HUD_CLICKER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
extern "C" fn hud_read(_: i64, token: u64, _: u64, _: *const i8, selector: i32, _: u8, out: *mut S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    if token != HUD_FRAME.with(|f| f.get().0) || selector != 1 { return 0; }
    let reference = crate::engine_functions::projection::EntityReference { index: 611, id: HUD_CLICKER.with(|c| c.get()) };
    let value = crate::engine_functions::projection::encode(crate::engine_functions::projection::ProjectedValue::Entity {
        reference: Some(reference), nullable: true }).unwrap();
    unsafe { *out = value };
    1
}
/// The native side copied the CUtlString before dispatch; the host serves that snapshot.
extern "C" fn hud_read_copy(_: i64, token: u64, _: u64, _: *const i8, selector: i32, value: *mut S2FunctionValue,
    output: *mut S2FunctionCopyOutput, _: *mut i8, _: i32) -> i32 {
    if token != HUD_FRAME.with(|f| f.get().0) || selector != 3 { return 0; }
    let text = b"Dismiss";
    unsafe {
        if (*value).flags != 4 || (*output).capacity < text.len() as u64 { return 0; }
        std::ptr::copy_nonoverlapping(text.as_ptr(), (*output).data, text.len());
        (*output).size = text.len() as u64;
        (*value).aux = text.len() as u32;
    }
    1
}
extern "C" fn hud_commit_copy(_: i64, token: u64, _: u64, _: *const i8, action: i32, _: *const S2FunctionValue,
    _: *const S2FunctionCopyInput, _: *const S2FunctionCopyProducer, _: *mut i8, _: i32) -> i32 {
    HUD_FRAME.with(|f| {
        let (id, _, committed) = f.get();
        if token != id || committed { return 0; }
        f.set((id, action, true));
        1
    })
}

/// The shipped package end to end: a plugin's `ui.onClicked` receives the host-copied button id
/// and the clicker during the native PRE (before the original would run), and the adapter
/// commits Continue so the engine's own click handling proceeds.
#[test]
fn shipped_ui_clicks_are_delivered_by_the_trusted_hud_adapter_before_the_original() {
    install_trusted_ops();
    let mut ops = engine_ops().unwrap();
    ops.function_frame_read = Some(hud_read);
    ops.function_frame_read_copy = Some(hud_read_copy);
    ops.function_frame_commit_copy = Some(hud_commit_copy);
    set_engine_ops(Some(ops));
    let root = build_real_package("hud");
    let handle = crate::game_packages::select(&root, "source2", "csgo", "linuxsteamrt64").unwrap();
    crate::game_packages::commit(handle, &merged_signatures(&root), b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    let target = TRUSTED_TARGETS.with(|t| t.borrow()["@s2script/cs2::customHudClicked"]);
    load_plugin_js("hud-clicks", r#"const {ui}=require("@s2script/cs2/ui");
        globalThis.log=[];
        exports.OnPluginStart=()=>{ ui.onClicked(v=>log.push(`${v.buttonId}:${v.slot}:${v.player&&v.player.index}`)); };"#, "{}");
    assert!(!is_failed("hud-clicks"), "{:?}", FAILED_PLUGINS.with(|p| p.borrow().clone()));
    HUD_CLICKER.with(|c| c.set(crate::entity_live::on_created(611, 42)));
    let id = crate::engine_functions::registry::next_id().unwrap();
    HUD_FRAME.with(|f| f.set((id, -1, false)));
    let info = S2FunctionFrameInfo { version: 1, struct_size: 48, frame_token: id, native_epoch: id,
        invocation_id: id, suppressed_owner: 0, parameter_count: 4, flags: 0 };
    assert_eq!(crate::ffi::s2script_core_dispatch_function(target, &info, 0), 1);
    // Delivered within PRE (the original has not run yet) and committed as Continue.
    assert_eq!(frame_tests::eval_in_context_string("hud-clicks", "log.join('|')"), "Dismiss:-1:611");
    assert_eq!(HUD_FRAME.with(|f| f.get()), (id, 0, true));
    assert_eq!(crate::ffi::s2script_core_dispatch_function(target, &info, 1), 1);
    crate::entity_live::on_deleted(611, 42);
    unload_plugin("hud-clicks");
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

// A pointer-free native frame stack for the trusted takeDamageOld binding: victim receiver (-1),
// borrowed CTakeDamageInfo record (0: inflictor, attacker, damage, damageType) and the hidden
// result pass-through (1). The shim always runs the original (the adapter never suppresses), so a
// frame only records the committed action and the native damage the original would observe.
#[derive(Clone, Copy, Default)]
struct DamageFrame {
    token: u64,
    victim: Option<(i32, u64)>,
    attacker: Option<(i32, u64)>,
    inflictor: Option<(i32, u64)>,
    info: bool,
    damage: f32,
    damage_type: i32,
    action: Option<i32>,
    writes: u32,
}
thread_local! {
    static DAMAGE_FRAMES: std::cell::RefCell<Vec<DamageFrame>> = const { std::cell::RefCell::new(Vec::new()) };
    static DAMAGE_TARGET: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
    static DAMAGE_NEXT: std::cell::RefCell<Option<DamageFrame>> = const { std::cell::RefCell::new(None) };
}
fn with_damage_frame<R>(token: u64, f: impl FnOnce(&mut DamageFrame) -> R) -> Option<R> {
    DAMAGE_FRAMES.with(|s| s.borrow_mut().iter_mut().find(|d| d.token == token).map(f))
}
fn entity_value(e: Option<(i32, u64)>) -> S2FunctionValue {
    crate::engine_functions::projection::encode(crate::engine_functions::projection::ProjectedValue::Entity {
        reference: e.map(|(index, id)| crate::engine_functions::projection::EntityReference { index, id }),
        nullable: true,
    }).unwrap()
}
extern "C" fn damage_instance(access: *const S2FunctionInstanceAccess, selector: i32, out: *mut S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    with_damage_frame(unsafe { (*access).frame_token }, |f| {
        let value = match selector {
            -1 => entity_value(f.victim),
            0 => {
                let mut v = crate::engine_functions::runtime::blank();
                v.kind = 1;
                v.bits = f.info as u64;
                v
            }
            _ => return 0,
        };
        unsafe { *out = value };
        1
    }).unwrap_or(0)
}
extern "C" fn damage_field(access: *const S2FunctionInstanceAccess, selector: i32, field: u32, out: *mut S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    with_damage_frame(unsafe { (*access).frame_token }, |f| {
        if selector != 0 || !f.info { return 0; }
        let mut v = crate::engine_functions::runtime::blank();
        match field {
            0 => v = entity_value(f.inflictor),
            1 => v = entity_value(f.attacker),
            2 => { v.kind = 6; v.bits = f.damage.to_bits() as u64; }
            3 => { v.kind = 2; v.bits = f.damage_type as u32 as u64; }
            _ => return 0,
        }
        unsafe { *out = v };
        1
    }).unwrap_or(0)
}
extern "C" fn damage_field_write(access: *const S2FunctionInstanceAccess, selector: i32, field: u32,
    value: *const S2FunctionValue, _: *mut i8, _: i32) -> i32 {
    with_damage_frame(unsafe { (*access).frame_token }, |f| {
        if selector != 0 || field != 2 || !f.info || unsafe { (*value).kind } != 6 { return 0; }
        f.damage = f32::from_bits(unsafe { (*value).bits } as u32);
        f.writes += 1;
        1
    }).unwrap_or(0)
}
extern "C" fn damage_commit(_: i64, token: u64, _: u64, _: *const i8, action: i32, value: *const S2FunctionValue,
    _: *mut i8, _: i32) -> i32 {
    with_damage_frame(token, |f| {
        if !value.is_null() || f.action.is_some() { return 0; }
        f.action = Some(action);
        1
    }).unwrap_or(0)
}
/// One native TakeDamageOld call: PRE, the original (always — the adapter never suppresses), POST.
/// Returns the frame as the original observed it (after PRE's commit) and after POST.
fn damage_dispatch(frame: DamageFrame) -> (DamageFrame, DamageFrame) {
    let (original, after, pre) = damage_dispatch_status(frame);
    assert_eq!(pre, 1, "PRE");
    (original, after)
}
fn damage_dispatch_status(frame: DamageFrame) -> (DamageFrame, DamageFrame, i32) {
    let target = DAMAGE_TARGET.with(|t| t.get());
    let id = crate::engine_functions::registry::next_id().unwrap();
    DAMAGE_FRAMES.with(|s| s.borrow_mut().push(DamageFrame { token: id, ..frame }));
    let info = S2FunctionFrameInfo { version: 1, struct_size: 48, frame_token: id, native_epoch: id,
        invocation_id: id, suppressed_owner: 0, parameter_count: 2, flags: 0 };
    let pre = crate::ffi::s2script_core_dispatch_function(target, &info, 0);
    let original = with_damage_frame(id, |f| *f).unwrap();
    assert_eq!(crate::ffi::s2script_core_dispatch_function(target, &info, 1), 1, "POST");
    let after = DAMAGE_FRAMES.with(|s| {
        let mut s = s.borrow_mut();
        let at = s.iter().position(|d| d.token == id).unwrap();
        s.remove(at)
    });
    (original, after, pre)
}
/// `__damageMap()` — a map transition inside a callback (the entity books and record epoch roll).
fn damage_map(_: &mut v8::PinScope, _: v8::FunctionCallbackArguments, _: v8::ReturnValue) {
    crate::entity_live::clear_for_map_transition();
}
/// The identity-chunk slot check behind `EntityRef.isValid()`: any booked entity resolves here.
extern "C" fn damage_ent_resolve(index: c_int, _: c_int) -> *mut std::ffi::c_void {
    static SLOT: [u8; 8] = [0; 8];
    if crate::entity_live::lookup(index).is_some() { SLOT.as_ptr() as *mut _ } else { std::ptr::null_mut() }
}
/// `__damageNested()` — a plugin native that deals damage synchronously (published as a nest
/// token like every outbound native, so the inner dispatch runs under the caller's CallbackScope).
fn damage_nested(_: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let frame = DAMAGE_NEXT.with(|n| n.borrow_mut().take()).unwrap();
    let (original, _) = crate::nest::with_outbound(&args, || damage_dispatch(frame));
    rv.set_double(original.damage as f64);
}

/// The shipped package end to end on the real V8 facade: engine-generic SDKHook routes
/// OnTakeDamage/OnTakeDamagePost to the provider the CS2 package registered at bootstrap, which
/// subscribes to the trusted takeDamageOld binding through legacy.damage.v1.
#[test]
fn shipped_sdkhook_damage_routes_through_the_trusted_damage_adapter() {
    install_trusted_ops();
    let mut ops = engine_ops().unwrap();
    ops.function_frame_read_instance = Some(damage_instance);
    ops.function_frame_field_read = Some(damage_field);
    ops.function_frame_field_write = Some(damage_field_write);
    ops.function_frame_commit = Some(damage_commit);
    ops.ent_resolve = Some(damage_ent_resolve);
    set_engine_ops(Some(ops));
    let root = build_real_package("damage");
    let handle = crate::game_packages::select(&root, "source2", "csgo", "linuxsteamrt64").unwrap();
    crate::game_packages::commit(handle, &merged_signatures(&root), b"GCR1\0\0\0\0\0\0\0\0").unwrap();
    DAMAGE_TARGET.with(|t| t.set(TRUSTED_TARGETS.with(|t| t.borrow()["@s2script/cs2::takeDamageOld"])));
    let victim = crate::entity_live::on_created(5, 50);
    let other = crate::entity_live::on_created(6, 60);
    let attacker = crate::entity_live::on_created(1, 10);
    let plugin = r#"const {SDKHook,SDKUnhook,SDKHookType}=require("@s2script/sdk/sdkhooks");
        globalThis.log=[];globalThis.mode={};globalThis.saved=null;
        globalThis.pre=function(d){
          globalThis.saved=d;
          log.push(`pre:${d.damage}:${d.damageType}:${d.attacker&&d.attacker.index}:${d.inflictor}:${d.victim&&d.victim.index}`);
          if(mode.half)d.damage=d.damage/2;
          if(mode.nested){log.push('nested:'+__damageNested());}
          if(mode.throwAfter){d.damage=77;throw new Error('boom');}
          if(mode.map){__damageMap();try{d.damage=1;log.push('wrote')}catch(e){log.push('refused')}}
          if(mode.async){(async()=>{await null;try{log.push('late:'+d.damage)}catch(e){log.push('late:'+e.message)}})();}
          return mode.action;
        };
        globalThis.post=function(d){log.push(`post:${d.damage}`);d.damage=99;return 2;};
        globalThis.hookAll=function(v){return [SDKHook(v,SDKHookType.OnTakeDamage,pre),SDKHook(v,SDKHookType.OnTakeDamagePost,post)].join();};
        exports.OnPluginStart=()=>{};"#;
    for id in ["damage-a", "damage-b"] {
        load_plugin_js(id, plugin, "{}");
        assert!(!is_failed(id), "{:?}", FAILED_PLUGINS.with(|p| p.borrow().clone()));
        with_host_isolate(|isolate| {
            let mut storage = v8::HandleScope::new(isolate);
            let mut scope = unsafe { std::pin::Pin::new_unchecked(&mut storage) }.init();
            let context = clone_plugin_context(id).unwrap();
            let context = v8::Local::new(&mut scope, &context);
            let scope = &mut v8::ContextScope::new(&mut scope, context);
            let global = context.global(scope);
            let nested = v8::Function::new(scope, damage_nested).unwrap();
            let key = v8::String::new(scope, "__damageNested").unwrap();
            global.set(scope, key.into(), nested.into()).unwrap();
            let map = v8::Function::new(scope, damage_map).unwrap();
            let key = v8::String::new(scope, "__damageMap").unwrap();
            global.set(scope, key.into(), map.into()).unwrap();
        }).unwrap();
        // The registrar is bootstrap-only; the package provider is reached only through SDKHook.
        assert_eq!(frame_tests::eval_in_context_string(id, "typeof __s2_sdkhook_provider_register"), "undefined");
        assert_eq!(frame_tests::eval_in_context_string(id,
            &format!("globalThis.victim={{index:5,id:{victim}}};globalThis.other={{index:6,id:{other}}};hookAll(victim)")), "true,true");
    }
    let set = |a: &str, b: &str| {
        eval_in_context("damage-a", &format!("log.length=0;mode={a};")).unwrap();
        eval_in_context("damage-b", &format!("log.length=0;mode={b};")).unwrap();
    };
    let log = |id: &str| frame_tests::eval_in_context_string(id, "log.join('|')");
    let base = DamageFrame { victim: Some((5, victim)), attacker: Some((1, attacker)), inflictor: None, info: true,
        damage: 40.0, damage_type: 2, ..Default::default() };
    // A PRE write commits before the original; POST sees it, its write and return are ignored.
    set("{half:true}", "{}");
    let (original, after) = damage_dispatch(base);
    assert_eq!((original.damage, original.action, original.writes), (20.0, Some(0), 1));
    assert_eq!((after.damage, after.writes), (20.0, 1), "POST cannot write");
    assert_eq!(log("damage-a"), "pre:40:2:1:null:5|post:20");
    assert_eq!(log("damage-b"), "pre:20:2:1:null:5|post:20", "b sees a's accepted write");
    // The view is borrowed: it expires with the synchronous callback.
    assert!(frame_tests::eval_in_context_string("damage-a",
        "(()=>{try{return String(saved.damage)}catch(e){return e.message}})()").contains("expired borrowed view"));
    // Handled blocks by zeroing AFTER the fan-out: later handlers still run, the original still runs.
    set("{action:2}", "{half:true}");
    let (original, _) = damage_dispatch(base);
    assert_eq!((original.damage, original.action), (0.0, Some(0)), "block-to-zero with Continue, never a skip");
    assert_eq!(log("damage-b"), "pre:40:2:1:null:5|post:0");
    // Stop ends delivery (b never runs) and blocks.
    set("{action:3}", "{half:true}");
    let (original, _) = damage_dispatch(base);
    assert_eq!(original.damage, 0.0);
    assert_eq!(log("damage-b"), "post:0");
    // A throwing handler is Continue and keeps the write it made before throwing.
    set("{throwAfter:true}", "{}");
    let (original, _) = damage_dispatch(base);
    assert_eq!((original.damage, original.action), (77.0, Some(0)));
    // Per-entity filtering: a different victim runs nobody and writes nothing.
    set("{half:true}", "{half:true}");
    let (original, _) = damage_dispatch(DamageFrame { victim: Some((6, other)), ..base });
    assert_eq!((original.damage, original.writes), (40.0, 0));
    assert_eq!(log("damage-a"), "");
    // A null info degrades every field and cannot be written.
    let (original, _) = damage_dispatch(DamageFrame { info: false, ..base });
    assert_eq!(log("damage-a"), "pre:0:0:null:null:5|post:0");
    assert_eq!(original.writes, 0);
    // Nested damage: the busy context is skipped, the other plugin is delivered, the outer frame
    // stays valid and each frame commits its own value.
    set("{nested:true,half:true}", "{}");
    DAMAGE_NEXT.with(|n| *n.borrow_mut() = Some(DamageFrame { damage: 8.0, ..base }));
    let (original, _) = damage_dispatch(base);
    assert_eq!(original.damage, 20.0);
    assert_eq!(log("damage-a"), "pre:40:2:1:null:5|nested:8|post:20");
    assert_eq!(log("damage-b"), "pre:8:2:1:null:5|post:8|pre:20:2:1:null:5|post:20");
    // A view retained across await is expired.
    set("{async:true}", "{}");
    damage_dispatch(base);
    with_host_isolate(|isolate| isolate.perform_microtask_checkpoint()).unwrap();
    assert!(log("damage-a").ends_with("|late:DamageInfo: expired borrowed view (valid only during the synchronous SDKHook callback)"),
        "{}", log("damage-a"));
    // SDKUnhook removes exactly that registration.
    assert_eq!(frame_tests::eval_in_context_string("damage-a",
        "String(__s2require('@s2script/sdk/sdkhooks').SDKUnhook(victim,'OnTakeDamage',pre))"), "true");
    set("{}", "{}");
    damage_dispatch(base);
    assert_eq!(log("damage-a"), "post:40");
    // A stale entity is refused before reaching the provider.
    crate::entity_live::on_deleted(6, 60);
    assert_eq!(frame_tests::eval_in_context_string("damage-a", "hookAll(other)"), "false,false");
    // A map transition inside the callback expires the borrowed record: the write after it is
    // refused, nothing is committed, and the native frame is left exactly as the engine built it.
    set("{}", "{map:true}");
    let (original, _, pre) = damage_dispatch_status(base);
    assert_eq!(pre, 0, "the PRE dispatch names the expired record lifetime");
    assert_eq!((original.damage, original.writes, original.action), (40.0, 0, None));
    assert!(log("damage-b").starts_with("pre:40:2:1:null:5|refused"), "{}", log("damage-b"));
    for id in ["damage-a", "damage-b"] { unload_plugin(id); }
    // Unload disposes every receipt: nothing is delivered and the frame is untouched.
    let (original, _) = damage_dispatch(base);
    assert_eq!((original.damage, original.writes, original.action), (40.0, 0, None));
    shutdown();
    crate::game_packages::clear().unwrap();
    set_engine_ops(None);
    std::fs::remove_dir_all(root).unwrap();
}

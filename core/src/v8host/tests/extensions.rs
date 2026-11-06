//! Native extensions (`crate::extensions`, `extensions_v8.rs`): loading and refusal, the generic
//! and fast call paths, failures, entity arguments, declared imports, and the generated typings.
//!
//! The extensions are real shared libraries, compiled here with the system C compiler. The
//! well-behaved one is the worked example, `examples/extension-fastmath/native/fastmath.c`.
use super::*;
use crate::extensions::{self, State};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// A fresh extensions directory for one test.
fn ext_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("s2ext-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Compile `source` (a path or inline C) into `dir/<name>.so`.
fn compile(dir: &Path, name: &str, source: &str) {
    let src = if source.ends_with(".c") {
        repo().join(source)
    } else {
        let p = dir.join(format!("{name}.c"));
        std::fs::write(&p, source).unwrap();
        p
    };
    let out = std::process::Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-shared", "-fPIC", "-O2", "-I"])
        .arg(repo().join("packages/sdk/ext"))
        .arg("-o")
        .arg(dir.join(format!("{name}.so")))
        .arg(&src)
        .output()
        .expect("a C compiler (cc) is required for the extension tests");
    assert!(out.status.success(), "cc failed for {name}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A minimal extension whose `s2ext_init` body is `init`, with info fields overridable.
fn tiny(abi: &str, name: &str, init: &str) -> String {
    format!(
        r#"#include "s2ext.h"
static int32_t one(void* r) {{ (void)r; return 1; }}
static const S2ExtInfo INFO = {{sizeof(S2ExtInfo), {abi}, "{name}", "1.0.0"}};
S2EXT_EXPORT const S2ExtInfo* s2ext_info(void) {{ return &INFO; }}
S2EXT_EXPORT int32_t s2ext_init(const S2HostApi* h, S2Registrar* reg, const S2RegistrarApi* api) {{
  (void)h; (void)one;
  {init}
}}
"#
    )
}

thread_local! {
    static ENTITY_BUF: std::cell::Cell<*mut c_void> = const { std::cell::Cell::new(std::ptr::null_mut()) };
}
extern "C" fn test_ent_resolve(index: c_int, _serial: c_int) -> *mut c_void {
    if index == 9 { ENTITY_BUF.with(|b| b.get()) } else { std::ptr::null_mut() }
}
extern "C" fn test_schema_offset(class: *const c_char, field: *const c_char) -> c_int {
    let (c, f) = unsafe { (std::ffi::CStr::from_ptr(class), std::ffi::CStr::from_ptr(field)) };
    if c.to_bytes() == b"CThing" && f.to_bytes() == b"m_nValue" { 0x40 } else { -1 }
}

/// Load the fastmath example and give plugin `p` a context that declares it.
fn fastmath_setup(test: &str, range: &str) {
    extensions::reset_for_tests();
    crate::entity_live::reset_for_tests();
    let dir = ext_dir(test);
    compile(&dir, "fastmath", "examples/extension-fastmath/native/fastmath.c");
    let _ = init(dummy_logger());
    set_engine_ops(Some(S2EngineOps {
        ent_resolve: Some(test_ent_resolve),
        schema_offset: Some(test_schema_offset),
        ..mock_event_ops()
    }));
    extensions::load_dir(&dir);
    assert_eq!(extensions::state_of("@community/fastmath"), Some(State::Loaded));
    let hard: HashMap<String, String> = [("@community/fastmath".to_string(), range.to_string())].into();
    extensions::register_plugin("p", &hard);
    create_plugin_context("p");
    eval_in_context("p", "globalThis.m = __s2_require('@community/fastmath')").unwrap();
}

#[test]
fn extension_module_is_frozen_with_one_function_per_native() {
    fastmath_setup("shape", "^0.1.0");
    assert!(eval_in_context_bool("p", "m !== null && Object.isFrozen(m) && Object.getPrototypeOf(m) === null"));
    assert_eq!(
        eval_in_context_string("p", "Object.keys(m).join()"),
        "clamp,lerp,lerpf,isEven,label,parseNumber,fieldOffset,readEntityInt32"
    );
    assert_eq!(eval_in_context_string("p", "m.clamp.name + ':' + m.clamp.length"), "clamp:3");
    shutdown();
}

/// Every type crosses correctly on the generic path, including mixed integer and float
/// arguments (which travel in separate register files) and single-precision floats.
#[test]
fn extension_natives_return_correct_values_on_the_slow_path() {
    fastmath_setup("slow", "^0.1.0");
    let got = eval_in_context_string("p", r#"[
        m.clamp(150, 0, 100), m.clamp(-5, 0, 100), m.clamp(2**31 + 5, 0, 10),
        m.lerp(0, 10, 0.25), m.lerpf(0, 1, 0.5), m.lerpf(0, 0.1, 1),
        m.isEven(4), m.isEven(7), m.isEven(-2),
        m.label(3, 1.5, 'kg'), m.parseNumber(' 2.5'.trim()),
    ].join('|')"#);
    // 2**31+5 → ToInt32 = -2147483643 → clamps to 0. lerpf(0, 0.1, 1) is 0.1 in single precision.
    assert_eq!(got, format!("100|0|0|2.5|0.5|{}|true|false|true|3 x 1.50 kg|2.5", 0.1f32 as f64));
    shutdown();
}

/// Optimised code calls a fast native directly, with the same answers, and never enters the
/// host's slow callback.
#[test]
fn extension_fast_natives_bypass_the_slow_callback_once_optimized() {
    fastmath_setup("fast", "^0.1.0");
    let warm = eval_in_context_string("p", r#"
        function run(n) { var s = 0; for (var i = 0; i < n; i++) s += m.clamp(i & 255, 10, 200) + m.lerp(0, 2, 0.5); return s; }
        String(run(300000))
    "#);
    let expected: f64 = (0..300000).map(|i| ((i & 255).clamp(10, 200) as f64) + 1.0).sum();
    assert_eq!(warm, expected.to_string());
    let before = extensions_v8::SLOW_CALLS.with(|c| c.get());
    eval_in_context("p", "run(100000)").unwrap();
    let slow = extensions_v8::SLOW_CALLS.with(|c| c.get()) - before;
    assert!(slow < 1000, "{slow} of 200000 optimised calls took the slow path");
    shutdown();
}

#[test]
fn extension_failures_throw_s2error_with_the_extension_code() {
    fastmath_setup("fail", "^0.1.0");
    let got = eval_in_context_string("p", r#"
        var out = [];
        try { m.parseNumber('nope'); out.push('returned'); } catch (e) { out.push(e.name, e.code, e.message); }
        out.push(m.parseNumber('7'));
        try { m.clamp(1); } catch (e) { out.push(e.code); }
        try { m.label(1, 2, 'a\0b'); } catch (e) { out.push(e.code); }
        out.join('|')
    "#);
    // A failure does not leak into the next call; too few arguments and an embedded NUL are refused.
    assert_eq!(got, "S2Error|bad_number|not a number|7|arity|argument");
    shutdown();
}

#[test]
fn extension_host_services_and_entity_arguments() {
    fastmath_setup("entity", "^0.1.0");
    let buf: &'static mut [u8; 256] = Box::leak(Box::new([0u8; 256]));
    buf[0x40..0x44].copy_from_slice(&1234i32.to_le_bytes());
    ENTITY_BUF.with(|b| b.set(buf.as_mut_ptr() as *mut c_void));
    let id = crate::entity_live::on_created(9, 3);
    let got = eval_in_context_string("p", &format!(r#"
        var E = __s2require("@s2script/sdk/entity").EntityRef;
        var e = new E(9, {id});
        var off = m.fieldOffset('CThing', 'm_nValue');
        [off, m.fieldOffset('CThing', 'nope'), m.readEntityInt32(e, off)].join('|')
    "#));
    assert_eq!(got, "64|-1|1234");
    crate::entity_live::on_deleted(9, 3);
    let stale = eval_in_context_string("p", r#"
        var out = [];
        for (var v of [e, {index: 9, id: 1.5}, 7, null]) {
          try { m.readEntityInt32(v, 0); out.push('called'); } catch (x) { out.push(x.code); }
        }
        out.join('|')
    "#);
    assert_eq!(stale, "stale_entity|stale_entity|stale_entity|stale_entity");
    shutdown();
}

/// Imports are declared: an undeclared name never reaches an extension, and a missing one or one at
/// the wrong version refuses the plugin.
#[test]
fn extension_imports_follow_the_manifest() {
    fastmath_setup("imports", "^0.1.0");
    create_plugin_context("other");
    assert_eq!(eval_in_context_string("other", "String(__s2_require('@community/fastmath'))"), "null");
    // Declared at a version the loaded one does not satisfy (the load gate refuses this plugin
    // first; the resolver still answers null rather than the wrong version).
    extensions::register_plugin("other", &[("@community/fastmath".to_string(), "^2.0.0".to_string())].into());
    assert_eq!(eval_in_context_string("other", "String(__s2_require('@community/fastmath'))"), "null");
    let hard = |name: &str, range: &str| -> Result<(), String> {
        extensions::check_manifest(&[(name.to_string(), range.to_string())].into())
    };
    assert_eq!(hard("@community/fastmath", "^0.1.0"), Ok(()));
    assert_eq!(hard("@community/fastmath", "^2.0.0").unwrap_err(), "extension @community/fastmath is 0.1.0, the plugin needs ^2.0.0");
    assert!(hard("@community/absent", "^1.0.0").unwrap_err().contains("is not installed"));
    let manifest: crate::loader::Manifest = serde_json::from_str(
        r#"{"id":"x","version":"1.0.0","apiVersion":"3.x","extensions":{"@community/fastmath":"^0.1.0"}}"#,
    ).unwrap();
    assert_eq!(manifest.extensions["@community/fastmath"], "^0.1.0");
    shutdown();
}

/// Through the real plugin path: the bundle's own `require` (what esbuild emits for an import of a
/// declared extension) reaches the module.
#[test]
fn a_plugin_bundle_requires_its_declared_extension() {
    extensions::reset_for_tests();
    let dir = ext_dir("bundle");
    compile(&dir, "fastmath", "examples/extension-fastmath/native/fastmath.c");
    LOG.lock().unwrap().clear();
    init(logger).unwrap();
    extensions::load_dir(&dir);
    extensions::register_plugin("bundle", &[("@community/fastmath".to_string(), "^0.1.0".to_string())].into());
    load_plugin_js("bundle", r#"
        var import_fastmath = require("@community/fastmath");
        exports.OnPluginStart = function () {
          console.log("FASTMATH:" + (0, import_fastmath.clamp)(150, 0, 100) + "," + import_fastmath.label(2, 0.5, "m"));
        };
    "#, "{}");
    let logs = LOG.lock().unwrap().join("\n");
    assert!(logs.contains("FASTMATH:100,2 x 0.50 m"), "{logs}");
    assert!(logs.contains("[ext @community/fastmath] registered 8 natives"), "{logs}");
    shutdown();
}

/// Each broken extension is refused with a named reason, and the good one beside it still loads.
#[test]
fn broken_extensions_are_refused_and_the_rest_load() {
    extensions::reset_for_tests();
    let _ = init(dummy_logger());
    let dir = ext_dir("refusals");
    compile(&dir, "a_good", "examples/extension-fastmath/native/fastmath.c");
    compile(&dir, "b_abi", &tiny("2", "@t/abi", "return 0;"));
    compile(&dir, "c_init", &tiny("S2EXT_ABI_VERSION", "@t/init", "return 7;"));
    compile(&dir, "d_sig", &tiny("S2EXT_ABI_VERSION", "@t/sig", r#"
        static const S2Type six[] = {S2T_I32, S2T_I32, S2T_I32, S2T_I32, S2T_I32, S2T_I32};
        S2NativeDesc d = {sizeof(S2NativeDesc), "six", (void*)one, S2T_I32, six, 6, 0, 0, 0};
        if (api->native(reg, &d)) return 1;
        return 0;"#));
    compile(&dir, "e_name", &tiny("S2EXT_ABI_VERSION", "Bad Name", "return 0;"));
    compile(&dir, "f_dup", &tiny("S2EXT_ABI_VERSION", "@community/fastmath", "return 0;"));
    compile(&dir, "g_ident", &tiny("S2EXT_ABI_VERSION", "@t/ident", r#"
        S2NativeDesc d = {sizeof(S2NativeDesc), "not-ident", (void*)one, S2T_I32, 0, 0, 0, 0, 0};
        return api->native(reg, &d);"#));
    compile(&dir, "h_nosym", "int unrelated(void) { return 0; }");
    std::fs::write(dir.join("i_garbage.so"), b"not an elf").unwrap();
    // An extension installed as a symlink loads like a copy (here, a link to the good one, which
    // is then refused only as a duplicate name).
    std::os::unix::fs::symlink(dir.join("a_good.so"), dir.join("j_link.so")).unwrap();
    extensions::load_dir(&dir);
    assert_eq!(extensions::state_of("@community/fastmath"), Some(State::Loaded));
    let reason = |n: &str| match extensions::state_of(n) {
        Some(State::Refused(r)) => r,
        other => panic!("{n}: {other:?}"),
    };
    assert_eq!(reason("@t/abi"), "built for extension ABI 2, this host speaks ABI 1");
    assert_eq!(reason("@t/init"), "s2ext_init returned 7");
    assert_eq!(reason("@t/sig"), "native \"six\": more than 5 integer-class arguments");
    assert_eq!(reason("Bad Name"), "name \"Bad Name\" is not a valid package name");
    assert_eq!(reason("f_dup.so"), "another file already loaded @community/fastmath");
    assert!(reason("@t/ident").contains("must be a JavaScript identifier"));
    assert_eq!(reason("h_nosym.so"), "does not export s2ext_info and s2ext_init");
    assert!(reason("i_garbage.so").starts_with("dlopen:"));
    assert_eq!(reason("j_link.so"), "another file already loaded @community/fastmath");
    let list = extensions::list_lines().join("\n");
    assert!(list.contains("[S2S] Extensions (10):") && list.contains("@community/fastmath@0.1.0 (8 natives"), "{list}");
    assert!(list.contains("@t/abi@1.0.0 (refused: built for extension ABI 2"), "{list}");
    // A second scan is a no-op: extensions load once per process.
    extensions::load_dir(&dir);
    assert!(extensions::list_lines()[0].contains("(10)"));
    shutdown();
}

/// The example's hand-shipped typings are exactly what the host generates from its registrations.
/// Regenerate: `S2_BLESS=1 cargo test -p s2script-core fastmath_typings_are_fresh`.
#[test]
fn fastmath_typings_are_fresh() {
    extensions::reset_for_tests();
    let _ = init(dummy_logger());
    let dir = ext_dir("typings");
    compile(&dir, "fastmath", "examples/extension-fastmath/native/fastmath.c");
    extensions::load_dir(&dir);
    let generated = extensions::types("@community/fastmath").unwrap();
    let path = repo().join("examples/extension-fastmath/.s2script/extensions/@community/fastmath/index.d.ts");
    if std::env::var_os("S2_BLESS").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(committed, generated, "examples/extension-fastmath/.s2script/extensions/@community/fastmath/index.d.ts is stale; rerun with S2_BLESS=1");
    shutdown();
}

/// Per-call cost of an extension native from optimised JS (fast path) and of the generic path.
/// `cargo test --release -p s2script-core extension_bench -- --ignored --nocapture`
#[test]
#[ignore]
fn extension_bench_call_cost() {
    fastmath_setup("bench", "^0.1.0");
    let out = eval_in_context_string("p", r#"
        function jsClamp(v, lo, hi) { return v < lo ? lo : v > hi ? hi : v; }
        function time(f, n) { f(n / 10); var t = Date.now(); f(n); return ((Date.now() - t) * 1e6 / n).toFixed(1); }
        var n = 20000000;
        var fast = time(function (k) { var s = 0; for (var i = 0; i < k; i++) s += m.clamp(i & 255, 10, 200); return s; }, n);
        var js = time(function (k) { var s = 0; for (var i = 0; i < k; i++) s += jsClamp(i & 255, 10, 200); return s; }, n);
        var str = time(function (k) { var s = 0; for (var i = 0; i < k; i++) s += m.fieldOffset('CThing', 'm_nValue'); return s; }, n / 20);
        var lab = time(function (k) { var s = 0; for (var i = 0; i < k; i++) s += m.label(i, 1.5, 'kg').length; return s; }, n / 20);
        var slowNum = time(function (k) { var s = 0; for (var i = 0; i < k; i++) s += m.clamp('5', 10, 200); return s; }, n / 20);
        'slow-path clamp ' + slowNum + ' ns, fast native ' + fast + ' ns, plain JS ' + js + ' ns, generic path: fieldOffset(two strings) ' + str + ' ns, label(int, double, string) -> string ' + lab + ' ns'
    "#);
    println!("{out}");
    shutdown();
}

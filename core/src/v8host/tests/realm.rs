use super::*;

const SHARED: &str = r#"
  var M = __s2_shared_module("@test/shared:1", function () {
    var made = { id: Math.random(), offset: __s2_schema_offset("C", "f") };
    return { made: made, view: function (ref) { return Object.create({ get v() { return ref.read(); } }); } };
  });
  globalThis.__s2pkg_cs2 = { M: M };
"#;

fn loaded(id: &str) {
    load_body(id, "globalThis.__M = require(\"@s2script/cs2\").M;", "{}");
}

/// Two plugins get the one framework-realm instance: the factory ran once, and both see its object.
#[test]
fn shared_module_is_built_once_for_every_plugin() {
    let _ = init(dummy_logger());
    register_injected_package("@s2script/cs2", SHARED);
    loaded("a");
    loaded("b");
    let a = eval_in_context_string("a", "String(__M.made.id)");
    assert_eq!(a, eval_in_context_string("b", "String(__M.made.id)"));
    assert_eq!(realm::module_count(), 1);
    // The realm's one native resolves offsets as in a plugin (no engine here: the -1 miss).
    assert_eq!(eval_in_context_string("a", "String(__M.made.offset)"), "-1");
    // The bootstrap-only native is gone once the package has run.
    assert_eq!(eval_in_context_string("a", "typeof __s2_shared_module"), "undefined");
    register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
    shutdown();
}

/// A shared module runs its accessors against the receiver the plugin passes, so values come back
/// in the plugin's realm; and no plugin can change the shared objects or the realm's builtins.
#[test]
fn shared_module_is_frozen_and_reads_through_the_plugin_receiver() {
    let _ = init(dummy_logger());
    register_injected_package("@s2script/cs2", SHARED);
    loaded("a");
    loaded("b");
    assert_eq!(
        eval_in_context_string("a", "var arr = []; String(__M.view({ read: function () { return arr; } }).v === arr)"),
        "true"
    );
    eval_in_context_string("a", r#"
        try { __M.made = null; } catch (e) {}
        try { __M.extra = 1; } catch (e) {}
        try { Object.getPrototypeOf(__M).polluted = 1; } catch (e) {}
        try { Object.getPrototypeOf(__M.view).call = null; } catch (e) {}
        "done""#);
    assert_eq!(
        eval_in_context_string("b", "String(__M.made !== null && __M.extra === undefined && Object.getPrototypeOf(__M).polluted === undefined && typeof Object.getPrototypeOf(__M.view).call)"),
        "function"
    );
    register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
    shutdown();
}

/// A factory that throws fails that plugin's bootstrap with a named reason; it is not cached, so a
/// fixed package builds it on the next load.
#[test]
fn a_failing_shared_module_is_not_cached() {
    let _ = init(dummy_logger());
    register_injected_package("@s2script/cs2", r#"
        try { __s2_shared_module("@test/bad:1", function () { throw new Error("boom"); }); }
        catch (e) { globalThis.__s2pkg_cs2 = { error: String(e.message) }; }
    "#);
    load_body("a", "globalThis.__E = require(\"@s2script/cs2\").error;", "{}");
    assert_eq!(read_global_string("a", "__E"), "shared module '@test/bad:1': Error: boom");
    assert_eq!(realm::module_count(), 0);
    register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
    shutdown();
}

/// The engine prelude and a package are compiled once and bound to each new context; a package
/// registered again with new source runs the new source, never a cached older compile.
#[test]
fn package_scripts_compile_once_and_never_go_stale() {
    let _ = init(dummy_logger());
    register_injected_package("@s2script/cs2", "globalThis.__s2pkg_cs2 = { v: 1 };");
    load_body("a", "", "{}");
    let after_first = realm::script_count();
    load_body("b", "", "{}");
    assert_eq!(realm::script_count(), after_first, "a second plugin reuses both compiled scripts");
    register_injected_package("@s2script/cs2", "globalThis.__s2pkg_cs2 = { v: 2 };");
    load_body("c", "globalThis.__V = String(require(\"@s2script/cs2\").v);", "{}");
    assert_eq!(read_global_string("c", "__V"), "2");
    register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
    shutdown();
}

use super::*;

/// The concatenated CS2 package bootstrap, in `game-package.jsonc` order.
fn cs2_bootstrap() -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../games/cs2/js");
    ["schema.generated.js", "nav.generated.js", "activity.js", "csitem.generated.js", "weapon.js",
     "pawn.js", "camera.js", "ui.js", "components.js", "hudinput.js", "menuhud.js", "voterail.js"]
        .iter()
        .map(|f| std::fs::read_to_string(root.join(f)).unwrap())
        .collect::<Vec<_>>()
        .join("\n;\n")
}

/// What each extra plugin context costs with the real CS2 package (time and retained V8 heap).
/// `cargo test -p s2script-core --release realm_bench -- --ignored --nocapture`
#[test]
#[ignore]
fn realm_bench_per_context_cost() {
    let _ = init(dummy_logger());
    register_injected_package("@s2script/cs2", &cs2_bootstrap());
    load_body("warm", "", "{}");
    let heap = || eval_in_context_string("warm", "__s2_v8_gc(); String(__s2_v8_heap_used())").parse::<f64>().unwrap();
    let before = heap();
    let n = 16;
    let t = std::time::Instant::now();
    for i in 0..n { load_body(&format!("p{i}"), "", "{}"); }
    let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    eprintln!("per context: {per:.2} ms, {:.2} MB", (heap() - before) / n as f64 / 1048576.0);
    register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
    shutdown();
}

/// A field read through an accessor that lives in the framework realm, next to the same accessor
/// defined in the plugin's own context (V8 doesn't inline a getter from another context).
#[test]
#[ignore]
fn realm_bench_cross_realm_read() {
    let _ = init(dummy_logger());
    set_engine_ops(None);
    register_injected_package("@s2script/cs2", r#"
      var M = __s2_shared_module("@bench/view:1", function () {
        var proto = {};
        Object.defineProperty(proto, "health", { get: function () { return this.ref.readInt32(8); } });
        return { wrap: function (ref) { var o = Object.create(proto); o.ref = ref; return o; } };
      });
      globalThis.__s2pkg_cs2 = { wrap: M.wrap };
    "#);
    create_plugin_context("bench");
    let buf = vec![7u8; 64];
    let addr = buf.as_ptr() as usize;
    let out = eval_in_context_string("bench", &format!(r#"
        (function () {{
          const {{ Ptr }} = __s2require("@s2script/sdk/unsafe");
          const p = Ptr.from({addr});
          const proto = {{}};
          Object.defineProperty(proto, "health", {{ get: function () {{ return this.ref.readInt32(8); }} }});
          const local = Object.create(proto); local.ref = p;
          const shared = __s2require("@s2script/cs2").wrap(p);
          const N = 2000000;
          function run(f) {{
            let best = Infinity, sink = 0;
            for (let r = 0; r < 5; r++) {{
              const t0 = Number(__s2_hrtime_ns());
              for (let i = 0; i < N; i++) sink += f();
              best = Math.min(best, (Number(__s2_hrtime_ns()) - t0) / N);
            }}
            return best.toFixed(1) + (sink === -1 ? "" : "");
          }}
          return "local " + run(() => local.health) + " ns, shared " + run(() => shared.health) + " ns";
        }})()
    "#));
    println!("view read: {out}");
    drop(buf);
    register_injected_package("@s2script/cs2", r#"globalThis.__s2pkg_cs2 = {};"#);
    shutdown();
}

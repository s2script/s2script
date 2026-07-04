# Slice 5E.2 — config materialization — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plugins declare typed config in `package.json`; the host materializes it at load (declared defaults merged with an auto-generated, admin-editable JSON override file); the plugin reads it via a typed `@s2script/config`, with opt-in live-reload via `config.onChange`.

**Architecture:** The CLI bakes `s2script.config` into `manifest.json`. At load, the loader materializes config (a pure core `materialize_config` merges defaults ⊕ the override file read by a new `config_read` shim op; `config_write` auto-generates the file) and injects a per-context `globalThis.__s2pkg_config_values`. A shared `@s2script/config` prelude module reads it via typed getters; `config.onChange` opts a plugin into live-reload — the loader's frame-drain poll stats the config file, re-materializes on change, and fires the handlers.

**Tech Stack:** Rust core (serde_json), C++ shim (file I/O), the injected JS prelude, the `@s2script/config` `.d.ts`, esbuild CLI, Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-07-03-slice-5e2-config-materialization-design.md`.

## Global Constraints

- **Config is engine-generic** — `@s2script/config`, `materialize_config`, and the ops live in core/shim (every plugin has settings). No CS2 identifier in `core/src`; both boundary gates green.
- **Four types:** `string`/`int`/`float`/`bool`. A decl = `{ type, default, description? }`. The CLI validates `default` matches `type` at build (fail); `materialize_config` degrades as a backstop (mismatch → WARN + type zero-value).
- **Materialize = defaults ⊕ override file, per-key.** Override file `addons/s2script/configs/<sanitized-id>.json` (JSONC), auto-generated with defaults + `//`-description on first run. A wrong-typed/malformed key → the default + a WARN. Undeclared file keys ignored. Type zero-values: `""` / `0` / `0` / `false`.
- **Degrade-never-crash:** absent file → auto-generate + defaults; malformed → per-key default + WARN; `config_read`/`config_write` op absent → defaults-only; undeclared key read → zero-value + WARN. No path throws into plugin JS.
- **`config.onChange` is opt-in live-reload** (the lazy-hook pattern): no subscription → read-only at load, no watch; the first subscription → the loader watches that plugin's config mtime, re-materializes on change, re-injects `__s2pkg_config_values`, fires the handlers. Auto-ledgered; stops on unload.
- **ABI append-only:** `config_read`/`config_write` appended to `S2EngineOps` after the 5D.3 event ops — C header AND Rust mirror, same order.
- **Commit trailer:** every commit ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`.
- **Test runners:** core = `cargo test -p s2script-core -- --test-threads=1`; CLI/JS = `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs`.
- **Pure-ESM authoring** (5E.1): `@s2script/config` is imported ESM; the demo passes the typecheck gate.

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `packages/cli/src/build.ts` | Modify | copy `s2script.config` into `manifest.json` |
| `packages/cli/src/config-validate.ts` | Create | `validateConfigBlock(config)` — `default` matches `type` (build gate) |
| `core/src/config.rs` | Create | `ConfigDecl`, `materialize_config`, `generate_default_jsonc`, type helpers |
| `core/src/loader.rs` | Modify | `Manifest.config`; materialize at load; the config-mtime watch in `poll_plugins` |
| `core/src/v8host.rs` | Modify | inject `__s2pkg_config_values`; the `@s2script/config` prelude + `onChange` native + config-mux + re-materialize; `S2EngineOps` Rust mirror append |
| `core/src/ffi.rs` | Modify (if needed) | — (no new C→core dispatch; watch reuses the frame drain) |
| `shim/include/s2script_core.h` | Modify | `config_read`/`config_write` op typedefs + struct append |
| `shim/src/s2script_mm.cpp` | Modify | `ConfigPath(id)` + `s2_config_read`/`s2_config_write` impls + wire into `S2EngineOps` |
| `packages/config/{package.json,index.d.ts}` | Create | the `@s2script/config` types-only package |
| `packages/cli/test/*.mjs`, `core/src/config.rs` (tests) | Create/Modify | CLI + core tests |
| `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md` | Modify | demo + docs |

---

## Task 1: CLI — `s2script.config` → manifest + build validation

**Files:**
- Modify: `packages/cli/src/build.ts`
- Create: `packages/cli/src/config-validate.ts`, `packages/cli/test/config-validate.test.mjs`

**Interfaces:**
- Produces: `validateConfigBlock(config: unknown): string[]` (returns error messages, empty = ok); `build.ts` copies `config` into `manifest.json`.

- [ ] **Step 1: Write the failing test (`packages/cli/test/config-validate.test.mjs`)**

```javascript
import { test } from "node:test";
import assert from "node:assert";
import { validateConfigBlock } from "../src/config-validate.ts";

test("valid config block → no errors", () => {
  assert.deepEqual(validateConfigBlock({
    greeting: { type: "string", default: "hi" },
    n: { type: "int", default: 3 }, f: { type: "float", default: 1.5 }, b: { type: "bool", default: true },
  }), []);
});
test("default not matching type → an error naming the key", () => {
  const errs = validateConfigBlock({ n: { type: "int", default: "oops" } });
  assert.equal(errs.length, 1);
  assert.match(errs[0], /n.*int/);
});
test("int rejects a non-integer default", () => {
  assert.equal(validateConfigBlock({ n: { type: "int", default: 1.5 } }).length, 1);
});
test("unknown type → an error", () => {
  assert.equal(validateConfigBlock({ x: { type: "date", default: 0 } }).length, 1);
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/config-validate.test.mjs`
Expected: FAIL — `Cannot find module '../src/config-validate.ts'`.

- [ ] **Step 3: Implement `packages/cli/src/config-validate.ts`**

```typescript
/** Validate an s2script.config block: each entry is { type, default, description? } and `default`
 *  matches `type`. Returns human error messages (empty = valid). Types: string/int/float/bool. */
export function validateConfigBlock(config: unknown): string[] {
  const errs: string[] = [];
  if (config == null) return errs;
  if (typeof config !== "object" || Array.isArray(config)) return ["s2script.config must be an object"];
  for (const [key, raw] of Object.entries(config as Record<string, unknown>)) {
    if (typeof raw !== "object" || raw === null) { errs.push(`config '${key}': must be { type, default }`); continue; }
    const decl = raw as { type?: unknown; default?: unknown };
    const t = decl.type, d = decl.default;
    if (t !== "string" && t !== "int" && t !== "float" && t !== "bool") {
      errs.push(`config '${key}': unknown type ${JSON.stringify(t)} (want string|int|float|bool)`); continue;
    }
    const ok =
      (t === "string" && typeof d === "string") ||
      (t === "int" && typeof d === "number" && Number.isInteger(d)) ||
      (t === "float" && typeof d === "number") ||
      (t === "bool" && typeof d === "boolean");
    if (!ok) errs.push(`config '${key}': default ${JSON.stringify(d)} does not match type '${t}'`);
  }
  return errs;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/config-validate.test.mjs`
Expected: PASS.

- [ ] **Step 5: Wire into `buildPlugin` (`packages/cli/src/build.ts`)**

Add the import: `import { validateConfigBlock } from "./config-validate.ts";`. In `buildPlugin`, after reading `pkg`/`s2` and BEFORE (or right after) the typecheck, validate + copy:

```typescript
  const config = s2.config ?? undefined;
  if (config !== undefined) {
    const cfgErrs = validateConfigBlock(config);
    if (cfgErrs.length) throw new Error(`invalid s2script.config:\n  ${cfgErrs.join("\n  ")}`);
  }
```

Add `config?: Record<string, unknown>;` to the `PluginPackageJson.s2script` interface, and include `config` in the manifest object (only when defined):

```typescript
  if (config !== undefined) manifest.config = config;
```

- [ ] **Step 6: Run the full CLI suite + commit**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs`
Expected: all pass.

```bash
git add packages/cli/src/config-validate.ts packages/cli/src/build.ts packages/cli/test/config-validate.test.mjs
git commit -m "$(printf 'feat(slice5e2): CLI validates s2script.config + bakes it into the manifest\n\nvalidateConfigBlock checks each { type, default } (string/int/float/bool; default matches type) and\nfails the build on a mismatch; buildPlugin copies config into manifest.json.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 2: Core — `materialize_config` (pure) + `Manifest.config`

**Files:**
- Create: `core/src/config.rs`
- Modify: `core/src/loader.rs` (add `config` to `Manifest`; register `mod config`), `core/src/lib.rs` (if it lists modules)

**Interfaces:**
- Produces: `ConfigDecl { r#type: String, default: serde_json::Value, description: Option<String> }`; `pub fn materialize_config(decls: &HashMap<String, ConfigDecl>, override_json: Option<&str>) -> MaterializeResult { values: serde_json::Map<String, serde_json::Value>, warnings: Vec<String> }`; `pub fn generate_default_jsonc(decls: &HashMap<String, ConfigDecl>) -> String`.

- [ ] **Step 1: Write the failing tests (`core/src/config.rs`, `#[cfg(test)]`)**

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core config::tests -- --test-threads=1`
Expected: FAIL — `config` module not found.

- [ ] **Step 3: Implement `core/src/config.rs`**

```rust
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
```

Register the module + add the manifest field. In `core/src/loader.rs` add `config` to `Manifest`:

```rust
    #[serde(default)]
    pub config: std::collections::HashMap<String, crate::config::ConfigDecl>,
```

Add `pub mod config;` where the core modules are declared (grep `mod loader;`/`mod event_mux;` in `core/src/lib.rs` and add `pub mod config;` alongside).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p s2script-core config::tests -- --test-threads=1`
Expected: PASS (all 5).

- [ ] **Step 5: Full core suite + boundary + commit**

Run: `cargo test -p s2script-core -- --test-threads=1 && bash scripts/check-core-boundary.sh`
Expected: all pass; boundary EXIT 0 (config.rs is engine-generic).

```bash
git add core/src/config.rs core/src/loader.rs core/src/lib.rs
git commit -m "$(printf 'feat(slice5e2): materialize_config (pure) + Manifest.config\n\nEngine-generic, V8-free core/src/config.rs: ConfigDecl + materialize_config (defaults + override, per-key\ntype-check, malformed/wrong-type/bad-default all degrade with a WARN) + generate_default_jsonc (stable,\nsorted, //-commented). Manifest carries config. 5 in-isolate tests.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 3: Shim ops + materialize-at-load + inject

**Files:**
- Modify: `shim/include/s2script_core.h`, `shim/src/s2script_mm.cpp`, `core/src/v8host.rs`, `core/src/loader.rs`

**Interfaces:**
- Consumes: `materialize_config`/`generate_default_jsonc` (Task 2); the `S2EngineOps` pattern; `load_plugin_js`/`create_plugin_context`.
- Produces: ops `config_read(id) -> *const c_char` (override file content, or null if absent), `config_write(id, content) -> c_int`; `load_plugin_js(id, js, config_values_json: &str)` (config injected before eval).

- [ ] **Step 1: Append the op typedefs + struct fields (`shim/include/s2script_core.h`)**

After the last event op typedef:

```c
/* Config ops (Slice 5E.2). Read/auto-generate the admin override file addons/s2script/configs/<id>.json. */
typedef const char* (*s2_config_read_fn)(const char* id);            /* file content, or null if absent; valid until the next config_read */
typedef int         (*s2_config_write_fn)(const char* id, const char* content); /* 1 ok / 0 fail */
```

APPEND to `S2EngineOps` after `event_fire`:

```c
    /* Config ops (Slice 5E.2) — APPENDED after the event ops; order is the ABI. */
    s2_config_read_fn  config_read;
    s2_config_write_fn config_write;
```

- [ ] **Step 2: Rust mirror (`core/src/v8host.rs`)**

After `EventFireFn`:

```rust
pub type ConfigReadFn  = extern "C" fn(id: *const c_char) -> *const c_char;
pub type ConfigWriteFn = extern "C" fn(id: *const c_char, content: *const c_char) -> c_int;
```

APPEND to `struct S2EngineOps` after `event_fire`:

```rust
    pub config_read:  Option<ConfigReadFn>,
    pub config_write: Option<ConfigWriteFn>,
```

- [ ] **Step 3: Shim impls + wiring (`shim/src/s2script_mm.cpp`)**

Add a `ConfigPath(id)` helper (mirror `PluginsDir` — dladdr to the addon root, then `/configs/<sanitized id>.json`; sanitize `id` the same way the CLI sanitizes: non-`[A-Za-z0-9._-]` → `_`). Add the op impls (a `static std::string` holds the read buffer so the returned `const char*` stays valid until the next call — mirror `s2_event_get_string`'s buffer discipline):

```cpp
static std::string s_configReadBuf;
static const char* s2_config_read(const char* id) {
    if (!id) return nullptr;
    std::ifstream f(ConfigPath(id));
    if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf();
    s_configReadBuf = ss.str();
    return s_configReadBuf.c_str();
}
static int s2_config_write(const char* id, const char* content) {
    if (!id || !content) return 0;
    std::string path = ConfigPath(id);
    // mkdir -p the configs dir (mirror the plugins-dir handling); then write.
    std::error_code ec; std::filesystem::create_directories(std::filesystem::path(path).parent_path(), ec);
    std::ofstream f(path); if (!f) return 0; f << content; return f.good() ? 1 : 0;
}
```

Wire into `S2EngineOps ops = {}` after `ops.event_fire`:

```cpp
    ops.config_read  = &s2_config_read;
    ops.config_write = &s2_config_write;
```

(Add `#include <sstream>`/`<fstream>`/`<filesystem>` if not present.)

- [ ] **Step 4: Materialize at load + inject (`core/src/loader.rs` + `core/src/v8host.rs`)**

In `core/src/v8host.rs`, add a helper that materializes + injects, and change `load_plugin_js` to accept the manifest config decls:

```rust
/// Materialize a plugin's config (defaults ⊕ the override file read via the config_read op; auto-generate
/// the file via config_write if absent) and return the values JSON to inject. Degrade: no ops → defaults.
pub(crate) fn materialize_for_load(id: &str, decls: &std::collections::HashMap<String, crate::config::ConfigDecl>) -> String {
    if decls.is_empty() { return "{}".to_string(); }
    let ops = ENGINE_OPS.with(|o| o.get());
    let cid = std::ffi::CString::new(id).ok();
    // read the override file
    let override_json: Option<String> = (|| {
        let ops = ops?; let f = ops.config_read?; let cid = cid.as_ref()?;
        let ptr = f(cid.as_ptr()); if ptr.is_null() { return None; }
        Some(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
    })();
    let was_absent = override_json.is_none();
    let mat = crate::config::materialize_config(decls, override_json.as_deref());
    for w in &mat.warnings { log_warn(&format!("config('{}'): {}", id, w)); }
    if was_absent {  // auto-generate the default file
        if let (Some(ops), Some(cid)) = (ops, cid.as_ref()) {
            if let Some(wf) = ops.config_write {
                if let Ok(content) = std::ffi::CString::new(crate::config::generate_default_jsonc(decls)) {
                    wf(cid.as_ptr(), content.as_ptr());
                }
            }
        }
    }
    serde_json::to_string(&serde_json::Value::Object(mat.values)).unwrap_or_else(|_| "{}".to_string())
}
```

Change `load_plugin_js(id, js)` → `load_plugin_js(id, js, config_values_json: &str)`; after `create_plugin_context(id)` and before the wrapper eval, inject the per-context values:

```rust
    // Inject the materialized config as a per-context global BEFORE the plugin evals (so config reads
    // in onLoad see it). @s2script/config's getters read globalThis.__s2pkg_config_values.
    let _ = eval_in_context(id, &format!("globalThis.__s2pkg_config_values = {};", config_values_json));
```

In `core/src/loader.rs`, at both `load_plugin_js(&manifest.id, &js)` call sites, materialize first + pass it:

```rust
                    let cfg = crate::v8host::materialize_for_load(&manifest.id, &manifest.config);
                    crate::v8host::load_plugin_js(&manifest.id, &js, &cfg);
```

Update the existing core tests that call `load_plugin_js(id, js)` to pass `"{}"` as the third arg.

- [ ] **Step 5: Verify (core compiles+tests; shim compile deferred to Task 5)**

Run:
```bash
cargo test -p s2script-core -- --test-threads=1
grep -c "config_read\|config_write" shim/src/s2script_mm.cpp   # expect >= 3
```
Expected: core green (existing tests updated for the new `load_plugin_js` arg); grep ≥ 3. Do NOT compile the shim (Task 5's sniper build).

- [ ] **Step 6: Commit**

```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp core/src/v8host.rs core/src/loader.rs
git commit -m "$(printf 'feat(slice5e2): config_read/config_write ops + materialize-at-load + inject\n\nconfig_read/config_write ops (ABI-appended, C header + Rust mirror); shim reads/auto-writes\naddons/s2script/configs/<id>.json. load path materializes (defaults + override, auto-generate if absent)\nand injects globalThis.__s2pkg_config_values before the plugin evals. load_plugin_js gains the values arg.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 4: `@s2script/config` runtime + `onChange` live-reload

**Files:**
- Modify: `core/src/v8host.rs` (prelude module + `onChange` native + config-mux + re-materialize), `core/src/loader.rs` (config-mtime watch in `poll_plugins`)
- Create: `packages/config/package.json`, `packages/config/index.d.ts`, `packages/cli/test/config-runtime.test.mjs`

**Interfaces:**
- Consumes: `__s2pkg_config_values` (Task 3); `materialize_for_load`; the `EVENT_MUX`/lazy-hook patterns.
- Produces: `@s2script/config` (`getString`/`getInt`/`getFloat`/`getBool`/`onChange`); a `re_materialize_config(id)` core fn the loader calls on a config-file change.

- [ ] **Step 1: The `@s2script/config` prelude module (`core/src/v8host.rs`)**

In the prelude, add the module object (reads the per-context `__s2pkg_config_values`; coerces to the requested type; undeclared → zero-value; `onChange` registers via a native). Place it with the other `__s2pkg_*`:

```javascript
  var __s2_config = {
    getString: function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return v == null ? "" : String(v); },
    getInt:    function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return (v == null || typeof v !== "number") ? 0 : (v | 0); },
    getFloat:  function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return (v == null || typeof v !== "number") ? 0 : v; },
    getBool:   function (k) { var v = globalThis.__s2pkg_config_values; v = v && v[k]; return v === true; },
    onChange:  function (h) { __s2_config_on_change(h); },
  };
```

and register it: `globalThis.__s2pkg_config = __s2_config;`.

- [ ] **Step 2: The `onChange` native + config-mux (`core/src/v8host.rs`)**

Add a thread-local `CONFIG_SUBS: RefCell<EventMux<Global<Function>>>` keyed by owner-id (reuse `EventMux`; one "slot" per plugin — subscribe under a fixed name like `"config"`). Add the native (mirror `s2_event_subscribe`, but on `CONFIG_SUBS`; the FIRST sub for a plugin signals the loader to watch — see Step 3):

```rust
/// Native `__s2_config_on_change(handler)` — opt into live config reload; the loader then watches this
/// plugin's config file and calls the handler with the re-materialized config on change.
fn s2_config_on_change(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        CONFIG_SUBS.with(|m| { m.borrow_mut().subscribe("config", owner.clone(), generation, handler_g); });
        crate::loader::watch_config_for(&owner);   // idempotent; starts stat-polling this plugin's file
    }));
}
```

Register `set_native(scope, global_obj, "__s2_config_on_change", s2_config_on_change);`. Reset `CONFIG_SUBS` in `shutdown` and `remove_by_owner` on unload (mirror `EVENT_MUX_PRE`), and on unload call `crate::loader::unwatch_config_for(id)`.

Add the re-materialize entry the loader calls:

```rust
/// Re-materialize a plugin's config after its override file changed: re-read + merge, re-inject the
/// per-context `__s2pkg_config_values`, and fire the plugin's onChange handlers with the new config.
pub(crate) fn re_materialize_config(id: &str) {
    let decls = PLUGINS.with(|p| p.borrow().get(id).map(|pi| pi.config_decls.clone()));  // stored at load
    let Some(decls) = decls else { return };
    let values_json = materialize_for_load(id, &decls);   // re-read + merge (+ never re-generates: file exists)
    let _ = eval_in_context(id, &format!("globalThis.__s2pkg_config_values = {};", values_json));
    // fire onChange handlers (snapshot; enter the owner context; pass the config object) — mirror dispatch_game_event
    // ... (invoke each CONFIG_SUBS handler for `id` with globalThis.__s2pkg_config_values as the arg) ...
}
```

Note to implementer: store `config_decls` on the `PluginInfo` at load (thread it through `load_plugin_js`/`create_plugin_context`) so `re_materialize_config` can re-run without the manifest. The onChange-fire loop mirrors `dispatch_game_event`'s per-owner context + TryCatch pattern; pass `globalThis.__s2pkg_config_values` (read in-context) as the single arg.

- [ ] **Step 3: The config-mtime watch (`core/src/loader.rs`)**

Add a set of watched plugin ids + their last-seen config mtimes. `watch_config_for(id)` adds the id; `unwatch_config_for(id)` removes it. Extend `poll_plugins` (or a sibling called from the same frame drain): for each watched id, stat its config file (via a new `config_mtime` op OR reuse `config_read` + a content hash if mtime isn't exposed) — on change, call `crate::v8host::re_materialize_config(id)`.

Note to implementer: the shim's `config_read` returns content; the simplest change-detection without a new op is a content hash (store the last hash per watched id; re-materialize when it differs). If you add a `config_mtime(id) -> i64` op instead, ABI-append it after `config_write` (C header + Rust mirror) — either is acceptable; the content-hash reuse of `config_read` avoids a third op. Pick one and note it.

- [ ] **Step 4: The `@s2script/config` types package + a vm test**

Create `packages/config/package.json` (`{ "name": "@s2script/config", "version": "0.1.0", "types": "index.d.ts" }`) and `packages/config/index.d.ts`:

```typescript
/** @s2script/config — typed access to the plugin's materialized config. NO runtime code. */
export type Config = Record<string, string | number | boolean>;
export declare const config: {
  getString(key: string): string;
  getInt(key: string): number;
  getFloat(key: string): number;
  getBool(key: string): boolean;
  /** Opt into live-reload: the handler fires with the re-materialized config when the file changes. */
  onChange(handler: (cfg: Config) => void): void;
};
```

Add a vm test (`packages/cli/test/config-runtime.test.mjs`) that loads the prelude with a stubbed `__s2_config_on_change` + a set `__s2pkg_config_values` and asserts `config.getString/getInt/getBool` read + coerce correctly and an undeclared key yields the zero-value. (Mirror `schema-runtime.test.mjs`'s vm harness; the config module is at `globalThis.__s2pkg_config`.)

- [ ] **Step 5: Tests + boundary + commit**

Run:
```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs && cd -
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
Expected: core green (incl. a `re_materialize`/onChange in-isolate test); CLI green (incl. the config vm test); both gates green.

```bash
git add core/src/v8host.rs core/src/loader.rs packages/config packages/cli/test/config-runtime.test.mjs
git commit -m "$(printf 'feat(slice5e2): @s2script/config runtime + onChange live-reload\n\nThe @s2script/config prelude (typed getters over __s2pkg_config_values; undeclared -> zero-value) +\n__s2_config_on_change native + CONFIG_SUBS mux + re_materialize_config; the loader frame-drain poll\nwatches opted-in plugins config files and re-materializes + fires handlers on change. Types package + vm test.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 5: Demo + sniper build + live gate + docs

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `examples/demo-plugin/package.json`, `README.md`, `CLAUDE.md`

Controller-driven (the sniper build + Docker server).

- [ ] **Step 1: Demo declares + reads config + opts into onChange**

`examples/demo-plugin/package.json` `s2script` block gains:

```json
    "config": {
      "greeting": { "type": "string", "default": "hello from s2script", "description": "Logged on load" },
      "maxUses":  { "type": "int",    "default": 3, "description": "Demo counter" },
      "enabled":  { "type": "bool",   "default": true }
    }
```

`examples/demo-plugin/src/plugin.ts` (pure ESM; passes the 5E.1 typecheck gate):

```typescript
import { config } from "@s2script/config";
export function onLoad(): void {
  console.log("[demo] onLoad — greeting=" + config.getString("greeting")
    + " maxUses=" + config.getInt("maxUses") + " enabled=" + config.getBool("enabled"));
  config.onChange((cfg) => {
    console.log("[demo] config changed — greeting=" + String(cfg.greeting) + " maxUses=" + String(cfg.maxUses));
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```

Build with `npx s2script build .` (must pass the typecheck gate + validate the config block).

- [ ] **Step 2: Controller — one sniper build**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: core + shim build clean (first compile of the Task-3 shim ops). Fix inline + rebuild if the shim fails.

- [ ] **Step 3: Controller — redeploy + live gate (the 5D.2 recipe)**

```bash
mkdir -p dist/addons/s2script/plugins && cp examples/demo-plugin/dist/*.s2sp dist/addons/s2script/plugins/
docker compose -f docker/docker-compose.yml restart cs2      # re-binds mount + keeps gameinfo
# wait past the boot window (poll for "[demo] onLoad — greeting=")
```
Expected: on first load the host AUTO-GENERATES `docker/cs2-data/game/csgo/addons/s2script/configs/_demo_hello.json` with the defaults; the demo logs `greeting=hello from s2script maxUses=3 enabled=true`. Then edit that file (`greeting` → a new value) and observe `[demo] config changed — greeting=<new>` fire WITHOUT a plugin reload. Corrupt/delete the file → defaults on the next reload, server ticking, no crash. Record the exact lines.

- [ ] **Step 4: Docs + live-gate findings**

- README: a "Plugin config (Slice 5E.2)" section (declare/materialize/read + `onChange` opt-in).
- CLAUDE.md `## Current state`: append a 5E.2 paragraph + update `Current focus` (config done; reload-handoff + permissions remain of the lifecycle set).

- [ ] **Step 5: Full sweep + commit**

Run:
```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs && cd -
for g in check-examples-typecheck check-nav-generated check-schema-generated check-events-generated check-core-boundary test-boundary-nameleak; do bash scripts/$g.sh >/dev/null 2>&1 && echo "$g PASS" || echo "$g FAIL"; done
```
Expected: core green; CLI green; all 6 gates PASS.

```bash
git add examples/demo-plugin README.md CLAUDE.md docs/superpowers/specs/
git commit -m "$(printf 'feat(slice5e2): live gate PASSED — config materialize + auto-generate + onChange\n\n<fill with the exact live evidence>\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Self-Review notes (author checklist — completed)

- **Spec coverage:** §1 declare → T1; §2 materialize → T2 (pure) + T3 (at-load + auto-generate); §3 read API → T4; §4 onChange live-reload → T4 (native+mux+watch); §5 boundary/data-flow → T2/T3/T4 (core+shim); §6 degrade → every task's fallback + tests; §7 tests → per-task + T5 gate; §8 tasks → T1–T5.
- **Type consistency:** `materialize_config(decls, override)`/`generate_default_jsonc(decls)` identical across T2 (def) + T3 (`materialize_for_load`). `ConfigDecl { r#type, default, description }` consistent. `config_read`/`config_write` op field order identical in C header (T3 S1) + Rust mirror (T3 S2) + shim wiring (T3 S3). `@s2script/config` methods (`getString/getInt/getFloat/getBool/onChange`) identical in the prelude (T4 S1), the `.d.ts` (T4 S4), and the demo (T5). `__s2pkg_config_values` / `__s2pkg_config` names consistent across T3 (inject), T4 (read), T5.
- **No placeholders:** complete code for T1/T2 + the ops/prelude; T3-S4/T4-S2-S3 carry "match the neighbour" notes pointing at concrete existing code (the `load_plugin_js` inject point, the `EVENT_MUX` mux + `dispatch_game_event` fire loop, the frame-drain poll) — the correct shape for integration threading, plus the one genuine implementer choice (content-hash vs a `config_mtime` op) called out explicitly.

# Slice 4 Spike — CJS eval wrapper + rusty_v8 context embedder slot (findings)

**Task 1 reconnaissance.** Validates the two §13 unknowns the context-per-plugin refactor
(Tasks 4–6) rests on, before it is built. Reconnaissance, not TDD. Both proofs were run as
throwaway Rust `#[test]`s inside `core/src/v8host.rs` against the pinned `v8 = "149.4.0"`
crate, passed green, and were then deleted (only this doc is committed).

- **PROVE #1 (spec §6 — API injection):** the CJS `require`/`module` eval wrapper works on an
  esbuild `--format=cjs --external:@s2script/*` bundle in a bare V8 context.
- **PROVE #2 (spec §3 — plugin identity):** a `v8::Context` carries a per-plugin identity that a
  native `FunctionCallback` reads back correctly *per context* via `get_current_context()`.

Evidence: `cargo test -p s2script-core slice4_spike -- --test-threads=1` →
`3 passed; 0 failed` (`prove1_cjs_wrapper_captures_module_exports`,
`prove2_context_identity_is_per_context`, `prove2b_global_context_create_enter_drop`).

Reproduce the bundle:
```
esbuild entry.ts --bundle --platform=neutral --format=cjs --external:@s2script/std
```
where `entry.ts`:
```ts
import { greet } from "@s2script/std";
export function onLoad() { globalThis.__loaded = greet("world"); }
```

---

## PROVE #1 — CJS eval wrapper

### What esbuild actually emits (the interop shape) — **[HC] proven**

The bundle (`--platform=neutral --format=cjs --external:@s2script/std`), verbatim:

```js
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

// entry.ts
var entry_exports = {};
__export(entry_exports, {
  onLoad: () => onLoad
});
module.exports = __toCommonJS(entry_exports);       // <-- REPLACES module.exports wholesale
var import_std = require("@s2script/std");           // <-- external require call
function onLoad() {
  globalThis.__loaded = (0, import_std.greet)("world");
}
```

Three load-bearing facts for Task 6:

1. **The bundle references exactly `module`, `require`, and (unused) `exports`.** Those are the
   three names the wrapper must inject. It calls `require("@s2script/std")` and reads
   `import_std.greet`, so `require` must return `{ greet }` (a plain object; no interop wrapper
   is applied to a `--platform=neutral` external named import — `import_std.greet(...)` is a
   direct member call, the `(0, …)` is just a `this`-stripping indirect call).

2. **esbuild does `module.exports = __toCommonJS(entry_exports)` — it REASSIGNS `module.exports`.**
   Therefore the host MUST capture the plugin's exports by reading **`module.exports` after
   eval**, never the `exports` parameter it passed in. The `exports` arg the wrapper passes is
   dead for esbuild-cjs output (design §6 passes `{}`; passing `module.exports` also works —
   either way it is ignored, because the plugin overwrites `module.exports`). **[risk-note]** —
   this is subtle; a naive wrapper that reads back the `exports` arg would silently get `{}`.

3. **`module.exports` is decorated: `__esModule: true` plus each named export as a lazy getter**
   (`onLoad` is `get: () => onLoad`). Reading `module.exports.onLoad` triggers the getter and
   yields the function; `module.exports.__esModule === true`. Neither affects capturing/calling
   `onLoad`/`onUnload`; just don't assume the exports object is a plain own-data-property bag.

### The wrapper + how `require`/`module` are provided — **[HC] proven**

Chosen provisioning: **an injected native `require` (`__s2require`, a `v8::Function`) + a
JS-built `module = { exports: {} }`**, and the wrapper **returns `module.exports`** so the Rust
host captures it as the script's return value (stored later as a `v8::Global<v8::Object>` per
plugin, to call `onLoad`/`onUnload`). Exact wrapper string (`{PLUGIN_JS}` = the bundle text):

```js
(() => {
  const module = { exports: {} };
  const require = __s2require;
  (function (require, module, exports) {
{PLUGIN_JS}
})(require, module, module.exports);
  return module.exports;
})()
```

Notes for Task 6:
- `__s2require` is the internal native; the design's `s2require` name maps to it. It is installed
  on the context global before eval. In the spike it returns `{ greet: <native> }` for
  `"@s2script/std"`; in the refactor it returns the per-context `{ OnGameFrame, delay, … }` /
  `{ Pawn }` objects built by the injected prelude. Alternatively the prelude can build the whole
  `require` in JS (a closure over prelude-built objects) — either works; the native form is proven
  here and keeps the API objects host-authored.
- Wrapping in an outer arrow-IIFE that `return`s `module.exports` lets `script.run()` hand the
  exports straight back to Rust. (If you prefer, stash `globalThis.__exports = module.exports`
  and read it back — also fine, but the return-value form avoids a global.)
- The inner `(function (require, module, exports) { … })(require, module, module.exports)` is the
  design §6 wrap verbatim; the `exports` positional is passed but unused (see fact 2).

### Rust proof (copyable) — **[HC] proven**

Fresh isolate + context mirroring `v8host::init`'s construction; native `require`; capture +
call `onLoad`:

```rust
fn spike_greet(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let s = args.get(0).to_rust_string_lossy(scope);
    let out = v8::String::new(scope, &format!("hi {s}")).unwrap();
    rv.set(out.into());
}
fn spike_require(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let name = args.get(0).to_rust_string_lossy(scope);
    if name != "@s2script/std" { rv.set_null(); return; }
    let obj = v8::Object::new(scope);
    let k = v8::String::new(scope, "greet").unwrap();
    let f = v8::Function::new(scope, spike_greet).unwrap();
    obj.set(scope, k.into(), f.into());
    rv.set(obj.into());
}

// ensure_platform(); let mut isolate = v8::Isolate::new(v8::CreateParams::default());
let mut hs_storage = v8::HandleScope::new(&mut isolate);
let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
let hs = &mut hs;
let ctx_local = v8::Context::new(hs, Default::default());
let scope = &mut v8::ContextScope::new(hs, ctx_local);

// Inject the native require on the global.
let global = ctx_local.global(scope);
let k = v8::String::new(scope, "__s2require").unwrap();
let f = v8::Function::new(scope, spike_require).unwrap();
global.set(scope, k.into(), f.into());

// Wrapper returns module.exports; the host captures it.
let wrapper = format!(
    "(() => {{\n  const module = {{ exports: {{}} }};\n  const require = __s2require;\n  (function (require, module, exports) {{\n{}\n}})(require, module, module.exports);\n  return module.exports;\n}})()",
    PLUGIN_JS
);
let code = v8::String::new(scope, &wrapper).unwrap();
let script = v8::Script::compile(scope, code, None).expect("compile");
let ret = script.run(scope).expect("run");
let exports = v8::Local::<v8::Object>::try_from(ret).expect("module.exports is object");

let onload_key = v8::String::new(scope, "onLoad").unwrap();
let onload_val = exports.get(scope, onload_key.into()).unwrap();
assert!(onload_val.is_function());                         // proven
let onload_fn = v8::Local::<v8::Function>::try_from(onload_val).unwrap();
let recv: v8::Local<v8::Value> = v8::undefined(scope).into();
onload_fn.call(scope, recv, &[]).expect("onLoad()");        // sets globalThis.__loaded
// globalThis.__loaded === "hi world"  ✔  (asserted)
```

Result asserted: `module.exports.onLoad` is a function; calling it set `globalThis.__loaded === "hi world"`;
`module.exports.__esModule === true`.

---

## PROVE #2 — context embedder slot + current-context read

### The current-context read from a native — **[HC] proven**

Inside a `FunctionCallback`, `scope.get_current_context()` returns the **calling** context's
`Local<'s, Context>`. Confirmed both empirically (the test) and by reading the crate: for a
`&FunctionCallbackInfo`, rusty_v8 builds the callback scope with `context: Cell::new(None)`
(`scope.rs` `make_new_scope` for `&FunctionCallbackInfo`), so `get_current_context()` falls
through to `v8__Isolate__GetCurrentContext(isolate)` — i.e. the context of the currently running
JS, not a cached/entered one. This is exactly the design §3 requirement ("uses V8's current
context, not a thread-local"), and it stays correct across the microtask checkpoint because each
continuation runs under its own `ContextScope`.

- Exact call: `let ctx: v8::Local<v8::Context> = scope.get_current_context();`
  (method is on `PinnedRef<'s, HandleScope>` = `PinScope`; `scope: &mut v8::PinScope`).
- **[risk-note]** `get_current_context()` caches into the scope's `context` cell on first call and
  returns the cached value thereafter. For a fresh per-callback scope the cache starts empty, so
  this is correct. Do NOT reuse one long-lived scope to read the "current" context across entering
  different contexts and expect it to change — read it fresh inside each native invocation (which
  is the natural pattern anyway).

### Stashing the plugin id — three proven options; **recommend (a) or (c)** — **[HC] proven**

`v8 = "149.4.0"` exposes two distinct mechanisms on `v8::Context` (both proven per-context):

**(a) Rust-typed slot — `Context::set_slot::<T>` / `get_slot::<T>` (RECOMMENDED).**
Stores an `Rc<T>` keyed by `TypeId` in a Rust-side "annex" hung off the context (internally an
aligned pointer in a reserved embedder slot). No `v8::Value`, no scope needed to read, no side
table. Cleanest for a Rust `String` plugin id.
```rust
struct PluginId(String);
let _ = ctx_local.set_slot(std::rc::Rc::new(PluginId("my.plugin".into())));   // stamp (no scope)
// inside a native:
let id: Option<std::rc::Rc<PluginId>> = scope.get_current_context().get_slot::<PluginId>();
```
Caveats: exactly one value **per Rust type** per context (wrap the id in a dedicated newtype);
the `Rc<T>` is dropped when the context is GC'd (or via `remove_slot`/`clear_all_slots`). `set_slot`
returns the previous `Rc<T>` (`Option`) — a reload that re-stamps the same context replaces it.

**(b) `set_embedder_data(0, Integer)` + a Rust side table.** The classic embedder pattern; stores a
`v8::Value` (here a `v8::Integer` index into a `Vec<String>`/`HashMap`). Needs a scope to read.
```rust
let idx = v8::Integer::new(scope, plugin_index);
ctx_local.set_embedder_data(0, idx.into());
// native: let v = scope.get_current_context().get_embedder_data(scope, 0);
//         let idx = v.and_then(|v| v.integer_value(scope)); // -> side-table lookup
```

**(c) `set_embedder_data(0, v8::String)` — the id String stored directly (no side table).**
Because embedder data holds any `v8::Value`, the plugin-id string can live there directly. This is
the literal "plugin id in embedder-data slot index 0" of design §3 with zero indirection.
```rust
let id_str = v8::String::new(scope, "my.plugin").unwrap();
ctx_local.set_embedder_data(0, id_str.into());
// native:
let id: String = match scope.get_current_context().get_embedder_data(scope, 0) {
    Some(v) => v.to_rust_string_lossy(scope),
    None => "<none>".into(),
};
```

Exact signatures (crate 149.4.0):
- `Context::set_slot<T: 'static>(&self, value: Rc<T>) -> Option<Rc<T>>`
- `Context::get_slot<T: 'static>(&self) -> Option<Rc<T>>` (also `remove_slot`, `clear_all_slots`)
- `Context::set_embedder_data(&self, slot: i32, data: Local<'_, Value>)`
- `Context::get_embedder_data<'s>(&self, scope: &PinScope<'s,'_,()>, slot: i32) -> Option<Local<'s, Value>>`
  (there is also `set_aligned_pointer_in_embedder_data`/`get_aligned_pointer_from_embedder_data` for a raw `*mut c_void`)

Note both `set_embedder_data`/`get_embedder_data` internally add `INTERNAL_SLOT_COUNT` to the
slot index, so user slot `0` is safe (V8's own slot-0 debugger caveat does not apply to the
rusty_v8 wrapper's user index).

**Recommendation for Task 4:** use **(a) `set_slot::<PluginId>`** if the id is only needed
Rust-side (subscription/timer routing to the ledger — it is), because it avoids `v8::Value`
juggling and needs no scope at read time. Fall back to **(c)** if you specifically want the id
observable as a JS `v8::Value` in slot 0. Avoid (b) unless you already keep a plugin `Vec`/registry
you want to index by a small integer.

### Rust proof (copyable) — **[HC] proven**

Two contexts in one isolate, each stamped all three ways, one native installed on both; each call
returns the OWNING context's identity:

```rust
struct PluginId(String);
thread_local! { static SIDE: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new()); }

fn spike_whoami(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let ctx = scope.get_current_context();
    let via_slot = ctx.get_slot::<PluginId>().map(|p| p.0.clone()).unwrap_or_else(|| "<none>".into());
    let via_emb = match ctx.get_embedder_data(scope, 0) {           // Integer -> side table
        Some(v) => { let i = v.integer_value(scope).unwrap_or(-1);
            if i >= 0 { SIDE.with(|s| s.borrow().get(i as usize).cloned()).unwrap_or_else(|| "<oob>".into()) }
            else { "<none>".into() } }
        None => "<none>".into(),
    };
    let via_str = match ctx.get_embedder_data(scope, 1) {           // String stored directly
        Some(v) => v.to_rust_string_lossy(scope), None => "<none>".into() };
    let out = v8::String::new(scope, &format!("slot={via_slot};emb={via_emb};str={via_str}")).unwrap();
    rv.set(out.into());
}

// two contexts in one isolate:
let ctx_a = v8::Context::new(hs, Default::default());
let ctx_b = v8::Context::new(hs, Default::default());
let _ = ctx_a.set_slot(std::rc::Rc::new(PluginId("plugin-A".into())));
let _ = ctx_b.set_slot(std::rc::Rc::new(PluginId("plugin-B".into())));
for (ctx, emb_idx, name) in [(ctx_a, 0i32, "plugin-A"), (ctx_b, 1i32, "plugin-B")] {
    let scope = &mut v8::ContextScope::new(hs, ctx);
    let idx = v8::Integer::new(scope, emb_idx);
    ctx.set_embedder_data(0, idx.into());
    let id_str = v8::String::new(scope, name).unwrap();
    ctx.set_embedder_data(1, id_str.into());
    let g = ctx.global(scope);
    let k = v8::String::new(scope, "__whoami").unwrap();
    let f = v8::Function::new(scope, spike_whoami).unwrap();
    g.set(scope, k.into(), f.into());
}
// __whoami() from ctx_a -> "slot=plugin-A;emb=plugin-A;str=plugin-A"
// __whoami() from ctx_b -> "slot=plugin-B;emb=plugin-B;str=plugin-B"   (asserted)
```

### `Global<Context>` create / enter / dispose — **[HC] proven**

- **Create:** `v8::Global::new(scope.as_ref(), ctx_local)` (`scope.as_ref()` → `&Isolate`;
  mirrors `v8host::init`). This is the `PluginInstance.context: v8::Global<v8::Context>` of §3.
- **Enter:** in a fresh `HandleScope`, `let ctx_local = v8::Local::new(hs, &g_ctx);` then
  `let scope = &mut v8::ContextScope::new(hs, ctx_local);` (mirrors `v8host::eval`).
- **Dispose:** there is **no explicit Context dispose** (unlike `Isolate`). A `Global<Context>` is
  a persistent (strong) handle; **dropping it removes the strong reference and the context becomes
  GC-eligible.** Its JS objects are reclaimed by isolate GC (or at isolate teardown). Proven: a
  slot stamped before capturing the `Global` **survives the drop-HandleScope → re-materialise
  round-trip** (read back `"persisted"`), and `drop(g_ctx); drop(isolate);` tears down cleanly with
  no crash.
- **[risk] Teardown ordering (matches §13):** any `v8::Global` pointing *into* a context — the
  plugin's `Global<Function>` handlers, `Global<PromiseResolver>`s in `RESOLVERS`, `Global<Object>`
  exports — must be dropped **before/with** the `Global<Context>` while the isolate is still alive
  (same discipline as today's `shutdown()`, which clears `RESOLVERS`/`CONCOMMANDS` before dropping
  the isolate). The ledger teardown (§4) is what must drop those per-plugin `Global`s in
  reverse-dependency order; dropping only the `Global<Context>` while handler/resolver `Global`s
  linger would keep the context's functions alive (leak) and, if dropped after isolate disposal,
  is unsound. This spike does not exercise the multi-`Global` teardown path — Task 4/the ledger
  owns proving that.

```rust
// create
let g_ctx: v8::Global<v8::Context> = {
    let mut hs_storage = v8::HandleScope::new(&mut isolate);
    let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
    let hs = &mut hs;
    let ctx = v8::Context::new(hs, Default::default());
    let scope = &mut v8::ContextScope::new(hs, ctx);
    let _ = ctx.set_slot(std::rc::Rc::new(PluginId("persisted".into())));
    v8::Global::new(scope.as_ref(), ctx)
};
// enter (fresh HandleScope) + read slot back — survives the round-trip
let seen = {
    let mut hs_storage = v8::HandleScope::new(&mut isolate);
    let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
    let hs = &mut hs;
    let ctx_local = v8::Local::new(hs, &g_ctx);
    let _scope = &mut v8::ContextScope::new(hs, ctx_local);
    ctx_local.get_slot::<PluginId>().map(|p| p.0.clone()).unwrap_or_default()
}; // == "persisted"
// dispose: drop the Global (context becomes GC-eligible), then the isolate
drop(g_ctx);
drop(isolate);
```

---

## Summary of tags

| Item | Status |
|------|--------|
| esbuild cjs bundle references `require`/`module`/`exports`; external require returns a plain `{ greet }` | **[HC] proven** |
| Wrapper `(function(require,module,exports){…})(require,module,module.exports)`, host captures via `return module.exports` | **[HC] proven** |
| `require` = injected native `__s2require`; `module = { exports: {} }` built in JS | **[HC] proven** |
| esbuild REASSIGNS `module.exports`; must read `module.exports`, not the `exports` arg | **[HC] proven** + **[risk-note]** (subtle) |
| `module.exports` carries `__esModule:true` + lazy getters; `onLoad` still callable | **[HC] proven** |
| `scope.get_current_context()` in a native returns the CALLING context (per-context) | **[HC] proven** (empirical + crate source: callback scope `context = None`) |
| `Context::set_slot::<T>`/`get_slot::<T>` (recommended for the id) | **[HC] proven** |
| `Context::set_embedder_data(0, Integer)` + side table | **[HC] proven** |
| `Context::set_embedder_data(0, v8::String)` (id string direct, no side table) | **[HC] proven** |
| `Global<Context>` create (`Global::new`) / enter (`Local::new` + `ContextScope`) / drop | **[HC] proven** |
| No explicit Context dispose; drop of `Global<Context>` → GC-eligible; slot survives round-trip | **[HC] proven** |
| Multi-`Global` teardown ordering (handlers/resolvers/exports dropped before context, isolate alive) | **[risk]** — ledger/Task 4 must prove; today's `shutdown()` shows the discipline |
| `get_current_context()` caches per-scope — read fresh inside each native (natural pattern) | **[risk-note]** |

## Notes for the refactor (Tasks 4–6)

- Store per plugin: `PluginInstance { id, context: Global<Context>, ledger, generation }` (§3).
  Stamp the id via `set_slot::<PluginId>` at context creation; also store the captured
  `Global<Object>` for `module.exports` to call `onLoad`/`onUnload`.
- Install `__s2require` (native) on each new context's global before eval; it returns that
  context's `@s2script/std` / `@s2script/cs2` API objects (built by the injected prelude over the
  existing `__s2_*` natives, whose internal names are unchanged — §6/§7).
- Every native that currently uses thread-local state to find "the" context (there is only one
  today) must switch to `scope.get_current_context()` → the id slot → that plugin's ledger.
- The crate is `crate-type = ["cdylib"]`, so unit tests must be inline `#[cfg(test)] mod` blocks
  in `core/src/*` (an integration test under `core/tests/` cannot link — no `rlib` artifact). The
  spike used `#[cfg(test)] mod slice4_spike` in `v8host.rs` and called the private
  `ensure_platform()` (V8 platform init-once) directly.

// Lint config for core/js/ — the engine-generic prelude that `core/src/v8host.rs` bakes in with
// `include_str!`.
//
// The prelude runs in a bare V8 context, NOT in Node and NOT as a module: there is no `require`, no
// `import`, no `process`, and the only globals are the ones core installs on the context. So the
// globals list here is DERIVED from core's own source rather than hand-maintained — rename a native
// in Rust without updating the JS and `no-undef` fails, which is the whole point of this gate. A
// checked-in literal list would drift silently and turn the gate into decoration.
import { readFileSync, readdirSync } from "node:fs";

// Scans ALL of core/src, not just v8host.rs. Natives used to live in exactly one file; the
// core-stabilization program is moving them into per-feature modules (`usermsg.rs` first), and a
// config pinned to v8host.rs would silently lose a native's global the moment its feature moved out
// — turning this gate from "catches a rename" into "fails on a refactor". Directory-scoped is the
// property that actually holds: wherever a native is registered in core, the prelude may name it.
const rustSources = (dir) =>
  readdirSync(dir, { withFileTypes: true, recursive: true })
    .filter((e) => e.isFile() && e.name.endsWith(".rs"))
    .map((e) => readFileSync(new URL(`${e.parentPath ?? e.path}/${e.name}`, import.meta.url), "utf8"))
    .join("\n");

const coreSrc = rustSources(new URL("../src/", import.meta.url));
const prelude = readFileSync(new URL("./prelude.js", import.meta.url), "utf8");

const readonlyGlobals = (names) =>
  Object.fromEntries([...new Set(names)].sort().map((n) => [n, "readonly"]));

// 1. Every native registered onto the context's global object.
const natives = [...coreSrc.matchAll(/set_native\(scope, global_obj, "([^"]+)"/g)].map((m) => m[1]);

// 2. Globals core installs by a path other than `set_native` (each needs a citation, so that a
//    stale entry is findable). `console` is built in Rust and set directly on the global object.
const extraFromRust = ["console"]; // v8host.rs — `let console_key = v8::String::new(scope, "console")`

// 3. The module objects this file itself publishes on `globalThis` and then reads back as bare
//    globals later in the same file. Legitimate at runtime (a `globalThis` property IS a global);
//    invisible to ESLint's scope analysis, which only sees the assignment.
const selfPublished = [...prelude.matchAll(/globalThis\.(__s2pkg_\w+|HookResult|Priority|Phase)\s*=/g)]
  .map((m) => m[1]);

if (natives.length === 0) {
  // A regex that silently matches nothing would turn `no-undef` into a wall of false failures and
  // tempt whoever hits it to disable the rule. Fail loudly instead.
  throw new Error("core/js/eslint.config.mjs: found no set_native globals in core/src — the regex is stale");
}

export default [
  {
    // colors.js is dual-use — it ships concatenated into the prelude AND is unit-tested under node
    // (`scripts/check-colors-test.sh`), so its `typeof module !== "undefined"` guard and its test
    // file legitimately reference CommonJS globals. Scoped narrowly rather than adding them to the
    // prelude's own globals, where they would mask a real `require` creeping into raw-context code.
    // Mirrors games/cs2/js/eslint.config.mjs's activity.js block.
    files: ["colors.js", "colors.test.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "script",
      globals: {
        module: "readonly", require: "readonly", exports: "readonly",
        globalThis: "readonly", console: "readonly",
      },
    },
    rules: { "no-undef": "error", "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }] },
  },
  {
    files: ["**/*.js"],
    ignores: ["colors.js", "colors.test.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "script",
      globals: readonlyGlobals([...natives, ...extraFromRust, ...selfPublished]),
    },
    linterOptions: { reportUnusedDisableDirectives: true },
    rules: {
      "no-undef": "error",
      // Unused catch bindings and callback params are idiomatic here (`catch (e) {}` swallowing a
      // degrade, handlers that ignore trailing args). Unused LOCALS still fail — that is dead code.
      "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }],
    },
  },
];

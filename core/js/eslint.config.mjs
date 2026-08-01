// Lint config for core/js/ — the engine-generic prelude that `core/src/v8host.rs` bakes in with
// `include_str!`.
//
// The prelude runs in a bare V8 context, NOT in Node and NOT as a module: there is no `require`, no
// `import`, no `process`, and the only globals are the ones core installs on the context. So the
// globals list here is DERIVED from core's own source rather than hand-maintained — rename a native
// in Rust without updating the JS and `no-undef` fails, which is the whole point of this gate. A
// checked-in literal list would drift silently and turn the gate into decoration.
import { readFileSync } from "node:fs";

const v8host = readFileSync(new URL("../src/v8host.rs", import.meta.url), "utf8");
const prelude = readFileSync(new URL("./prelude.js", import.meta.url), "utf8");

const readonlyGlobals = (names) =>
  Object.fromEntries([...new Set(names)].sort().map((n) => [n, "readonly"]));

// 1. Every native registered onto the context's global object.
const natives = [...v8host.matchAll(/set_native\(scope, global_obj, "([^"]+)"/g)].map((m) => m[1]);

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
  throw new Error("core/js/eslint.config.mjs: found no set_native globals in v8host.rs — the regex is stale");
}

export default [
  {
    files: ["**/*.js"],
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

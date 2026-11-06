# Native extensions

A native extension is a shared library that adds natives to s2script without a core change, the
way SourceMod extensions do. Use one when a plugin needs native code: a fast regex or geo-IP
lookup, a custom database driver, a math kernel, or a library that only has a C API.

The C contract is [`packages/sdk/ext/s2ext.h`](../packages/sdk/ext/s2ext.h) (ABI 1), shipped in
the `@s2script/sdk` npm package under `ext/`. The worked example is
[`examples/extension-fastmath`](../examples/extension-fastmath): the extension in `native/`, and a
plugin that imports it.

## For operators

- Put the extension's `.so` in `addons/s2script/extensions/` and restart the server. Extensions
  load once at startup and are never unloaded or reloaded.
- `sm ext list` shows every extension: loaded with its natives and the plugins using it, or
  refused with the reason.
- A broken extension (wrong ABI, a bad descriptor, a failed init) is refused on its own. The
  server and every other extension keep running.
- A plugin that needs an extension the server does not have, or has at the wrong major version,
  is refused at load with the reason (`sm plugins list`).
- **Trust:** native code runs with full access to the server process, the same as a Metamod
  plugin. Install only extensions you trust.

## For plugin authors

Declare the extension in `package.json` and import it like any module:

```json
"s2script": { "extensions": { "@community/fastmath": "^0.1.0" } }
```

```ts
import { clamp, parseNumber } from "@community/fastmath";
```

- The types are real, never `any`. The build uses `.s2script/extensions/<name>/index.d.ts` if it
  exists, otherwise the extension's own npm package. `sm ext types <name>` prints that file on a
  server that has the extension.
- An extension is a hard dependency: the plugin does not load without it. A version range
  matches on the major version, as interface dependencies do.
- A native that reports a failure throws an `S2Error` whose `code` is the extension's code. An
  entity argument that is no longer live throws `S2Error` code `"stale_entity"`.

## For extension authors

```c
#include "s2ext.h"

static int32_t clamp(void* recv, int32_t v, int32_t lo, int32_t hi) {
  (void)recv;
  return v < lo ? lo : v > hi ? hi : v;
}

static const S2ExtInfo INFO = {sizeof(S2ExtInfo), S2EXT_ABI_VERSION, "@community/fastmath", "0.1.0"};
S2EXT_EXPORT const S2ExtInfo* s2ext_info(void) { return &INFO; }

S2EXT_EXPORT int32_t s2ext_init(const S2HostApi* host, S2Registrar* reg, const S2RegistrarApi* api) {
  static const S2Type args[] = {S2T_I32, S2T_I32, S2T_I32};
  S2NativeDesc d = {sizeof(S2NativeDesc), "clamp", (void*)clamp, S2T_I32, args, 3, 0, "Clamp an integer.", 0};
  return api->native(reg, &d);
}
```

Build with `cc -shared -fPIC -O2 -I node_modules/@s2script/sdk/ext -o fastmath.so fastmath.c`.
For a server, build inside the same Steam Runtime 3 (Debian bullseye) container the runtime uses,
so the library doesn't need a newer glibc than the server has.

### Types and paths

| `S2Type` | C | JavaScript |
|---|---|---|
| `S2T_VOID` | `void` | `undefined` (result only) |
| `S2T_BOOL` | `bool` | `boolean` |
| `S2T_I32` / `S2T_U32` | `int32_t` / `uint32_t` | `number` (`ToInt32` / `ToUint32`) |
| `S2T_F32` / `S2T_F64` | `float` / `double` | `number` |
| `S2T_STRING` | `const char*` (UTF-8) | `string`; a `NULL` result is `null` |
| `S2T_ENTITY` | `void*` (the live entity, during the call only) | `EntityRef` (argument only) |

A native whose types are all numbers and booleans, and which doesn't set `S2N_CAN_FAIL`, is a
**fast native**. V8's fast API points optimised JavaScript straight at your C function. Every
other native goes through the host's generic path, which converts the arguments, resolves
entities through the host's books, and places each argument in its register.

Measured in this repository (release build, `extension_bench_call_cost`):

| Call | Cost |
|---|---|
| Fast native (`clamp`) | 8 ns |
| Plain JS function doing the same work | 1.3 ns |
| Generic path, numbers only | about 120 ns |

### Rules

- The ABI is two exported functions and append-only structs that carry their own size. A real
  break bumps `S2EXT_ABI_VERSION`, and the host refuses extensions built for a different ABI.
- Natives run on the game thread. A fast native must not block, must not keep its receiver, and
  may call only the `log` and `schema_offset` host services.
- Engine access goes through host services, never through engine tables directly. That keeps the
  core the owner of every engine touchpoint. ABI 1 offers `log`, `schema_offset` and `fail`;
  services are added by appending to `S2HostApi`.
- An entity pointer and a string argument are valid only during the call. Never store them.
- ABI 1 supports x86-64 Linux only (System V calling convention): at most 5 integer-class
  arguments (bool, integers, strings, entities) and at most 8 float arguments per native.

## Not in ABI 1

- Optional extension dependencies (an import that can be `null`).
- Extension-owned events, hooks or per-plugin resources (handles that the ledger tears down).
- Windows.
- A standalone tool that generates typings without a server. For now `sm ext types` does it,
  and a test keeps the example's typings in step with what the host generates.

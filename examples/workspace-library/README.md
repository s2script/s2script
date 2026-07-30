# workspace-library

The workspace-sibling half of the library-packages feature — `examples/library-package` +
`examples/library-consumer` show the other half, the vendored `.s2script/libs/` copy `s2s add`
produces from a *published* library. This example shows the monorepo case: a library declared
right here, in the same workspace, resolved straight from its own source with no copy anywhere.

```
package.json                      the workspace root: workspaces + s2script.workspace.plugins
tsconfig.base.json                shared editor/tsc config every member extends
libs/greeter/                     s2script.kind: "library" — @ws-example/greeter
plugins/greeter-user/             s2script.libraries: { "@ws-example/greeter": "^0.1.0" }
```

## Build it

```bash
npx @s2script/sdk build examples/workspace-library
```

This builds **both** members: `libs/greeter/dist/_ws-example_greeter.s2lib` (the library, built so
the workspace's own gate proves it's buildable — nothing here ever runs `s2s deploy` on it) and
`plugins/greeter-user/dist/_ws-example_greeter-user.s2sp` (the plugin, with the library's code
bundled straight in). `--filter @ws-example/greeter-user` builds just the plugin;
`--filter @ws-example/greeter` builds just the library.

## The declaration

`libs/greeter/package.json`:

```json
{ "name": "@ws-example/greeter", "main": "src/index.ts", "types": "src/index.d.ts", "s2script": { "kind": "library" } }
```

`plugins/greeter-user/package.json`:

```json
{ "s2script": { "libraries": { "@ws-example/greeter": "^0.1.0" } } }
```

and `plugins/greeter-user/src/plugin.ts` imports it like any other module:
`import { greet } from "@ws-example/greeter";`. That's the whole contract — no `.npmrc`, no
`s2s add`, no `.s2script/libs/` copy anywhere in `plugins/greeter-user`.

## Why no vendored copy here

`s2script.libraries` resolves from two sources, tried in order (`packages/sdk/src/libraries.ts`):
a vendored copy at `.s2script/libs/<name>/`, or — only inside an npm workspace — a sibling package
opting in with `s2script.kind: "library"`, resolved straight from its own `main`/`types` on disk.
`libs/greeter` sits in *this* workspace, so the second path applies: both the typecheck pass and
the esbuild bundle step resolve `@ws-example/greeter` directly to `libs/greeter/src/index.ts` and
`src/index.d.ts` — the exact files above, not a copy of them. Edit `libs/greeter/src/index.ts` and
rebuild; the change is picked up immediately, the same way editing `plugins/greeter-user/src/plugin.ts`
itself would be. Re-vendoring a sibling's copy after every edit would be the untenable part —
vendoring exists for the case where the library isn't sitting right here on disk (see
`examples/library-consumer`'s README for that case, and for why it's needed there and not here).

## Proving the library is really inlined

```bash
npx @s2script/sdk build examples/workspace-library
node -e "
const {unzipSync}=require('fflate');
const fs=require('fs');
const z=unzipSync(fs.readFileSync('examples/workspace-library/plugins/greeter-user/dist/_ws-example_greeter-user.s2sp'));
const js=Buffer.from(z['plugin.js']).toString();
if (js.includes('require(\"@ws-example/greeter\")')) throw new Error('left EXTERNAL');
if (!/hello, /.test(js)) throw new Error('library code missing from the bundle');
console.log('bundled OK');
"
```

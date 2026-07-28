# monorepo

Several plugins, one repo, one build — and a sibling plugin dependency with
**no hand-copied types**.

```
package.json                 the workspace root: workspaces + s2script.workspace.plugins
tsconfig.base.json            shared editor/tsc config every member extends
plugins/producer/             publishes @monorepo-example/producer@1.0.0
plugins/consumer/             depends on it — a WORKSPACE SIBLING, not a registry package
packages/shared/              a library bundled into BOTH plugins
```

## The root marker

```json
{
  "workspaces": ["plugins/*", "packages/*"],
  "s2script": { "workspace": { "plugins": ["plugins/*"] } }
}
```

`workspaces` is npm's own field — it is what makes npm (and `s2s build`) symlink every member
into `node_modules`, whether or not anything declares a dependency on it. `s2script.workspace.plugins`
says which of those members are plugins; `packages/shared` is everything else, a shared library.
The *presence* of `s2script.workspace` is the only thing that makes this directory a workspace
root — nothing here marks the plugin `package.json` files themselves.

## Build everything

```bash
npx @s2script/sdk build examples/monorepo
```

One command builds **both** plugins — `plugins/producer/dist/*.s2sp` and
`plugins/consumer/dist/*.s2sp` — in dependency order. `--filter <pattern>` narrows to one
(`--filter @monorepo-example/consumer` or `--filter plugins/producer`); naming one plugin's own
directory (`s2s build examples/monorepo/plugins/producer`) builds only that plugin, exactly as it
would outside a workspace.

## The sibling contract: no copy, anywhere

`plugins/producer/package.json` publishes an interface:

```json
{ "name": "@monorepo-example/producer", "types": "api.d.ts", "s2script": { "publishes": "self" } }
```

`plugins/consumer/package.json` depends on it like any other interface:

```json
{ "s2script": { "pluginDependencies": { "@monorepo-example/producer": "^1.0.0" } } }
```

and `plugins/consumer/src/plugin.ts` writes `import type { Greeter } from
"@monorepo-example/producer"` — a plain import of the producer's own package name.
**There is no `.s2script/types/@monorepo-example/producer/` directory in `plugins/consumer`.**
That absence is the entire point of this example.

It works because npm's workspace machinery already put the producer on disk right next to the
consumer: `node_modules/@monorepo-example/producer` is a symlink to `../../plugins/producer`, and
`moduleResolution: "bundler"` resolves the producer's own `types` field through it with no
config and no generated file. `s2s build` typechecks the consumer against that *exact* file, and
hashes those *exact* bytes into `manifest.compiledAgainst` — the same bytes the producer itself
hashed for its own `manifest.publishes`. Compare:

```bash
npx @s2script/sdk build examples/monorepo
unzip -p examples/monorepo/plugins/producer/dist/*.s2sp manifest.json | grep -A1 typesSha256
unzip -p examples/monorepo/plugins/consumer/dist/*.s2sp manifest.json | grep -A1 compiledAgainst
sha256sum examples/monorepo/plugins/producer/api.d.ts
```

All three name the same hash. Drift between the two plugins isn't merely checked — it's
impossible, because there was only ever one copy of the contract on disk.

### Contrast: examples/greeter-consumer keeps its copy, and should

`examples/greeter-consumer` depends on `examples/greeter-plugin`'s `@demo/greeter` the same way —
`s2script.pluginDependencies` and a plain `import type` — but it *does* keep a hand-maintained
byte-copy at `.s2script/types/@demo/greeter/index.d.ts`. That isn't an oversight; it's the other
half of this lesson. `greeter-plugin` is not a sibling of `greeter-consumer` in any workspace —
each is its own standalone package, and in the general case a plugin's producer is a *registry*
dependency, published and versioned independently, with no copy of its source sitting on your
disk to resolve through. The verified copy (and the hash the build takes of it) is what makes that
case safe. Sibling resolution, the mechanism this example demonstrates, is available only when the
producer really is right there in the same workspace — which is exactly the situation
`plugins/producer` and `plugins/consumer` are in and `greeter-plugin`/`greeter-consumer` are not.

## packages/shared: still a build-time factoring, now shared by two plugins

`examples/monorepo-plugin` (this example's predecessor) taught one lesson: *workspace packages are
a build-time factoring of **one** plugin.* `packages/shared` here is the same mechanism, aimed at a
new case — a library **two separate plugins** both want, without either depending on the other for
it:

- Sibling packages should declare `exports`, not `main`:

  ```json
  { "name": "@monorepo-example/shared", "exports": { ".": "./src/index.ts" } }
  ```

- **Both plugins bundle their own copy.** `plugins/producer` and `plugins/consumer` each `import`
  `@monorepo-example/shared` directly, and esbuild inlines it into each `.s2sp` independently — it
  is not a runtime dependency of either, and does not appear in either manifest. `Tally`, the
  counter `@monorepo-example/shared` exports, makes this concrete: the producer's `tally` and the
  consumer's `asked` are two different objects in two different `.s2sp` files, even though they
  come from the exact same source line. If two plugins need to observe the *same* live count, that
  is what the published interface above is for — `greetCount()` crosses the plugin boundary as a
  method call; `Tally` never crosses it at all.

`node_modules/@monorepo-example/*` are committed as symlinks (git mode `120000`) into
`plugins/producer`, `plugins/consumer`, and `packages/shared` — standing in for the `npm install`
a nested workspace like this one never receives from the repo root. A checkout on Windows needs
`core.symlinks` enabled (and Developer Mode or an elevated `git clone`), or git materializes them
as plain-text files containing the link target instead of the packages themselves, and the
typecheck gate fails confusingly (module not found, pointing at what looks like a valid directory).

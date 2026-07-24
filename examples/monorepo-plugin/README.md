# monorepo-plugin

One plugin, split across npm workspace packages.

```
package.json          workspaces: ["packages/*"]
src/plugin.ts         the entry point — composes the feature packages
packages/core/        shared types and state
packages/commands/    one slice of behaviour, importing core
```

## Two rules

1. **Sibling packages should declare `exports`**, not `main`:

   ```json
   { "name": "@monorepo-example/core", "exports": { ".": "./src/index.ts" } }
   ```

   `main` also works, but `exports` is the modern field and is what the
   bundler resolves without any configuration.

2. **The whole tree bundles into one `.s2sp`.** Sibling packages are inlined at
   build time. They are not runtime dependencies and do not appear in the
   manifest.

`node_modules/@monorepo-example/*` are committed as symlinks (git mode
`120000`) into `packages/*`, standing in for the workspace `npm install`
would otherwise create. A checkout on Windows needs `core.symlinks` enabled
(and Developer Mode or an elevated `git clone`), or git materializes them as
plain-text files containing the link target instead of the packages
themselves, and the typecheck gate fails confusingly (module not found,
pointing at what looks like a valid directory).

## Not the same as a cross-plugin interface

Workspace packages are a build-time factoring of **one** plugin. If two parts
need to load, unload, and version independently, they are two plugins talking
over a published interface — see `examples/greeter-plugin`.

## Build

```bash
npx @s2script/sdk build examples/monorepo-plugin
```

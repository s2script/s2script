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

## Not the same as a cross-plugin interface

Workspace packages are a build-time factoring of **one** plugin. If two parts
need to load, unload, and version independently, they are two plugins talking
over a published interface — see `examples/greeter-plugin`.

## Build

```bash
npx s2s build examples/monorepo-plugin
```

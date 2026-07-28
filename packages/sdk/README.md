# @s2script/sdk

The **s2script SDK** — the TypeScript types and the `s2s` CLI for building
[Source 2](https://developer.valvesoftware.com/wiki/Source_2) / Counter-Strike 2 server plugins.

s2script is a plugin framework for Source 2 games, loaded via
[Metamod:Source](https://www.sourcemm.net/). This package is what you develop against; the runtime
ships with the server addon.

## Quickstart

```bash
npx @s2script/sdk create my-plugin
cd my-plugin
npm run build          # → dist/<id>.s2sp
```

Copy the `.s2sp` into `addons/s2script/plugins/` on a running server and it loads immediately —
re-drop to hot-reload, delete to unload. No restart.

```ts
import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.commands.register("hello", (cmd) => {
    cmd.reply("hello from s2script");
  });
});
```

This package is **types-only** — the engine injects the implementation at load time, and
`s2s build` marks `@s2script/*` external rather than bundling it. Capabilities are imported as
per-capability subpaths (`@s2script/sdk/entity`, `@s2script/sdk/timers`, `@s2script/sdk/clients`, …);
game-specific schema types ship separately in
[`@s2script/cs2`](https://www.npmjs.com/package/@s2script/cs2).

## Workspaces

A directory whose root `package.json` carries an `s2script.workspace.plugins` glob list — next to
npm's own `workspaces` field — is a **workspace**: a repo holding several plugins that build,
publish, and version together.

```json
{
  "name": "my-server-plugins",
  "private": true,
  "workspaces": ["plugins/*", "packages/*"],
  "s2script": { "workspace": { "plugins": ["plugins/*"] } }
}
```

```bash
npx @s2script/sdk create --workspace my-server-plugins
cd my-server-plugins
npx @s2script/sdk create shop      # detects the workspace, writes plugins/shop (no per-plugin devDeps)
npm install                        # links every member into node_modules
npx @s2script/sdk build            # every plugin, dependency order; --filter <pattern> to narrow
npx @s2script/sdk deploy           # build + publish, skipping `private` and already-shipped versions
npx @s2script/sdk version          # applies pending changesets, cascading sibling version ranges
```

Finding no `s2script.workspace` marker above the target directory, every command behaves exactly as
it does today — workspace mode is opt-in by construction. `s2s build <one-plugin-dir>` (naming a
specific plugin rather than the root) always stays single-plugin mode even inside a workspace;
`--filter` is the workspace-mode way to narrow to a subset, by package name (`@me/shop`, `@me/*`) or
by path glob (`plugins/sh*`). `--stamp-version <v>` (build-only) rewrites every plugin's version
*and* the sibling ranges that would otherwise break.

**Sibling interfaces need no `.s2script/types/` copy.** npm already symlinks every workspace member
into `node_modules` — declared as a dependency or not — so a consumer's `import type { ShopAPI }
from "@me/shop"` resolves straight to `@me/shop`'s own published `types` file under
`moduleResolution: "bundler"`, no config and no copy required. Contrast this with depending on a
**registry** package (`s2s add @edge/admin-core`): that pulls the producer's `.d.ts` down into your
own `.s2script/types/<dep>/index.d.ts`, because a registry producer isn't sitting in the same
checkout — a byte-copy (hashed into `manifest.compiledAgainst`, re-verified against the producer's
published hash at load) is the only way your build can see its shape at all, and it goes stale the
moment the producer's interface changes until you re-run `s2s add`. A workspace sibling's contract
can never go stale that way: the file your build resolves to *is* the producer's current contract,
on the same disk, in the same commit. If a plugin still carries a pre-migration
`.s2script/types/<dep>/` copy after joining a workspace, the live sibling wins and the build warns
that the copy is now dead weight.

**`private: true` means built, never published.** Ordinary npm/Changesets semantics, not a new
field: `s2s build` builds every plugin in a workspace regardless of `private`, but `s2s deploy`
prints a per-plugin plan that skips a `private: true` package by name instead of shipping it. That's
what lets a workspace hold a large first-party or in-house plugin suite that must never reach the
public registry — build it, drop the `.s2sp`s into your own server, never `s2s deploy` it. (`s2s
deploy <dir>` on a single private plugin outside a workspace refuses the same way.)

## Docs

**[s2script.com/docs](https://s2script.com/docs)** — getting started, guides, and the full API
reference. Source and issues: [GitHub](https://github.com/s2script/s2script).

## License

MIT OR Apache-2.0

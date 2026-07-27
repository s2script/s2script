---
"@s2script/sdk": minor
---

SDK workspaces: a repository of many plugins that builds, publishes, and versions together

A directory whose `package.json` carries an `s2script.workspace.plugins` glob list is now a
first-class thing the CLI understands. Discovery walks UP from the target directory looking for
that marker, so a tree without it — or a `s2s build <one-plugin-dir>` inside one — behaves exactly
as it does today. Workspace mode is opt-in by construction.

- `s2s build` at a workspace root builds every plugin in dependency order, preflighting the whole
  workspace first (sibling `pluginDependencies` ranges must match the versions actually being
  shipped, and every violation is reported at once) and then collecting per-plugin failures rather
  than stopping at the first, so one broken plugin cannot hide seventeen others. `--filter` narrows
  by package name or path glob; `--stamp-version` rewrites every plugin's version AND the sibling
  ranges that stamp would otherwise break.
- `s2s deploy` at a workspace root builds the filtered set, requires it all green (a partial
  publish is unrecoverable state), prints a per-plugin plan — `skip (private)` /
  `skip (already published)` / `PUBLISH` — and uploads in the same dependency order, so a consumer
  is never live against an absent producer. A duplicate-version rejection mid-fan-out is a skip,
  never a failure, which makes re-running after a partial failure safe.
- `s2s deploy <dir>` now refuses a `private: true` package by name in single-plugin mode too,
  closing a hole where a private plugin could be published to the registry.
- `s2s version` applies pending changesets across a workspace, cascading bumps through
  `s2script.pluginDependencies` — which changesets cannot see — by handing it an in-memory mirror
  of the packages with those edges injected, then applying the resulting plan against the real
  ones. Nothing mirrored is ever written to disk.
- `s2s create --workspace <dir>` scaffolds a workspace root; `s2s create <name>` inside one writes
  a minimal `plugins/<name>/` that inherits the root's devDependencies, eslint config, and tsconfig.

A plugin can now depend on a workspace sibling's published interface with **no hand-copied
`.s2script/types/<dep>/index.d.ts`**: npm already symlinks every workspace member, so the typecheck
gate simply stops writing its ambient `any` stub for a sibling and lets node resolution find the
producer's own `types` file. `manifest.compiledAgainst` then hashes those same bytes the producer
publishes, so the loader's drift check passes for structural reasons rather than by luck. A stale
local copy loses to the sibling and the build says so. A dependency that is not a workspace sibling
keeps today's exact behaviour.

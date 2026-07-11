# Changesets

This repo uses [Changesets](https://github.com/changesets/changesets) to version and publish the `@s2script/*` packages under `packages/` to npm.

**Two release trains (do not couple them):**

| Train | Trigger | Ships |
|-------|---------|--------|
| **npm** | PR adds a changeset → merge to `main` → version PR → merge → `changeset publish` | `@s2script/*` types + CLI |
| **runtime zip** | `git tag v*` + push | sniper binaries + base `.s2sp` plugins (GitHub Release) |

Base plugins under `plugins/` are **not** Changesets packages. They declare `s2script.apiVersion` and ship in the zip.

## Maintainer flow (npm)

1. On a PR that changes anything under `packages/`, run:

   ```bash
   npm run changeset
   ```

   Pick the bump (major/minor/patch). All `@s2script/*` packages share one fixed version, so one changeset covers the set.

2. Merge the PR. CI opens a **Version Packages** PR that bumps versions + changelogs.

3. Merge the version PR. CI publishes to npm via **trusted publishing (OIDC)** — no `NPM_TOKEN`. One-time setup: `scripts/bootstrap-npm-trusted-publishing.sh --apply` (uses `npm trust` in a loop — not 29 clicks; see `docs/INSTALL.md`).

Plugin-only or runtime-only work needs **no** changeset — tag a `v*` release instead (see `docs/INSTALL.md`).

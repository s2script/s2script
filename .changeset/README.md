# Changesets

This repo uses [Changesets](https://github.com/changesets/changesets) to version and publish the `@s2script/*` packages under `packages/` to npm.

**Packages version independently.** A changeset only bumps the packages you select — `@s2script/admin` can be `0.2.0` while `@s2script/math` stays `0.1.0`.

**Two release trains (do not couple them):**

| Train | Trigger | Ships |
|-------|---------|--------|
| **npm** | PR adds a changeset → merge to `main` → version PR → merge → `changeset publish` | `@s2script/*` types + CLI (independent semver) |
| **runtime zip** | `git tag v*` + push | sniper binaries + base `.s2sp` plugins (GitHub Release) |

Base plugins under `plugins/` are **not** Changesets packages. They declare `s2script.apiVersion`, ship in the zip, and are **stamped to the release tag version** at build time (`VERSION=… scripts/build-base-plugins.sh`).

## Maintainer flow (npm)

1. On a PR that changes anything under `packages/`, run:

   ```bash
   npm run changeset
   ```

   Select **only the packages that changed** and the bump (major/minor/patch). Dependent packages get a patch bump automatically when needed (`updateInternalDependencies`).

2. Merge the PR. CI opens a **Version Packages** PR that bumps those packages + changelogs.

3. Merge the version PR. CI publishes to npm via **trusted publishing (OIDC)** — no `NPM_TOKEN`. One-time setup: `scripts/bootstrap-npm-trusted-publishing.sh --apply` (uses `npm trust` in a loop — not 29 clicks; see `docs/INSTALL.md`).

Plugin-only or runtime-only work needs **no** changeset — tag a `v*` release instead (see `docs/INSTALL.md`).

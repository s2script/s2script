# Installing s2script (CS2, Linux)

s2script ships as a **SourceMod-style zip**: extract over your server's `game/csgo/` directory so `addons/` overlays in place. The zip includes the runtime **and** the first-party base plugins (already under `addons/s2script/plugins/`).

Linux x86-64 only. Windows is not supported yet.

## Prerequisites

1. A Counter-Strike 2 dedicated server.
2. **[Metamod:Source 2.0](https://www.sourcemm.net/)** installed under `game/csgo/addons/metamod/` (same as any other Metamod plugin).

## Install the runtime

1. Download the latest `s2script-cs2-linux-*.zip` from [GitHub Releases](https://github.com/GabeHirakawa/s2script/releases).
2. Extract it into `game/csgo/`:

   ```bash
   cd /path/to/cs2/game/csgo
   unzip /path/to/s2script-cs2-linux-0.1.1.zip
   ```

   That creates:

   ```
   addons/metamod/s2script.vdf
   addons/s2script/
     VERSION
     bin/linuxsteamrt64/s2script.so
     bin/linuxsteamrt64/libs2script_core.so
     gamedata/core.gamedata.jsonc
     js/pawn.js
     plugins/          # base .s2sp plugins + drop more here
     configs/          # auto-generated on first load
     data/             # SQLite DBs
   ```

3. Patch `game/csgo/gameinfo.gi` so Metamod is on the SearchPath (once per game install; CS2 updates can wipe this). Insert as the **first** SearchPath entry:

   ```
   Game    csgo/addons/metamod
   ```

   immediately before the bare `Game    csgo` line. The repo's [`docker/patch-gameinfo.sh`](../docker/patch-gameinfo.sh) does this idempotently if you prefer a script.

4. Ensure `addons/s2script/configs` and `addons/s2script/data` are **writable** by the user that runs the server (the host auto-creates config JSON and SQLite files there).

5. Restart the server. Confirm load:

   ```
   meta list
   ```

   You should see `s2script` loaded, and server logs should include `[s2script]` boot lines (gamedata validation, plugin dir, etc.) plus the base plugins loading.

## Base plugins (included)

The release ships the SourceMod-parity suite from `plugins/` (demos live under `examples/` and are not packaged):

`adminhelp` · `adminmenu` · `antiflood` · `basebans` · `basechat` · `basecomm` · `basecommands` · `basetriggers` · `basevotes` · `clientprefs` · `funcommands` · `playercommands` · `reservedslots`

Opt-in plugins under `disabled/` (nominations, rockthevote, nextmap, funvotes) are **not** in the zip — build and drop them yourself if you want them.

## Add more plugins

Build a plugin to a `.s2sp` (see the README authoring section), then copy it into:

```
addons/s2script/plugins/<name>.s2sp
```

The runtime watches that directory (top-level only, ~1s poll): drop → load, replace → hot-reload, delete → unload. No server restart required for plugin changes.

Plugins declare `s2script.apiVersion` (today `"1.x"`). The host refuses a mismatched **major** at load time.

## After a CS2 update

- Re-check `gameinfo.gi` — a full game re-download often removes the Metamod SearchPath; re-apply the patch above.
- If signatures/offsets moved, install a newer s2script release (updated `gamedata/core.gamedata.jsonc` and/or binaries). Prefer replacing the whole zip contents rather than mixing versions.

## Publishing a release (maintainers)

There are **two independent release trains**. Do not couple them — plugin-only updates need a zip tag; package-only updates need a changeset.

### Runtime zip (binaries + base plugins)

```bash
git tag v0.1.1
git push origin v0.1.1
```

The [`release`](../.github/workflows/release.yml) workflow sniper-builds (GLIBC ≤ 2.31), builds base plugins **stamped to that tag’s version**, packages `s2script-cs2-linux-*.zip`, and uploads it to a GitHub Release. Base plugins declare `s2script.apiVersion` (today `"1.x"`); they are **not** published to npm. Plugin `.s2sp` manifests always match the zip tag (e.g. `v0.1.1` → every shipped plugin `version: "0.1.1"`).

Local dry-run (after a sniper build):

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
VERSION=0.1.1 bash scripts/build-base-plugins.sh   # stamps plugins → 0.1.1
bash scripts/package-release.sh 0.1.1
# → dist/s2script-cs2-linux-0.1.1.zip
```

### npm packages (`@s2script/*` types + CLI)

Versioning and publish are owned by [Changesets](https://github.com/changesets/changesets) ([`.changeset/`](../.changeset/), workflow [`changesets.yml`](../.github/workflows/changesets.yml)). Packages version **independently** (only the ones you select in a changeset bump). CI publishes with **npm trusted publishing (OIDC)** — no `NPM_TOKEN` secret.

1. On a PR that changes `packages/`, run `npm run changeset`, select the packages that changed, and commit the file.
2. Merge to `main` → CI opens a **Version Packages** PR (only those packages + needed internal dep patches).
3. Merge the version PR → CI runs `changeset publish` via OIDC (+ automatic provenance).

#### One-time trusted-publishing bootstrap

You do **not** need to click through 29 package settings pages. Use the CLI loop (npm’s own [`npm trust`](https://docs.npmjs.com/cli/v11/commands/npm-trust/)):

```bash
npm install -g npm@latest          # need >= 11.15 for `npm trust`
npm login                          # interactive 2FA (bypass-2FA tokens won't work for trust)
scripts/bootstrap-npm-trusted-publishing.sh          # dry-run plan
scripts/bootstrap-npm-trusted-publishing.sh --apply  # publish any missing + trust all
```

On the **first** 2FA browser prompt during `--apply`, enable “skip 2FA for the next 5 minutes” so the rest of the loop is unattended.

What `--apply` does for every public `packages/*` package:

1. Classic-publishes any name that doesn’t exist yet (today usually just `@s2script/zones`)
2. Runs `npm trust github <pkg> --repo GabeHirakawa/s2script --file changesets.yml --allow-publish --yes`

Optional hardening afterward: package **Publishing access** → “Require two-factor authentication and disallow tokens” (OIDC still works; revoke leftover automation tokens).

After that, version-PR merges publish without secrets. Emergency local fallback: `DRY_RUN=1 scripts/publish-packages.sh` (classic token login — prefer OIDC CI).

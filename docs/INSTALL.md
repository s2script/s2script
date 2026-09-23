# Installing s2script (CS2, Linux)

s2script ships as a **SourceMod-style zip**: extract over your server's `game/csgo/` directory so `addons/` overlays in place. The zip includes the runtime **and** the first-party base plugins (already under `addons/s2script/plugins/`).

Linux x86-64 only. Windows is not supported yet.

## Prerequisites

1. A Counter-Strike 2 dedicated server.
2. **Stock [Metamod:Source](https://www.sourcemm.net/) with plugin API (PLAPI) 18 support**, installed under `game/csgo/addons/metamod/` according to the upstream instructions. No Metamod or KHook patches, private host binaries, or s2script-specific host build are required.

   The vendored Metamod pin `7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33` and nested KHook `1e200e4cc8e0badcb7cf941525268d6977f6a4e6` are an unmodified source reference for development tests. Operators may use a compatible official upstream release. Confirm PLAPI compatibility for that release and every other native plugin installed on the server.

   The native s2script shim stays resident during gameplay. Hot reload applies to `.s2sp` plugins managed inside s2script. Stop and restart the server for native shim or Metamod updates; native shim hot unload/reload is not required.

   For the repository's Docker setup, the optional installer defaults to the exact official 2.0.0.1467 archive and published checksum below when no source/release variables are set. Its PLAPI 18 compatibility comes from that release's checked source pin. An existing verified installation is rechecked and kept. To select an archive explicitly:

   ```bash
   # Official 2.0.0.1467 release: the unchanged vendored Metamod source pin.
   export S2_METAMOD_RELEASE_URL='https://github.com/alliedmodders/metamod-source/releases/download/2.0.0.1467/mmsource-2.0.0-git1467-linux.tar.gz'
   export S2_METAMOD_RELEASE_SHA256='f3dd81999e93ef86d45ed8f0f451c93806ad7fba1514dafd5f2623c61ca637a2'
   export S2_METAMOD_RELEASE_PLAPI=18  # operator-confirmed for this release
   # Optional: use an already downloaded archive instead of downloading the URL.
   # export S2_METAMOD_RELEASE_ARCHIVE=/path/to/mmsource-release-linux.tar.gz
   sudo docker compose -f docker/docker-compose.yml stop cs2
   bash scripts/cloud/install.sh --metamod-only
   sudo docker compose -f docker/docker-compose.yml start cs2
   ```

   This concrete example uses the [official 2.0.0.1467 release](https://github.com/alliedmodders/metamod-source/releases/tag/2.0.0.1467) and its Linux asset digest from the [GitHub release API](https://api.github.com/repos/alliedmodders/metamod-source/releases/tags/2.0.0.1467). That is a published checksum, not a signature. For another selected release, supply its independently recorded expected checksum and confirm PLAPI explicitly. Exact AlliedModders GitHub release asset URLs and legacy `https://mms.alliedmods.net/mmsdrop/<branch>/mmsource-<version>-linux.tar.gz` URLs are accepted; moving `releases/latest` URLs are not selected. The installer verifies the checksum before extracting regular files, then checks the required Linux x86-64 shared objects and GLIBC requirements. Archive hashes do not establish the ABI, and actual runtime load/handshake remains required for acceptance.

   Any release override requires all three identity variables; it never inherits the default's compatibility confirmation. A supplied tree uses `S2_METAMOD_TREE` plus `S2_METAMOD_BUILD_MANIFEST`. Schema 2 manifests identify either an official archive or an unmodified source build and hash the required runtime artifacts. The optional [source build](BUILDING.md#docker-live-gate) produces this manifest from checked upstream source and is selected explicitly with those tree/manifest variables. No manifest is invented from a candidate tree during verification. Previously installed bytes are rechecked, replacement is staged while CS2 is stopped, and rollback preserves the complete prior tree (`docker/metamod.prev`).

## Install the runtime

1. Download the latest `s2script-cs2-linux-*.zip` from [GitHub Releases](https://github.com/s2script/s2script/releases).
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
     gamedata/core/            # common / engine.source2 / game.cs2 + master.gamedata.jsonc
     gamedata/cs2/             # the CS2 game package's own facts + master.gamedata.jsonc
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

`adminhelp` · `adminmenu` · `antiflood` · `basebans` · `basechat` · `basecomm` · `basecommands` · `basetriggers` · `basevotes` · `clientprefs` · `funcommands` · `playercommands` · `reservedslots` · `zones`

Opt-in plugins (nominations, rockthevote, nextmap, funvotes) ship in the zip under `addons/s2script/plugins/disabled/` but are **not** loaded — the runtime's `plugins/` scan is non-recursive, so it skips the `disabled/` subdir. To enable one, move its `.s2sp` up one level into `addons/s2script/plugins/`.

## Add more plugins

Build a plugin to a `.s2sp` (see the [authoring docs](https://s2script.com/docs)), then copy it into:

```
addons/s2script/plugins/<name>.s2sp
```

The runtime watches that directory (top-level only, ~1s poll): drop → load, replace → hot-reload, delete → unload. No server restart required for plugin changes.

### Or install from the registry

`s2s install` downloads a plugin and its dependencies straight into the plugins
directory — handy in a Dockerfile or provisioning script, and it needs no login:

```bash
# from a manifest checked in next to your server config
s2s install --dir /path/to/csgo/addons/s2script/plugins

# or by name
s2s install rtv@^1.0.0 --dir /path/to/csgo/addons/s2script/plugins
```

A manifest (`s2script-plugins.json`) pins what the server should have:

```json
{ "plugins": { "rtv": "^1.0.0", "@edge/foo": "2.1.0" } }
```

Base `@s2script/*` plugins are skipped (already in the runtime). Unreviewed
community plugins install with a warning; pass `--reviewed-only` to block them,
or `--dry-run` to preview the resolved set.

Plugins declare `s2script.apiVersion` (today `"1.x"`). The host refuses a mismatched **major** at load time.

## After a CS2 update

- Re-check `gameinfo.gi` — a full game re-download often removes the Metamod SearchPath; re-apply the patch above.
- If signatures/offsets moved, install a newer s2script release (updated `gamedata/` tree and/or binaries). Prefer replacing the whole zip contents rather than mixing versions.

### Hot-fixing a broken signature

If the boot banner reports `gamedata FAIL <Name> — signature NOT FOUND (moved — regenerate)`, you
do not have to wait for a release. Create `addons/s2script/gamedata/core/custom/fix.jsonc` with
just the entries you are replacing:

```jsonc
{
  "signatures": {
    "SetModel": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 ...",
        "resolve": "direct"
      }
    }
  }
}
```

Files in `custom/` are applied after everything shipped, in sorted filename order, replacing whole
named entries. Never edit the shipped files — an upgrade overwrites them. Overrides are announced
at boot and recorded in any crash report, so a patched signature is never mistaken for a shipped
one.

#### The one thing an override does *not* replace: `validate`

Some shipped signatures carry a `validate` block — a **semantic** check on whatever address the
pattern resolved to (does the function reference the log string it should? is it really a member of
that class's vtable?). It exists because a *unique* match can still be the *wrong* function: the
borrowed CS# `TerminateRound` signature matches exactly once on our pinned build, at a function that
treats its second argument as a pointer. Uniqueness and the "is it in `.text`" check both pass it;
only the validator catches it.

So if your `custom/` entry supplies a pattern and **omits** `validate`, the shipped validator is
**carried forward** onto your pattern rather than deleted, and boot says so:

```
[s2script] WARN: gamedata OVERRIDE CCSGameRules_TerminateRound (cs2/custom/) supplied a pattern
but NO "validate" — the SHIPPED validator was CARRIED FORWARD and is now checked against YOUR
pattern.
```

Two follow-ups from there:

- **The descriptor is disabled with the validator's reason.** Your pattern moved the instruction the
  validator reads. Re-derive the validator's offsets for the new match and ship them in the same
  entry — pattern and validator belong together, which is why they live in one entry.
- **You want no semantic gate at all** (you are certain, or you are debugging). Say it explicitly
  with `"validate": {}`. That is honoured, and gets its own boot `WARN` — the entry is then accepted
  on uniqueness and a `.text`-range check alone.

```jsonc
{
  "signatures": {
    "CCSGameRules_TerminateRound": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 ...",
        "resolve": "direct",
        // re-derived for the pattern above; or `{}` to run with no semantic gate
        "validate": { "string-xref": { "at": 11, "dispOff": 3, "instrLen": 7, "expect": "TerminateRound" } }
      }
    }
  }
}
```

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
2. Runs `npm trust github <pkg> --repo s2script/s2script --file changesets.yml --allow-publish --yes`

Optional hardening afterward: package **Publishing access** → “Require two-factor authentication and disallow tokens” (OIDC still works; revoke leftover automation tokens).

After that, version-PR merges publish without secrets. Emergency local fallback: `DRY_RUN=1 scripts/publish-packages.sh` (classic token login — prefer OIDC CI).

## S1 acceptance bundle (maintainers)

The production runtime uses the shared validated resolver and stock Metamod KHook
provider. The native module stays resident; normal plugin updates replace `.s2sp`
archives. The separate `tools/khook-probe` and `examples/khook-acceptance` artifacts
are acceptance tools and must not enter production release packages.

Suites B and C now have source-bound registries and collectors. Prepare each run
with the controller's exact runtime/artifact identity, then use the same run folder
for collection. A remains the default suite. On a host controlling a container,
pass the bind-mounted gamedata directory explicitly:

```bash
bash scripts/test-khook-live.sh B --collect --run-dir "$S1_B_RUN_DIR" \
  --gamedata-root "$HOST_S2SCRIPT_ADDON/gamedata"
bash scripts/test-khook-live.sh C --collect --run-dir "$S1_C_RUN_DIR" \
  --gamedata-root "$HOST_S2SCRIPT_ADDON/gamedata"
```

The controller maps only the probe-listed relative gamedata files into that root,
rejects escapes, and captures SHA-256 before and after. These hashes establish the
inspected deployed inputs, not an unseen main module load-time snapshot. Missing
mapping, unresolved current-build recipes, missing callbacks and missing human
actions remain pending.

B needs a real bot for public item POST and damage witnesses, a real connected
client for chat/usercmd witnesses, and two archive replacements with the same
run/artifact handoff to observe three script generations. The fixture can drive a
safe bot item grant and owned relay output; actual damage still requires an engine
action against the hooked bot. C needs actual precache callbacks across two map
generations. Private native fixtures separately prove exact original counts and
nested manifest restoration. Public resource-add evidence makes no claim about
later rendering or a particular internal AddResource route.

Named registration phases remove only early peer callbacks asynchronously, observe
completion on a later frame, then install retained late peers around resident main
bindings. These one-way phases require a fresh process for another independent
run. They do not replace fresh operator captures in both physical module load
orders. The actual C peer join additionally needs a guard registered before the
main callback; otherwise that peer row stays pending while independent main-frame
resource evidence can still collect. Never infer its invocation from matching raw
pointer values after the main scope ends.

The integrated S1 release is still pending exact-head review and mandatory live
acceptance. Current game capability failures require verified gamedata, not relaxed
validation. Known shutdown-only exit 139 is diagnostic and nonblocking under the
accepted disposition; runtime callback retirement and stale-generation rejection
remain required. Do not run repeated quit loops to obtain a terminal green row.

### Damage recipe migration and current-build repairs

The internal damage signature is now `CBaseEntity_TakeDamageOld`, paired atomically
with the audited native `void(victim*, damageInfo*, optional damageResult*)` binding.
Public `OnTakeDamage`/SDKHooks behavior is unchanged. The previous
`DispatchTraceAttack` key is retired without an alias: its old pattern matched
unrelated output logic. Re-derive any custom damage override under the new key;
never rename/transplant that old pattern. Preserve both the exact prologue and the
TakeDamageOld semantic string validator, including its newline. Do not deploy this
recipe alone against an older runtime with the incompatible damage ABI.

Static evaluation of the edited source accepts damage and the pFirst head-cell
recipe on exact builds 25218825 and 25470087, and rejects a wrong damage diagnostic.
The revised CanAcquire recipe is specific to build 25470087 (1.41.8.2), retaining
both prologue and weapon string validators; it correctly rejects build 25218825.
Only those three recipes are repaired here; other unavailable targets, including
FireOutput, remain unavailable pending their own ABI/identity work.

The operator separately verified pFirst/CanAcquire resolver acceptance and factory
registration on the unchanged installed b4 runtime with official Metamod 1467 and
14 default plugins, through inferno→nuke→inferno with two bots and no humans.
CanAcquire was armed but its lazy hook was uninstalled without subscribers. This is
bounded resolution/map smoke, not acquisition callback/result, resource delivery,
new damage binding, or S1 release acceptance. Those live witnesses remain required.

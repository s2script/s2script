# Installing s2script (CS2, Linux)

s2script ships as a **SourceMod-style zip**: extract over your server's `game/csgo/` directory so `addons/` overlays in place. Plugins are separate `.s2sp` archives you drop into `addons/s2script/plugins/`.

Linux x86-64 only. Windows is not supported yet.

## Prerequisites

1. A Counter-Strike 2 dedicated server.
2. **[Metamod:Source 2.0](https://www.sourcemm.net/)** installed under `game/csgo/addons/metamod/` (same as any other Metamod plugin).

## Install the runtime

1. Download the latest `s2script-cs2-linux-*.zip` from [GitHub Releases](https://github.com/GabeHirakawa/s2script/releases).
2. Extract it into `game/csgo/`:

   ```bash
   cd /path/to/cs2/game/csgo
   unzip /path/to/s2script-cs2-linux-0.1.0.zip
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
     plugins/          # drop .s2sp here
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

   You should see `s2script` loaded, and server logs should include `[s2script]` boot lines (gamedata validation, plugin dir, etc.).

## Install plugins

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

```bash
git tag v0.1.0
git push origin v0.1.0
```

The [`release`](../.github/workflows/release.yml) workflow sniper-builds (GLIBC ≤ 2.31), packages `s2script-cs2-linux-0.1.0.zip`, and uploads it to a GitHub Release for that tag.

Local dry-run (after a sniper build):

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
bash scripts/package-release.sh 0.1.0
# → dist/s2script-cs2-linux-0.1.0.zip
```

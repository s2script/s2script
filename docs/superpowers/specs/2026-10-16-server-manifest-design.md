# Server plugins.json + plugins-lock.json, updated at startup

**Status:** implemented on `sdk/server-manifest`.

## Goal

Let an operator manage a server's plugins the way an npm project manages its dependencies. Provide
one command that generates the files, and have the runtime keep the plugins current on every start.

## Decisions

| Question | Choice | Why |
| --- | --- | --- |
| What a boot does | Update within ranges, as `npm update` does | The operator's choice. A `^` range bounds what can change, and the major never moves on its own. |
| Where the list lives | `addons/s2script/plugins.json`: npm-style `dependencies` plus flat settings (`with`, `autoUpdate`, `reviewedOnly`, `updateTimeoutSeconds`, `registry`) | Familiar npm shape, but deliberately not `package.json`. In a folder with a `package.json`, `npm install` resolves `@edge/rtv` from npmjs.com, so anyone who registers that name there gets code run on the game server. npm would also overwrite the lock and editors and Dependabot would treat the folder as Node. `manifest.json` was rejected because every `.s2sp` already has one. `plugins.json` says what it holds and pairs with the `plugins/` folder beside it. It lives in the addon root, not inside `plugins/` (the loader's scan target, which operators shuffle files in and out of) and not in `configs/` (per-plugin `configs/<id>.json`, where a plugin with id `plugins` would collide). The file's name is the marker, so no `s2script.server` flag or `private` is needed. |
| Lock shape | `plugins-lock.json`, npm lockfile v3. `packages` keys are install paths (`plugins/<file>.s2sp`), and `integrity` is an SRI `sha256-<base64>` | Mirrors npm. The registry's hashes are sha256. |
| Ownership | The lock is the ownership record | Only listed files are replaced or pruned. A pruned file must still hold the locked bytes, so a hand-edited file stays. Unlisted (hand-dropped) plugins are never touched. |
| Resolution | The registry's existing `/api/v1/plan`, once per dependency, with `with` for bundle members | No semver engine in the native core. The runtime and the CLI resolve identically. |
| Where the runtime runs it | `loader::set_plugins_dir` (Metamod Load), on the shared tokio runtime. The loader's first scan is held until the update finishes or `updateTimeoutSeconds` (default 60) passes | Load never blocks. Plugins still load from what is on disk if the update fails. |
| Atomicity | Stage every download as `<file>.s2s-tmp` and verify each sha256. Commit (renames, prune, lock) only when all succeeded. A deadline that passes before the commit abandons it | A partial update never runs. The loader ignores non-`*.s2sp` files, so staged files are invisible. |
| Restricted plugins | `S2SCRIPT_TOKEN` or `configs/registry.json` `{ "token" }`, sent only to the registry's host | Secrets stay out of the committed plugins.json. A redirect to the CDN drops the header. |
| Generator | `s2s server init` | It manages a plugin only when the registry publishes that exact version with the same bytes. Everything else is reported as unmanaged. |
| CLI parity | In a server dir, `s2s install` installs the lock exactly when the lock still matches plugins.json (`npm ci`-like), and otherwise resolves while keeping locked versions that still satisfy. `s2s update [names]` is the boot behaviour on demand | npm's own split between `install` and `update`. |

## Shared format

`packages/sdk/src/server/lock.ts` and `core/src/server_update.rs` both serialize the lock to the
same bytes. Both test suites round-trip `packages/sdk/test/fixtures/server-lock/plugins-lock.json`.
A live run had the runtime and the CLI each accept the lock the other wrote, unchanged.

## Replaces s2script-plugins.json

The earlier `s2s install --dir <plugins> [--file s2script-plugins.json]` flow is removed. It read
`{ "plugins": { name: range } }` from wherever the command ran, kept no lock, overwrote files by
name and never updated on its own. Nobody depended on it, so `plugins.json` replaces it outright
rather than running beside it. `s2s install` creates `plugins.json` on first use, as `npm install`
creates `package.json`. `--with` records bundle members under `"with"`, and `--reviewed-only`
becomes `"reviewedOnly": true` in the file.

## Not in this slice

- Updating while the server runs (only at startup).
- A per-plugin opt-out of auto-update. `autoUpdate` is server-wide, and a pinned range (`1.2.3`)
  freezes one plugin.
- Live CS2 gate. The core path was exercised against a local registry over real HTTP, but not
  inside a running CS2 server.

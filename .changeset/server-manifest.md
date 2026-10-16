---
"@s2script/sdk": minor
---

Servers manage their registry plugins with `plugins.json` + `plugins-lock.json` in
`addons/s2script/`: npm's `dependencies` and lockfile shapes, under names npm never picks up.
`s2s install name[@range]` adds a plugin (creating `plugins.json` on first use, like npm), plain
`s2s install` installs exactly the lock, and the new `s2s update [names]` moves to the newest
version each range allows. `s2s server init` writes both files from plugins already installed. The
runtime does the same update at every startup.

**Breaking:** the `s2script-plugins.json` manifest and `s2s install --dir` / `--file` /
`--reviewed-only` are removed; `plugins.json` replaces them (`"reviewedOnly": true` in the file).

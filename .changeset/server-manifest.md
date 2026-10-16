---
"@s2script/sdk": minor
---

Servers can manage their plugins with `s2script.json` + `s2script-lock.json` in `addons/s2script/`:
npm's `dependencies` and lockfile shapes, under names npm never picks up. `s2s server init` writes
both from the plugins already installed (anything the registry publishes byte for byte becomes a
`^version` dependency). In a server directory, `s2s install` installs exactly the lock (or adds
`name[@range]`), and the new `s2s update [names]` moves to the newest version each range allows.
The runtime does the same update at every startup. The older `--dir`/`--file s2script-plugins.json`
flow is unchanged.

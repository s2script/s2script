---
"@s2script/sdk": minor
---

A server can install a plugin without loading it: `s2s install <name> --disabled` (and
`Plugins.install(names, { disabled: true })`, behind `sm plugins install --disabled`) puts it in
`plugins/disabled/` and lists it under `"disabled"` in `plugins.json`, where the startup update
still keeps it current. New `s2s enable` / `s2s disable` (and `Plugins.enable/disable`, behind
`sm plugins enable|disable`) move an installed plugin between the folders without re-downloading.

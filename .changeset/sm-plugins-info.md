---
"@s2script/sdk": minor
---

`s2s build` copies the package's `description`, `author` and `homepage` into the plugin manifest,
so `sm plugins list` shows each plugin's version and author and the new `sm plugins info` shows its
description and URL. `Plugins.list()` entries gain `version`.

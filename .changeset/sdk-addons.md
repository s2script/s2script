---
"@s2script/sdk": minor
---

Mount Steam Workshop addons on the server and push them to every client without a separate
Metamod plugin. `@s2script/sdk/addons` adds `Addons` (`status`, `mount`, `unmount`, `mounted`,
`setExtraAddons`, `setClientAddons`, `clientAddons`, `configure`, `clearClientCache`, `currentMap`,
`owner`). Map loads carry the extra addons after the map's own one, and each client is walked
through downloading the addons it lacks, one per reconnect. This replaces MultiAddonManager.

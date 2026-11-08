---
"@s2script/sdk": minor
---

Reach the server's Steam session and download Steam Workshop items. `@s2script/sdk/steam` adds
`Steam` (`isAvailable`, `isLoggedOn`, `isSecure`, `steamId`, `publicIp`, `onConnected`,
`onDisconnected`, `onConnectFailure`) and `Workshop` (`download` → `Promise<{ id, result, ok }>`,
`request`, `state`, `isInstalled`, `downloadProgress`, `installInfo`, `suspendDownloads`,
`onDownloadResult`, `onItemInstalled`), plus the `WorkshopItemState` flags and common `SteamResult`
codes. Ids are decimal strings (a safe integer or `bigint` is accepted). This covers the workshop
half of ModSharp's `ISteamApi` and SourceMod's SteamWorks extension.

---
"@s2script/sdk": minor
---

Read client network stats. `client.netStats()` in `@s2script/sdk/clients` returns one snapshot of the
client's net channel: latency, loss, choke, data and packet rates per direction, totals, connection
time and timeout state (SourceMod `GetClientLatency`, `GetClientAvgLoss`, `GetClientAvgChoke`,
`GetClientAvgData`, `GetClientAvgPackets`, `GetClientTime`, `IsClientTimingOut`, `GetClientDataRate`).
It returns `null` for a bot.

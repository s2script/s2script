---
"@s2script/sdk": minor
---

Query and replicate client cvars. `client.queryConVar(name)` in `@s2script/sdk/clients` asks the client
for its own value of a cvar and resolves `{ status, value }` (SourceMod `QueryClientConVar`), or `null`
for a bot or a client that leaves first. `client.replicateConVar(name, value)` sets the client's copy
of a replicated cvar without changing the server's value (SourceMod `SendConVarValue`).

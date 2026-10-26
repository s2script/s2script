---
"@s2script/sdk": minor
---

Read and write the engine's network string tables. `@s2script/sdk/stringtables` adds
`StringTables.find(name)` / `get(id)` / `count()` / `all()`, returning a `StringTable` handle with
`count()`, `findString()`, `getString()`, `getData()`, `setData()` and `addString()`. A handle is
plain data and is looked up again on every call, so one kept across a map change answers
-1 / null / false. This matches SourceMod's string table natives and ModSharp's
`INetworkingStringTable`.

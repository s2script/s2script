---
"@s2script/sdk": minor
---

Precache models (and any resource) through the game session manifest

`PrecacheContext.add` accepted model paths and never made them resident. A plugin precached a model,
`add` returned true, no warning was logged, and the model then spawned as the pink-and-black ERROR box
with the engine complaining "requested but is not in the system (Missing from a manifest?)". Sounds were
unaffected, which is why it went unnoticed: they go through a separate global helper, and the precache
slice was specified and tested for soundevents only.

The adds were reaching the manifest handed out by `CGameRulesGameSystem::OnPrecacheResource`, which does
not govern residency. The one that does is the GAME SESSION manifest, delivered to game systems as
`EventBuildGameSessionManifest_t::m_pResourceManifest`, so the shim now registers a game system to
receive it — the same way CounterStrikeSharp does, which is why the same model paths work there.
Registration needs `CBaseGameSystemFactory::sm_pFirst`, which is not exported and is sig-resolved; if
that resolve fails, nothing is registered and model precaching stays inert rather than crashing.

`add` still reports only that the engine accepted the string — it returned true for a path with no file
behind it — so its result is not evidence a model will load.

Also fixes `UserMessage.setString` silently dropping the value on a REPEATED field. It returned 0 and
the message went out without the text while `send()` reported success, because delivery had happened.
`CUserMessageTextMsg.param` is repeated, which is why centre-screen hint text arrived blank. Repeated
string fields now append via `AddString`.

Known limitation: a plugin loaded AFTER the session manifest was built (the boot map) misses that map's
precache, so its models are unusable until the next map change.

---
"@s2script/sdk": patch
---

Document what `PrecacheContext.add` actually guarantees

It returns true iff the engine ACCEPTED the string, which is not the same as the resource existing or
being loadable — it returns true for a path with no file behind it too, and the engine only objects
later, at spawn, with "requested but is not in the system (Missing from a manifest?)". The doc now says
so, shows a model alongside the soundevents example, and records the timing constraint: the manifest is
built once per map, so a plugin loaded after that point (including on a fresh server's boot map) misses
that map's precache entirely.

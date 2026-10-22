---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

`EntityRef` and `Ptr` gain `readVector`, `readQAngle`, `readVectorChain` and `readQAngleChain`. The
generated schema and nav accessors are now built once, in the host's shared framework realm, and
used by every plugin; they read vectors through these methods so each plugin still gets its own
`Vector`. Objects those modules create are frozen and inherit from the framework realm, so
`view instanceof Object` is `false` in plugin code.

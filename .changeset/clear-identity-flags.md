---
"@s2script/sdk": minor
---

`EntityRef.clearIdentityFlags(mask)` — drop identity-slot flag bits

Clear-only: `mask` names bits to DROP and nothing can be raised, and the invalid-ehandle bit is refused
outright, because a plugin must never be able to present a dead slot as live.

The case it exists for is the STAGING bit. `setModel` routes through `SetupModel`, which asserts the
entity is not in the staging list, and a created-but-unspawned entity is — so the
create -> setModel -> spawn ordering that CS2's own body spawner uses was simply unavailable, and the
two remaining orderings each fail in their own way: setting the model first trips the assertion and
leaves a half-initialised skeletal entity for clients to choke on (`CopyExistingEntity: missing client
entity N`), while setting it after spawn leaves a model entity that spawned with no model at all.

Spawn keyvalues remain the simplest route when they fit. This is for the cases they do not.

---
'@s2script/cs2': minor
---

Add `notifyChanged()` to nav wrappers, so a chain write can be replicated.

Nav setters deliberately do not flag the sub-object for replication: for most targets the server reads
the field every tick and replication is irrelevant, and notifying with a chain-relative offset would
mark the wrong bytes on the wrong entity.

It is NOT irrelevant for anything a CLIENT renders. `Player.matchStats` drives the scoreboard, so a
write nobody is told about is a write nobody sees — a gamemode hiding kill counters would set them and
watch the old values stay on screen.

`notifyChanged()` notifies at `path[0]`, which is a field on the ROOT entity and therefore the offset
the engine's dirty-tracking understands — exactly what the C# reference does for this case
(`SetStateChanged(controller, "m_pActionTrackingServices")`). It stays an explicit call rather than
something setters do implicitly, because which hop to notify is a property of the chain, not of the
field being written. A wrapper reached with no pointer hop no-ops rather than notifying at a
fabricated offset 0.

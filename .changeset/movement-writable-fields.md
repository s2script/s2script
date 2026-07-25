---
'@s2script/cs2': minor
'@s2script/sdk': minor
---

Make curated `pawn.movementServices` fields writable — the `GetMaxSpeed` equivalent.

All 53 generated accessors on the movement services were read-only. 14 now have setters:
`maxspeed`, `stamina`, `surfaceFriction`, `fallVelocity`, the duck group, and the six move-input
fields. Writability is opt-in per field via a `writable` allowlist in `nav-targets.json` rather than
derived from the field's type — which byte a field lives at is regenerable layout, but whether
writing it is safe is a reviewed behavioural decision, and the type-derived set would have exposed
engine bookkeeping such as `m_nTraceCount`.

A bad allowlist entry now fails codegen instead of silently emitting nothing: an unknown field name
(what a CS2 rename produces) and a kind with no `EntityRef.write*Via` both throw, naming the field.

These writes are not flagged for replication — the change-notifier addresses the root entity while a
nav write changes a subobject — so a predicting client may see brief mismatch. Stated on the emitted
`MovementServices` interface.

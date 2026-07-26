---
'@s2script/cs2': minor
---

Reach fields behind a struct embedded in a pointer target, and add `Player.matchStats`.

A nav target could only expose fields declared on the target class or an ancestor. Anything behind an
embedded struct was unreachable, because every entry in a `read*Via` path is DEREFERENCED and an
embedded struct must not be — it is part of the object already reached.

`nav-targets.json` entries take an optional `base`: hops whose offsets are SUMMED into the field
offset instead. `CCSPlayerController_ActionTrackingServices::m_matchStats` is one — the stats live
inside the services object, so `m_iKills` is one pointer hop plus a base offset plus the field. A base
lookup that fails returns `null` rather than falling through as 0, which would silently read the start
of the target object and hand back plausible numbers from the wrong field.

`Player.matchStats` uses it: kills/deaths/assists/damage/utilityDamage are writable, the rest of the
block is engine bookkeeping and stays read-only, per the existing opt-in allowlist. This is the path a
gamemode needs to hide scoreboard counters — TTT must zero them until a body is identified, and was
carrying ~186 lines of hardcoded offsets, a probe and an operator override file to try. Those offsets
were also wrong (`+0x7f8`/`+0x98` against a real `+2760`/`+208`), so the feature was silently disabled
on every build — exactly the failure mode a borrowed constant produces, and why offsets now resolve by
name at runtime instead.

Wrapper constructors take a third `base` argument. Generated code only, but it is a signature change
in `nav.generated.js` — regenerate rather than hand-merging.

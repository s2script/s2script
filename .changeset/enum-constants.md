---
'@s2script/cs2': minor
'@s2script/sdk': patch
---

Named enum constants: `moveType = MoveType_t.VPHYSICS` instead of `moveType = 5`.

Enum fields were typed `number`, so every value had to be hardcoded from Source convention or another
framework — the borrowed-constant problem, one level below the offsets it was already solved for. The
dump now walks each enum's enumerators (`SchemaEnumInfoData_t::m_pEnumerators`) alongside the width it
already read, into a sibling `schema-enums.json`.

87 enums are emitted as frozen const objects with a value-union type, and the fields that hold them
narrow from `number` to the enum type — so a stray integer is a compile error. Only enums a generated
field can actually hold are emitted; the other 430 in the dump belong to animgraph, particle and
renderer classes that are not generated, and would be names for values nothing can be assigned.

THIS FINDS REAL BUGS. Three of the five constants a TTT port hardcoded the same day were wrong:
MOVETYPE_VPHYSICS is 5 (it used 9, which is MOVETYPE_LADDER), MOVETYPE_FLY is 3 (it used 5, which is
VPHYSICS), and kRenderNone is 2 (it used 1, which is kRenderTransAlpha — the glow only looked right
because transparent happened to be close enough).

Case is preserved rather than PascalCased: `MOVETYPE_VPHYSICS` has no unambiguous PascalCase form. The
enum's own redundant prefix is stripped (`MoveType_t.VPHYSICS`), all-or-nothing per enum — one member
that does not fit, or two that would collapse, and that enum keeps raw names, because a
partly-stripped enum is worse than an unstripped one.

Nested schema enums (`CFuncMover::Move_t`) sanitise to `CFuncMover__Move_t`; two that would collapse
onto one identifier are both dropped to plain integers rather than emitted ambiguously.

Absent `schema-enums.json` degrades to exactly the previous output — verified byte-identical.

---
"@s2script/sdk": minor
---

Plugin-declared engine calls: support static functions and stack-passed args

Two additive extensions to the declared-call format, both engine-generic.

`receiver.kind: "none"` declares a STATIC/free engine function — no `this`. The generated callable
takes no leading `self`, and the first declared arg occupies the register the receiver would have
used. `via` is rejected on such a descriptor, since a sub-object hop is a hop from a receiver.

The integer-class arg budget rises from 5 (+ `this`) to 9 (+ optional receiver). Six was the SysV
register count rather than a limit on the call; args past the sixth spill to the stack, which the
shim's max-arity prototypes now cover.

Together these make engine FACTORIES declarable from a plugin's own gamedata. They are static by
nature and commonly take more than six arguments, so previously the only route was a game-specific op
in the core — which the core-boundary gates exist to prevent.

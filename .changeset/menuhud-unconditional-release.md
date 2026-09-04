---
"@s2script/cs2": patch
---

menuhud: release the cursor on teardown even with no tracked session

The disconnect/activate teardown returned early when it held no session for the
slot, on the reasoning that there was nothing to release. That is wrong in the
exact case the teardown exists for.

A session is deleted by `renderer.close`, but the cursor grab is separate
per-player state on the layout entity and is released separately. Any path that
drops one without the other leaves the player captured — pointing at a menu that
is no longer drawn — and reconnecting could not fix it, because by then there was
no session left to find. Measured on a live server: the teardown ran and logged
nothing at all, having already returned.

Releasing state that is already released is free. Failing to release it is a
player who cannot play. The teardown no longer asks whether it thinks it has
anything to do; only the log line stays conditional, since `onActive` fires for
every joiner.

Menus also no longer freeze the player. Freezing is a convenience — it stops
someone wandering off mid-menu — while being unable to move is a trap the moment
anything else about the menu fails. On a live server that trap was real: a broken
`sm_admin` left admins frozen, unable to select anything and unable to close it.
The upside is cosmetic and the downside is a player who cannot play, so it stays
off until the menu surface can guarantee it is interactive. The unfreeze path is
kept so anyone frozen by an older build is released on the next teardown.

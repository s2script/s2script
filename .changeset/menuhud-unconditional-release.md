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

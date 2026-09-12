---
"@s2script/cs2": minor
"@s2script/sdk": minor
---

Add the CustomHudLayout observable option for CS2's September 9 update, defaulting
to false. Preserve the policy across respawns and reject conflicting resource reuse.
Keep shared hudkit UI non-observable and update passive workshop overlays to yield
to the client buy menu and scoreboard. Workshop CSS changes require asset delivery.

Add Pawn.getCustomCamera(), CustomPlayerCamera owner/mode access and native follow
configuration. Add the validated-call resolver for named call-site anchors when
native function bodies have indistinguishable byte signatures. Requires the matching
updated runtime shim.

Re-resolve the three per-player HUD setters after the observable field shifted their state vector, and use live-schema offsets in hud-lab diagnostics.

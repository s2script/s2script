---
"@s2script/cs2": patch
---

Document the public HUD API as `ui` (`import { ui } from "@s2script/cs2"`), not `ctx.ui`. Runtime still hangs the same object off the load ctx; authors import `ui`.

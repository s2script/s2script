---
"@s2script/cs2": patch
---

ui: six pooled center sheets instead of two

Two was never an engine limit — it was how many `s2_m*` panel trees the shared
workshop layout happened to define, set back when nothing needed more. Then one
plugin wanted three surfaces (a shop, a round log, an admin queue) alongside the
framework's own menu renderer, which claims one during the prelude, and four
things wanted two slots.

The failure was quiet, which is the worst part: `modal pool exhausted` in the
server log, a shop silently degrading to the chat menu and a round log to the
developer console, with nothing user-visible saying why.

`s2script_lib.xml` now defines `s2_m0`–`s2_m5` and `MODALS` is 6. The pool size
belongs to the markup — a server can only address panels the client's layout
already contains, so raising `MODALS` alone would hand out sheets that paint
nothing. A test pins the two together, and checks every sheet has its full
complement of rows, footers and detail lines.

**Requires a workshop republish of item 3790153369.** A client on the older
addon has only `s2_m0`/`s2_m1`; Panorama ignores unknown ids silently, so such a
client sees nothing for a third concurrent sheet rather than breaking. Claims are
handed out lowest-first, so the common case keeps working on an old addon.

Each sheet costs ~51 panel ids: the layout interns 432 of the 1024 cap.

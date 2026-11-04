---
"@s2script/cs2": patch
---

`HudInput` watches the real Tab bit (`IN_SCORE`, bit 33; bit 16 was the walk key), and `HudInput.arm` takes `hold: true` with `onRelease`: while the slot is armed, holding Tab gives the cursor and releasing it takes the cursor away. The vote rail uses hold mode, so holding Tab while a vote is open lets you click an option.

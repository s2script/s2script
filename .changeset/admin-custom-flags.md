---
"@s2script/sdk": minor
---

Add SourceMod's six custom admin flags (`ADMFLAG.CUSTOM1`-`CUSTOM6`) to the admin flag model

`admin_groups.json` entries using flag letters `o` through `t` (SM's `Admin_Custom1`..`Custom6`)
used to be silently dropped — the letter parser capped out at `n`, logging only an "unknown admin
flag letter" line, with no error and no group grant. Those letters now resolve correctly: `o`=bit
15 (32768) through `t`=bit 20 (1048576), continuing on from `z`=`ROOT` at bit 14 and matching SM's
own `Root=14, Custom1=15` numbering. `ADMFLAG.CUSTOM1`-`CUSTOM6` are also now exported from
`@s2script/sdk`'s `admin.d.ts`, so plugin code can reference them by name instead of raw letters.
Custom flags are how a server expresses its own permission tiers, so this closes a real SourceMod
parity gap.

---
"@s2script/cs2": patch
---

ui: add `Row.tone` so a list row can carry colour

Rows could say "unavailable" (`disabled`) but never "good", "careful" or
"bad" — the only levers a server has are classes and dialog variables, and no
class existed for a row tint. Anything that wanted to call out one line in a
list had to spend the text, prefixing a marker like `[BAD]` and hoping it read.

`tone: "good" | "warn" | "bad"` sets a class on the row button, and the
stylesheet tints the primary cell through a descendant rule — the same shape
`.s2-btn-good .s2-btn-label` already uses, so the tint outranks `.s2-cell-a`
by specificity and composes with the selection highlight and `disabled`
instead of fighting them.

Needs a workshop addon carrying the new `.s2-li-good` / `-warn` / `-bad`
rules. On an older addon the client has no rule for the class and the row
renders untinted, so a tone must never be the ONLY way a row says something.

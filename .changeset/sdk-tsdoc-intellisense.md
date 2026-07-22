---
"@s2script/sdk": patch
---

Rich TSDoc across the author-facing `@s2script/sdk` capability stubs: every exported symbol and member now carries a doc comment — a summary line, `@param`/`@returns`/`@throws` where they add signal, `{@link}` cross-references, and an `@example` (drawn from real plugin/example usage) on each major entry point — so hovering any SDK symbol in an editor gives complete intellisense. Types are unchanged; this is a comments-only pass verified against every base plugin and example.

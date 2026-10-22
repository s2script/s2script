---
"@s2script/sdk": patch
---

Generated schema and nav accessors keep each field's resolved offset in a per-field slot instead of calling the offset native on every read and write. Offsets cannot change while the server runs, so a hit is kept for good and a miss is retried, as before. Regenerate with `s2s gen-schema` / `gen-nav` to pick it up.

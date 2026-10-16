---
"@s2script/sdk": minor
---

`s2s deploy --access public|restricted` and package.json `publishConfig.access`, as in npm: a
restricted package is visible only to its org's members. Only a scoped package can be restricted.
The CLI checks the registry's `/api/v1/capabilities` first and refuses a restricted deploy that
would go out public. `s2s install` now sends your login (or `S2SCRIPT_TOKEN`), so it can fetch
restricted packages. A workspace deploy no longer reports a 409 that is not a duplicate version
(for example `access_mismatch` or `kind_mismatch`) as "already published".

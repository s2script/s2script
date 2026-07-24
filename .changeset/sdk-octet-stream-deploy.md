---
'@s2script/sdk': minor
---

Deploy over `application/octet-stream`, report registry errors properly, and log in through the browser.

**Breaking:** `s2s deploy` now posts a single zip (`manifest.json` + `plugin.s2sp` + optional
`types.tgz`) as `application/octet-stream` instead of `multipart/form-data`. It requires a registry
running the matching server change and cannot deploy to an older one.

Multipart was the reason deploys failed: SvelteKit's CSRF guard rejects form-content-type POSTs
whose `Origin` does not match, and Node's `fetch` sends no `Origin` at all, so every deploy was
refused before the server ever read the token.

Errors are now surfaced instead of swallowed. The client previously read only `body.error`, but the
registry emits `{message, status}` from SvelteKit's `error()` helper, `{error, code}` from explicit
JSON responses, and plain text from the CSRF guard — so most failures showed up as a bare
`deploy failed (403)`. All four client methods now handle every shape, plus HTML error pages, empty
and malformed bodies, network failures, and timeouts, and say what to do next.

`s2s login` opens the browser and uses device authorization rather than asking for a pasted token.
Pasting still works, and `S2SCRIPT_TOKEN` is unchanged for CI.

The registry URL now defaults to `www` and saved apex URLs are normalized, because an apex redirect
downgrades POST to GET and strips the `Authorization` header.

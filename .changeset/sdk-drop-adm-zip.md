---
"@s2script/sdk": patch
---

Replace the `adm-zip` dependency with `fflate` for all `.s2sp` zip read/write in the `s2s` CLI (`build`, `config gen`, `deploy`). `adm-zip <0.6.0` carries a high-severity advisory (GHSA-xcpc-8h2w-3j85, crafted-ZIP 4 GiB allocation) that kept surfacing in `npm audit`; `fflate` has no known advisories, ships its own types, and is small enough to bundle into `dist/cli.js` — so it is no longer a runtime dependency at all (the CLI now installs zero third-party zip code). Archive output is unchanged (standard DEFLATE members read by core's `read_s2sp`); verified against the base-plugin build and an independent `unzip` reader.

Also refresh the `s2s login` prompt for the registry's new auth flow: it now points at the full `<registry>/account/tokens` URL and notes you sign in (or create an account) first, and fixes a stale `s2script login` reference.

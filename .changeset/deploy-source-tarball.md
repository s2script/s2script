---
"@s2script/sdk": minor
---

`s2s deploy` now uploads the package's source alongside the build, as `npm publish` does: an
npm-shaped tarball (every file under `package/`) chosen with npm's own rules — package.json
`files`, `.npmignore`/`.gitignore`, and npm's always-in/never-in lists, plus `.env` files kept
out. The registry serves it from the package page. New `s2s pack [dir] [--dry-run] [--json]
[--pack-destination <dir>]` lists or writes the same tarball, like `npm pack`.

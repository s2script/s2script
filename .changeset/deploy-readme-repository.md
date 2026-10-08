---
"@s2script/sdk": minor
---

`s2s deploy` sends the package's README and source link to the registry. A `README.md` beside
`package.json` (found the way npm finds one) rides in the deploy as its own entry, and package.json's
`repository` field goes into the deploy manifest. Both fill in the package's page on s2script.com
and never enter the `.s2sp`/`.s2lib` manifest the runtime reads. A README over the registry's
100,000-character cap is refused before anything uploads.

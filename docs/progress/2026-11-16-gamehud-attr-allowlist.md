## GameHud layouts pass CustomHud validation (Nov 16)

The first live load of the published game-kit addon rejected `s2script_gamehud.xml`:
`Layout contains disallowed attribute scaling for panel type 'Image'`. The generator gave every
`Image` (kill-feed weapon icons, drawer thumbnails) `scaling="stretch-to-fit-preserve-aspect"`; it is
dropped, and the generator now refuses any attribute other than `id`, `class`, `text` and `src`.
`PANORAMA.md` records the allowlist. The addon must be recompiled and republished.

# panorama-preview (prototype)

Shows a `custom_hud_layout` in a browser the way the game lays it out. Input is the same XML and
CSS the workshop addon ships, plus a scenario: which panels the server has shown, which classes it
has set, and the dialog variable strings.

```bash
npm run build   # scenarios/gamehud.json -> out/gamehud.html (self-contained, opens from disk)
npm test        # 8 tests, no dependencies
```

## Why it can match the game

A preview built from web CSS drifts, because Panorama's layout is not the web's: `flow-children`,
`fit-children`, `fill-parent-flow()`, per-axis alignment. `src/engine.mjs` implements those rules
itself and computes every panel's rectangle. The browser does two things only: measure a string in
a font, and paint a box at the coordinates it is given. Nothing is positioned by web layout.

CustomHud keeps this small: four panel types, no scripting, and the real game kit stylesheet uses
about 40 properties.

## What is not verified yet

`ASSUMPTIONS` in `src/engine.mjs` lists every layout rule taken from documentation or memory rather
than from an in-game capture (box model, percentage bases, alignment in a flowing parent, the
cascade, text inheritance, default line height, the font). The preview is as accurate as that list
is short. Closing it takes a calibration layout: one cell per rule, screenshotted in game at
1920x1080 and compared against this engine's output for the same file.

Known gaps beyond that list:

- Text is measured and drawn by the browser. Glyph shapes and widths match only with the game's
  font installed, and antialiasing will still differ.
- Images are drawn as placeholders: `src` points at game resources.
- `transition`, `@keyframes`, `background-blur`, `wash-color` and Panorama's `text-shadow` strength
  are not emulated. `:hover` is off unless the scenario sets it.
- `overflow: squish` is treated as clip.

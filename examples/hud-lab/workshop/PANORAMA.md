# Writing layouts for `custom_hud_layout`

Reference for anyone authoring a HUD addon. Everything here is from Valve's own files or extracted
from `libpanorama.so` — not inferred.

## Two vocabularies, and they are not the same

`libpanorama.so` implements **full Panorama**: 140 CSS properties, and XML attributes like
`onactivate`, `oncontextmenu`, `style`, `draggable`, `tabindex`.

`custom_hud_layout` exposes a **strict subset**. From Valve's `point_script.d.ts`:

```
<Panel>  id, class, hittest
<Label>  id, class, hittest, text
<Image>  id, class, hittest, src
<Button> id, class
"Styling with css is supported. Events and client side scripting are not supported."
```

**Do not wire `onactivate`, and do not use `style=`.** They exist in the engine and are dumped by
`dump_panorama_css_properties`, but they are not available to us. `resourcecompiler` will not stop
you and there is no runtime error — the attribute is simply ignored. Every layout offset must live
in a class.

## The include line — get this wrong and everything renders unstyled

```xml
<include src="file://{resources}/styles/custom_game/<name>.css" />
```

Not `s2r://`, no `panorama/` prefix, and the **source `.css`**, not the compiled `.vcss`. Proven by
A/B compile: an `s2r://` include compiles clean and the stylesheet is never attached. **The tell** —
a resolved include makes the compiler print the `.css` a second time.

## The entity's layout path

```
panorama/layout/custom_game/<name>.xml     ← .xml SOURCE extension
```

`.vxml` and `.vxml_c` are rejected by the client: `Layout xml is an invalid resource name`.

## CSS: what does not exist

These silently do nothing. The compiler validates none of them.

`display` · `flex` / `grid` and every companion (`justify-content`, `align-items`, `gap`, `order`,
`flex-*`) · `float` · `clear` · `top`/`right`/`bottom`/`left` · `box-sizing` · `content` ·
`list-style` · `outline` · `filter` · `object-fit` · `aspect-ratio` · `clip-path` · `mask` ·
`pointer-events` · `user-select` · `calc()` · `var()` and custom properties · `rgb()`/`rgba()`/`hsl()` ·
`@media` · `:not()` · `::before`/`::after` · attribute selectors · `:nth-of-type`

`mix-blend-mode` exists only as `-s2-mix-blend-mode`.

**Layout is flow-based, not flex**: `flow-children` plus `width`/`height` taking `fit-children`,
`fill-parent-flow(w)` or `width-percentage(p)`, plus `position: x y z`, `align`, `ignore-parent-flow`.

## CSS: what exists and is worth using

| | |
|---|---|
| `box-shadow` | `[inset\|fill\|hollow] color hOff vOff blur spread` — real elevation |
| `gradient()` | 2008 WebKit form: `gradient( linear, 0% 0%, 0% 100%, from(#a), to(#b) )`; radial too |
| `background-blur` | `gaussian(n)` — blurs what is BEHIND the panel. Frosted glass over the game world |
| `text-shadow` | `hOff vOff blur strength color` — note `strength`, which has no CSS analogue |
| `overflow` | `squish \| clip \| scroll \| noclip` |
| others | `text-transform`, `letter-spacing`, `line-height`, `paragraph-spacing`, `white-space`, `text-overflow`, `z-index`, `saturation`, `brightness`, `contrast`, `hue-rotation`, `ui-scale`, `cursor`, `transition-timing-function`, `@keyframes` + `animation-*` |
| `sound` / `sound-out` | plays a named sound when a selector applies/unapplies |

**Selectors**: `:hover` `:active` `:focus` `:selected` `:disabled` `:descendantfocus` `:root`, and
structurally `:nth-child` `:first-child` `:last-child`. Descendant selectors are confirmed working
in-game.

`font-weight` accepts `light | thin | normal | medium | bold | black`.

**`@define` and `@import` are supported at-rules.** `@define` is the closest thing to a CSS variable
and is resolved at compile time — the sane way to express a theme palette.

## Four things that bite

**A blank panel is not a load failure.** Every `{s:...}` resolves to an empty string when unset, so
a correctly-loaded production layout renders as an empty frame until the plugin fills it. Use a
literal-text demo layout when you want to eyeball the design with no plugin attached.

**The root `<Panel>` may not have an `id`.** Hard resource-compile error. So a theme class cannot go
on the document root — put it on the first child and treat that as the themed wrapper.

**Animated hide is two-phase.** `visibility: collapse` cannot be transitioned:

```
show:  fade ON, hide OFF, wait ~0.03s, fade OFF
hide:  fade ON, wait ~0.25s (> the CSS duration), hide ON
```

Guard each slot with a **generation counter** so a re-fired slot abandons its in-flight timer.
Without it a fast second event yanks the first off screen mid-life — the classic kill-feed bug.
`Hud.showAnimated` / `hideAnimated` / `flash` implement this.

**The client is the only real CSS validator.** It checks at load and pops a dialog naming file,
line, column, property and value. The compiler checks nothing. **Load once after every CSS change** —
that dialog is the entire validation channel in this pipeline.

## Things that turn out not to work

- **`background-blur` is useless here.** It composites Panorama content, and the 3D world is not in
  the panorama tree — frosted glass over the game is unreachable from this API.

## One piece of genuinely client-side interactivity

```css
.s2-tip-host:hover .s2-tip { opacity: 1; }
```

A pure-CSS hover reveal needs **no server call at all**, and generalises to any hover-to-reveal
pattern — row detail, hover cards. It is the only client-side interactivity available.

## Consequences for the server-side API

**Zebra striping should be `:nth-child`, not server-set classes.** That removes per-row bookkeeping
from the framework entirely. (The explicit `.s2-tr-zebra` class is what is currently proven in-game;
`:nth-child` is the better end state once verified.)

**Theming works by cascade.** A class on the root reaches every component, and because
`SetHasClassForPlayer` is per-player, two players can see different themes of the same layout at
once. Palettes must be pre-baked — there is no `var()` — so the server *selects* from a finite set.
`Hud.setTheme()` does this, clearing the previous theme class first (classes accumulate).

**`background-blur` costs a composition pass per panel.** Keep it to top-level surfaces. If a
framework ever lets users mark arbitrary panels frosted, cap it — this is a HUD that can redraw
every tick.

**Prefer `id == varName`.** A layout whose Labels use `id="timer_value"` with `{s:timer_value}`
needs no mapping layer — `Hud.set(slot, id, value)` is the whole binding.

**Meter widths are step classes**, not numbers. `Hud.setMeter()` takes 0–100 and rounds to the
nearest 5%, and clears the previous step first.

**`disabled` is cosmetic.** `.s2-btn-disabled` greys a button; the click still arrives. Enforcement
is server-side — `Hud.setDisabled()` does both.

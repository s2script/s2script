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

## Hard caps — the one that will bite at scale

`CCSCustomHudLayout` interns every name the server references into three networked vectors, and all
three are capped. `server.dll`:

```
The maximum number of panel ids has been reached, no more can be referenced.
The maximum number of class names has been reached, no more can be referenced.
The maximum number of dialog variables has been reached, no more can be referenced.
```

**The cap is on what the SERVER references, not what the layout declares.** A panel you never touch
is free. A name is interned the first time you pass it to a setter and holds a slot forever. So a
large layout costs nothing; a chatty server does. `SetDialogVariableString` interns **both** its
panelId and its variableName, into separate vectors.

The ceiling is **1024 per vector** (`CMP dword [...], 1024` + `JL` past the warning, so it fires at
>= 1024). Generous alone, but shared by every plugin on the entity — bespoke per-plugin layouts
consume it multiplicatively and hit the wall around plugin 14.

**Never generate names dynamically.** `row_{page}_{i}` will exhaust the budget; reuse a fixed pool of
ids and change their content instead. And the vectors belong to the **entity**, so one entity
driving two layouts pays for both.

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

## Clamping untrusted text

Player-typed text that reaches an admin's screen (a report reason, a nickname) must not be able to
push the surrounding UI around. The clamp is a **fixed-height wrapper** — `.s2-clampbox`, `height:
62px` — and that height is the load-bearing part. It bounds the region; the overflow policy only
decides what happens at the boundary.

`overflow: squish squish` and `text-overflow: ellipsis` are written explicitly on the box, but both
are Panorama **defaults** ("Children are squished to fit within the panel's bounds if needed
(default)"; "We default to ellipsis, which is contrary to the normal CSS spec"). So they are
reinforcement, not the mechanism: if the parser rejected either declaration the behaviour would
fall back to itself. The mitigation is not one declaration away from vanishing.

One open question, worth observing rather than pre-empting: whether `squish` **clips** the label or
**compresses** it. "Squished to fit" is ambiguous, and for text those differ — clipped-with-ellipsis
is wanted; scaled-down-to-fit is legible but lets an author shrink an admin's reading of their own
UI. Either way the panel keeps its bounds, so the failure actually being guarded against — a wall of
text shoving the verdict buttons off screen — cannot happen. If it does compress, the fix is
`text-overflow: shrink min( 10px ) ellipsis` (shrink to a floor, then ellipsise the remainder).

Escaping still happens server-side before the value is ever set; the clamp is the second layer, not
the first.

## Reveal synchronously; animate only on the way out

Showing a panel must not depend on a later callback. Clear any `*-out` (fade/transform) class
FIRST, then clear the hide class, in the same call. Acquire the cursor LAST, after it is visible.

The asymmetry is the point:

- A failed **hide** leaves the panel on screen — loud, obvious, self-reporting.
- A failed **reveal** leaves a panel that is present, sized, capturing input, and at `opacity: 0` —
  completely invisible, with no error client-side or server-side, and indistinguishable from an
  addon that never loaded.

So exit animations are safe and entry animations are not. Clearing the `*-out` class before
un-hiding also means a panel left transparent by an older build recovers on its next open instead
of staying stuck.

Structure belongs in markup (`.s2-sheet`, `.s2-li`, `.s2-toast`, `.s2-hudbadge` and the layout
classes); the SERVER should only ever drive **state** — the hide class, `*-out`, selection,
disabled, variants and width modifiers. Asserting a structural class from the server doubles the
interned class-name count for no gain, and it masks a stylesheet that is not loading at all.

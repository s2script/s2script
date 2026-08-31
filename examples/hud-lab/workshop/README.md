> ## Status: addon layouts WORK on retail as of CS2 build 24934554 (2026-08-25)
>
> Valve flipped the gate in that update. `csgo_core/gameinfo.gi` now ships `"AllowCustomGameUI" 1`
> (it was `0`), and the blanket refusal in `libpanorama.so` — `Plat_FatalError("Addons cannot add
> layouts.")` — is replaced by a SCOPED one:
>
> ```
> Error loading %s: Addons can only add layouts in the %s subdirectory.
> ```
>
> The only such path in the library is `panorama/layout/custom_game/`, which is where these files
> already live. On builds BEFORE 24934554 this addon would have crashed clients outright, so do not
> ship it to an older client.
>
> ## Compiling (verified 2026-08-25 on Windows, Workshop Tools)
>
> Two invocation traps cost real time; both are in the working command:
>
> ```bash
> cs="/c/Program Files (x86)/Steam/steamapps/common/Counter-Strike Global Offensive"
> cd "$cs/game/bin/win64"
> ./resourcecompiler.exe -game "$cs/game/csgo" -nop4 -f \
>   -i "$cs/content/csgo_addons/<addon>/panorama/styles/custom_game/<name>.css" \
>   -i "$cs/content/csgo_addons/<addon>/panorama/layout/custom_game/<name>.xml"
> ```
>
> 1. **`-game` is required** and points at `game/csgo`, NOT at the addon. The compiler derives
>    `ModName = csgo_addons/<addon>` from the CONTENT path. Without it: `FATAL ERROR: Application
>    unable to load gameinfo.gi file from directory "csgo"`.
> 2. **Wildcards need WINDOWS BACKSLASH paths.** A forward-slash path containing `*` matches ZERO
>    files and reports `0 compiled` with no error — a silent failure. Explicit filenames are fine
>    with forward slashes.
>
> ### The `<include>` form: `file://{resources}/…` and the SOURCE extension
>
> ```xml
> <include src="file://{resources}/styles/custom_game/<name>.css" />
> ```
>
> NOT `s2r://`, NOT a `panorama/` prefix, NOT the compiled `.vcss`. Proven by A/B compile,
> three identical files differing only in the include line:
>
> | include | result |
> |---|---|
> | `s2r://panorama/styles/custom_game/x.vcss` | compiles clean, **stylesheet silently NOT attached** |
> | `file://{resources}/styles/custom_game/x.css` | compiles, and the .css is listed a SECOND time — the dependency resolved |
> | `file://{resources}/styles/custom_game/NO_SUCH.css` | **hard compile error** with line:col, `0 compiled, 1 failed` |
>
> An `s2r://` include is a silent no-op: the layout compiles, ships, and renders COMPLETELY
> UNSTYLED with no diagnostic anywhere. That is the single most likely cause of a
> "my panel loaded but looks wrong / invisible" report.
>
> **The tell:** a resolving include makes the compiler print the `.css` a second time, right after
> its `.xml`. One line = dead include. Two = attached.
>
> This is also the ONE thing the compiler does validate — see below.
>
> ### resourcecompiler validates almost nothing else
>
> Calibrated 2026-08-25 by compiling deliberately broken input. ALL of these compiled with
> `OK: 1 compiled, 0 failed`, exit 0, no warning:
>
> ```css
> .control-a{this-property-does-not-exist: 42px;}
> .control-b{color: not_a_real_color_value;}
> .control-c{width: 100zz;}
> ```
> ```xml
> <include src="s2r://panorama/styles/custom_game/THIS_FILE_DOES_NOT_EXIST.vcss" />
> <NotARealPanelType id="bogus" text="{s:nope}" />
> ```
>
> A fabricated panel element and every bogus CSS property sail through. (A broken `file://`
> include is the one exception — see above.) So it is close to a
> PACKAGER, not a validator: "it compiled" carries no information about whether the markup is
> correct, whether `s2r://` resolves, whether `{s:}` dialog variables bind, or whether any CSS
> property is supported by CS2's Panorama.
>
> The compiled header listing `m_InputDependencies` / `m_SearchPath` is not evidence either — the
> broken-include control produced the same entries.
>
> **Every syntax question can only be settled by a runtime load with the client console open.**
> Combined with `RestrictFlatFileAddonsToTools 1` (loose addon files load only in tools mode), that
> means publishing is the ONLY way to learn whether any of this works.
>
> `addoninfo.txt` is NOT compiled; it lives on the game side (`game/csgo_addons/<addon>/`).
> A successful run prints `Leaked KeyValues blocks: NNN` — that is Valve's own debug noise on a
> zero-exit success, not a problem.

# s2script HUD kit — workshop addon source

The client half of the custom-HUD story. A server plugin cannot invent panels at runtime: nothing
renders on a client that is not already on that client. So this ships a **fixed, generic surface**
and the plugin drives it entirely from the server.

```
addoninfo.txt
panorama/layout/custom_game/s2script_hud.xml
panorama/styles/custom_game/s2script_hud.css
panorama/layout/custom_game/s2_vote.xml      ← fragment to paste into s2script_lib.xml
panorama/styles/custom_game/s2_vote.css      ← include/merge into the lib stylesheet
```

That is the whole **probe** addon — no map, no models. The reference implementation it is modelled on
(workshop item 3789924061) is 5.7 KB compiled.

Production menus and the vote rail drive **workshop addon 3790153369**, layout
`panorama/layout/custom_game/s2script_lib.xml` (not checked in here). Menus already use that
lib (`s2_m0` / `s2_m1`). The vote rail is a new panel family **on that same layout**, not a
second `custom_hud_layout` resource.

To ship the rail: paste `panorama/layout/custom_game/s2_vote.xml` into the lib root, include
`panorama/styles/custom_game/s2_vote.css` from `s2script_lib.xml` (or merge the CSS into the
lib stylesheet), compile, and republish **the same** addon. Do not upload `s2_vote.xml` as its
own layout / workshop item.

## How the server drives it

Two engine calls, both armed in `../gamedata/hud-lab.gamedata.jsonc`:

| What | Call |
|---|---|
| show / hide / restyle | `SetHasClassForPlayer(slot, panelId, className, status)` |
| text | `SetDialogVariableStringForPlayer(slot, panelId, varName, value)` |

Everything variable is therefore either a **class** or a **dialog variable**, because those are the
only two levers a server has. It cannot set a style property, which is why the meter's width is a
class family (`s2-w0` … `s2-w10`) rather than a percentage.

## Panel ids

| id | purpose |
|---|---|
| `s2_dialog` | centre modal; `s2_dialog_kicker` / `_title` / `_body`, buttons `s2_btn_0..3` |
| `s2_hud_tl` `s2_hud_tr` `s2_hud_bl` `s2_hud_br` | corner readouts, each with `_head` / `_body` |
| `s2_banner` | wide centre-top announcement |
| `s2_list` | 8-row menu surface: `s2_row_0..7`, plus `s2_list_title` / `s2_list_foot` |
| `s2_meter` | progress bar: `s2_meter_label`, `s2_meter_fill` |

Ids are the addressing scheme — **keep them stable**. Renaming one silently breaks every server
calling it.

## Dialog variables

`kicker` `title` `body` `btn0`–`btn3` · `tl_head`/`tl_body` (and `tr_`/`bl_`/`br_`) · `banner` ·
`list_title` `row0`–`row7` `list_foot` · `meter_label`

Unset variables render empty, so every slot is safe to leave alone.

## Classes

- **visibility** `s2-hidden` (opacity fade; every panel starts hidden)
- **accent** `s2-accent-gold|red|green|blue`
- **text** `s2-text-white|gold|red|green|blue|grey`
- **fill** `s2-fill-red|amber|green`
- **size** `s2-sm|md|lg|xl`
- **meter width** `s2-w0` … `s2-w10`
- **rows** `s2-row-active|disabled|empty`
- **attention** `s2-dim` `s2-flash`

## Building and publishing

Needs **CS2 Workshop Tools on Windows** — the resource compiler turns `.xml`/`.css` into the
`.vxml_c`/`.vcss_c` the engine actually loads. There is no way around this from Linux or macOS: the
compiled form is a Source 2 resource whose payload is an LZ4-compressed KV3 tree, and hand-writing
one means re-implementing a format Valve ships a compiler for.

1. Install **Counter-Strike 2 → Tools → Counter-Strike 2 Workshop Tools**.
2. Create an addon: Workshop Tools launcher → *Create New Addon* → name it `s2script_hud`.
3. Copy this directory's contents into
   `.../Counter-Strike Global Offensive/content/csgo_addons/s2script_hud/`.
4. Compile the panorama files (Asset Browser will compile on scan, or run `resourcecompiler` over
   `panorama/`). Output lands in `game/csgo_addons/s2script_hud/`.
5. Publish from the Workshop Tools UI. Note the resulting **workshop id**.

### Two items, not one

Publish it twice if you want both delivery modes — the files are identical and it is ~6 KB:

- **`IsPlayable = false`** (as written here): mounts alongside any map, but **clients must have it
  subscribed**. `+host_workshop_map` does NOT deliver a content addon — it looks for maps *inside*
  the addon, finds none, unmounts, and leaves the server idle. Verified; see the note in
  `docker/docker-compose.hudlab.yml`.
- **`IsPlayable = true`** + a stub map: `+host_workshop_map <id>` hosts the map and mounts the
  assets in one step, and clients auto-download on connect. The cost is that the server runs your
  map.

## Using it

```
sm_hud_create panorama/layout/custom_game/s2script_hud.vxml
sm_hud_capture 1
sm_hud_class s2_dialog s2-hidden 0          # reveal
sm_hud_var   s2_dialog title Hello world    # set text
sm_hud_class s2_meter_fill s2-w7 1          # 70% meter
```

`m_strLayout` takes the **`.xml` source extension** — confirmed on a live client. `.vxml` and
`.vxml_c` are both rejected outright:

```
Layout xml is an invalid resource name "panorama/layout/custom_game/x.vxml"
```

The resource system resolves the compiled `.vxml_c` behind the source name, the same way
`models/x.vmdl` works.

## What is still missing

Clicks. `CS_UM_CustomHudClicked` is an inbound user message with no plugin-level path yet, so the
buttons here are styled affordances whose labels the server sets, not things it can react to. When
inbound delivery lands, swapping `<Panel class="s2-button">` to `<Button>` is the only change.

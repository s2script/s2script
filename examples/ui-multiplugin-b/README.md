# UI multi-plugin acceptance fixture B

This test-only plugin is a separate plugin context from fixture A. It opens a
higher-priority exclusive modal so the live acceptance run can observe A becoming
covered, B receiving clicks, and A restoring on a later frame after B closes.

Build from the repository root after UI hardening Tasks 3–6 are integrated:

```sh
node packages/sdk/dist/cli.js build examples/ui-multiplugin-b --packages-dir packages
```

Commands accept an optional zero-based slot and otherwise use the in-game caller:

| Command | Action |
| --- | --- |
| `sm_ui_b_open [slot]` | Open B's exclusive modal at priority 20. |
| `sm_ui_b_refresh [slot]` | Repaint synchronously with `tryRefresh`. |
| `sm_ui_b_invalidate [slot]` | Queue a coalesced repaint and report the immediate provider-call delta. |
| `sm_ui_b_close [slot]` | Close B while retaining its modal pool claim, allowing A to restore. |
| `sm_ui_b_release` | Close all B views and release B's modal pool claim. |
| `sm_ui_b_status [slot]` | Print provider, click, claim, view, and last-update state as JSON. |

The commands and counters provide server-side evidence only. A human still must prove
visible focus transfer, covered-A click suppression, later-frame restoration, cursor
release, same-slot reconnect safety, plugin reload cleanup, and pool reclaim. Reload is
performed through the operator's normal plugin workflow; this fixture does not reload
another plugin or share a JavaScript global with A.

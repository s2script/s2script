---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

TopMenu hub is a tabbed dashboard; plugins declare their own tab

`sm_admin` / `sm_menu` paint `hudkit.dashboard()` over `s2_dash` on
`s2script_lib.xml` instead of a category → item drill-down. Each plugin that
contributes items calls `topmenu.addTab({ id, title })` and `addItem(tabId, item)`.
`addCategory(name)` is still the id==title form.

Snapshot grows `tabs: [{ id, title }]`. Republish workshop addon 3790153369
after compiling the new `s2_dash` panels (sources in `examples/hud-lab/workshop/`).

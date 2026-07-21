import { plugin } from "@s2script/sdk/plugin";
import { TopMenu } from "@s2script/sdk/topmenu";
import { Menu, MenuStyle } from "@s2script/sdk/menu";
import { Admin, ADMFLAG } from "@s2script/sdk/admin";

// Pure helpers (no side effects) — module-level.
function itemsFor(category: string, flags: number) {
  return TopMenu.snapshot().items.filter(i => i.category === category && ((flags & ADMFLAG.ROOT) !== 0 || (flags & i.flags) === i.flags));
}

function showCategory(slot: number, category: string, flags: number): void {
  const items = itemsFor(category, flags);
  const m = new Menu(category);
  m.style = MenuStyle.Center;
  m.freezePlayer = true;   // WASD nav — freeze movement while the menu is open
  for (const it of items) m.addItem(it.id, it.name);
  m.onSelect(e => { TopMenu.select(e.info, slot); });
  m.display(slot, 30);
}

export default plugin((ctx) => {
  // Fix the standard category order (items land in these; a plugin may add more).
  ctx.topmenu.addCategory("Player Commands");
  ctx.topmenu.addCategory("Server Commands");
  ctx.topmenu.addCategory("Voting Commands");

  ctx.commands.register("sm_admin", (cmd) => {
    const slot = cmd.callerSlot;
    if (slot < 0) { cmd.reply("Run sm_admin in-game."); return; }
    const admin = Admin.forSlot(slot);
    if (!admin) { cmd.reply("No access."); return; }
    const snap = TopMenu.snapshot();
    // Only categories with >=1 visible item.
    const cats = snap.categories.filter(c => itemsFor(c, admin.flags).length > 0);
    if (cats.length === 0) { cmd.reply("No admin actions available."); return; }
    const m = new Menu("Admin Menu");
    m.style = MenuStyle.Center;
    m.freezePlayer = true;   // WASD nav — freeze movement while the menu is open
    for (const c of cats) m.addItem(c, c);
    m.onSelect(e => { showCategory(slot, e.info, admin.flags); });
    m.display(slot, 30);
  });

  console.log("[adminmenu] onLoad — sm_admin registered");
});

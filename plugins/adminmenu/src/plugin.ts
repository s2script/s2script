import { TopMenu } from "@s2script/topmenu";
import { Menu, MenuStyle } from "@s2script/menu";
import { Commands } from "@s2script/commands";
import { Admin } from "@s2script/admin";

// Fix the standard category order (items land in these; a plugin may add more).
TopMenu.addCategory("Player Commands");
TopMenu.addCategory("Server Commands");
TopMenu.addCategory("Voting Commands");

function itemsFor(category: string, flags: number) {
  return TopMenu.snapshot().items.filter(i => i.category === category && ((flags & (1 << 14)) !== 0 || (flags & i.flags) === i.flags));
}

function showCategory(slot: number, category: string, flags: number): void {
  const items = itemsFor(category, flags);
  const m = new Menu(category);
  m.style = MenuStyle.Center;
  for (const it of items) m.addItem(it.id, it.name);
  m.onSelect(e => { TopMenu.select(e.info, slot); });
  m.display(slot, 30);
}

Commands.register("sm_admin", ctx => {
  const slot = ctx.callerSlot;
  if (slot < 0) { ctx.reply("Run sm_admin in-game."); return; }
  const admin = Admin.forSlot(slot);
  if (!admin) { ctx.reply("No access."); return; }
  const snap = TopMenu.snapshot();
  // Only categories with >=1 visible item.
  const cats = snap.categories.filter(c => itemsFor(c, admin.flags).length > 0);
  if (cats.length === 0) { ctx.reply("No admin actions available."); return; }
  const m = new Menu("Admin Menu");
  m.style = MenuStyle.Center;
  for (const c of cats) m.addItem(c, c);
  m.onSelect(e => { showCategory(slot, e.info, admin.flags); });
  m.display(slot, 30);
});

console.log("[adminmenu] onLoad — sm_admin registered");

/**
 * An interactive button panel: pick an option, the server sees the pick and acts on it.
 *
 * HOW SELECTION REACHES THE SERVER. Not by mouse. A HUD mouse-click would arrive as
 * `CS_UM_CustomHudClicked`, an INBOUND user message with no plugin-level path (see the Tier C note
 * in plugin.ts) — so no server plugin can observe a click today. What DOES reach the server is the
 * button mask on the player's usercmd, which the centre-panel renderer already polls: W/S move the
 * cursor, E selects. That fires `onSelect` server-side with the entry's `info` key, which is a real
 * selection event you can hang anything off.
 *
 * So this is a button panel driven by W/S/E instead of the mouse. Everything downstream — echoing
 * the pick to chat, running a console command, mutating the player — works exactly as it would with
 * a click.
 *
 * Every entry echoes what was selected before it acts, because "did the server actually see my
 * pick?" is the question this panel exists to answer.
 */
import { Menu, MenuStyle } from "@s2script/sdk/menu";
import { Server } from "@s2script/sdk/server";
import { Chat } from "@s2script/sdk/chat";
import { Player, Pawn, ChatColors, CsItem } from "@s2script/cs2";
import type { Pawn as PawnType } from "@s2script/cs2";
import type { DemoHud } from "./demohud";

/** One button. `run` returns a short result string that is echoed after the selection line. */
interface Button {
  id: string;
  label: string;
  run: (slot: number, pawn: PawnType | null) => string;
}

/** How long a panel stays up before auto-closing, in seconds. */
const PANEL_SECONDS = 60;

/** Echo the selection itself — the proof the server received it — then the action's result. */
function echo(slot: number, label: string, id: string, result: string): void {
  Chat.toSlot(slot, `${ChatColors.Green}[panel]${ChatColors.Default} you selected ${ChatColors.Yellow}${label}${ChatColors.Default} (id: ${id})`);
  Chat.toSlot(slot, `${ChatColors.Green}[panel]${ChatColors.Default} ${result}`);
  // Also to the server console, so the pick is visible without being in-game.
  console.log(`[hud-lab] panel: slot ${slot} selected '${id}' (${label}) -> ${result}`);
}

function needPawn(pawn: PawnType | null): string | null {
  return pawn?.isValid ? null : "no live pawn — spawn first";
}

/** The main panel's buttons. Deliberately varied: state reads, mutations, and a console command. */
const BUTTONS: Button[] = [
  {
    id: "whoami",
    label: "Report my state",
    run: (slot, pawn) => {
      const p = Player.fromSlot(slot);
      if (!p) return "player gone";
      return `name=${p.playerName} team=${pawn?.teamNum ?? "?"} hp=${pawn?.health ?? "?"} armor=${pawn?.armorValue ?? "?"}`;
    },
  },
  {
    id: "heal",
    label: "Heal to 100",
    run: (_slot, pawn) => {
      const bad = needPawn(pawn); if (bad) return bad;
      pawn!.health = 100;
      return `health is now ${pawn!.health}`;
    },
  },
  {
    id: "armor",
    label: "Give full armor",
    run: (_slot, pawn) => {
      const bad = needPawn(pawn); if (bad) return bad;
      pawn!.armorValue = 100;
      return `armor is now ${pawn!.armorValue}`;
    },
  },
  {
    id: "noclip",
    label: "Toggle noclip",
    run: (_slot, pawn) => {
      const bad = needPawn(pawn); if (bad) return bad;
      const now = pawn!.moveType;
      pawn!.moveType = now === 7 ? 2 : 7;
      return `movetype ${now} -> ${pawn!.moveType}`;
    },
  },
  {
    id: "ak",
    label: "Give AK-47",
    run: (_slot, pawn) => {
      const bad = needPawn(pawn); if (bad) return bad;
      const w = pawn!.giveNamedItem(CsItem.AK47);
      return w ? `gave AK-47 (entity ${w.ref.index})` : "giveNamedItem failed";
    },
  },
  {
    id: "awp",
    label: "Give AWP",
    run: (_slot, pawn) => {
      const bad = needPawn(pawn); if (bad) return bad;
      const w = pawn!.giveNamedItem(CsItem.AWP);
      return w ? `gave AWP (entity ${w.ref.index})` : "giveNamedItem failed";
    },
  },
  {
    id: "strip",
    label: "Strip weapons",
    run: (_slot, pawn) => {
      const bad = needPawn(pawn); if (bad) return bad;
      return `stripWeapons -> ${pawn!.stripWeapons()}`;
    },
  },
  {
    id: "tp",
    label: "Teleport to where I'm aiming",
    run: (_slot, pawn) => {
      const bad = needPawn(pawn); if (bad) return bad;
      const hit = pawn!.aimTrace();
      if (!hit?.endPos) return "aim trace hit nothing";
      const pos = hit.endPos;
      // Lift a little so the teleport doesn't land inside the surface that was hit.
      const ok = pawn!.ref.teleport([pos.x, pos.y, pos.z + 8], null, [0, 0, 0]);
      return ok ? `teleported to ${pos.x.toFixed(0)},${pos.y.toFixed(0)},${pos.z.toFixed(0)}` : "teleport refused";
    },
  },
  {
    id: "cmd_hostname",
    label: "Run a server command (hostname)",
    run: () => {
      // Proof that a selection can drive the server console. Queued; runs next frame.
      Server.command('say "[panel] a button just ran a server console command"');
      return "queued: say ... (Server.command)";
    },
  },
  {
    id: "announce",
    label: "Announce to everyone",
    run: (slot) => {
      const name = Player.fromSlot(slot)?.playerName ?? `slot ${slot}`;
      Chat.toAll(`${ChatColors.Yellow}[panel]${ChatColors.Default} ${name} pressed the announce button`);
      return "announced to all chat";
    },
  },
];

/**
 * Open the panel for `slot`.
 *
 * `hud` is optional so the panel can offer a HUD toggle without this module owning one; passing it
 * in keeps `DemoHud` a single instance shared with the commands.
 */
export function open(slot: number, hud?: DemoHud, style: MenuStyle = MenuStyle.Center): void {
  const m = new Menu("s2script Action Panel");
  m.style = style;
  // Freeze while it's open: W/S drive the cursor, so a player who is also walking gets both.
  m.freezePlayer = style === MenuStyle.Center;
  m.exitButton = true;

  for (const b of BUTTONS) m.addItem(b.id, b.label);
  if (hud) m.addItem("hud", hud.has(slot) ? "Turn demo HUD OFF" : "Turn demo HUD ON");

  m.onSelect((e) => {
    const pawn = Pawn.forSlot(e.slot);

    if (e.info === "hud" && hud) {
      const on = !hud.has(e.slot);
      if (on) hud.show(e.slot); else hud.hide(e.slot);
      echo(e.slot, e.display, e.info, `demo HUD ${on ? "ON" : "off"}`);
      reopen(e.slot, hud, style);
      return;
    }

    const button = BUTTONS.find((b) => b.id === e.info);
    if (!button) { echo(e.slot, e.display, e.info, "no handler for this id (bug)"); return; }
    let result: string;
    try {
      result = button.run(e.slot, pawn);
    } catch (err) {
      // A throwing handler must not take the panel down with it — report and keep going.
      result = `handler threw: ${String(err)}`;
    }
    echo(e.slot, e.display, e.info, result);
    reopen(e.slot, hud, style);
  });

  m.onCancel((e) => {
    // Only a real Exit/timeout is worth reporting. NewMenu fires every time we reopen below, and
    // saying "closed" on each selection would be noise.
    if (e.reason === 3) return;   // MenuCancelReason.NewMenu — const enum, not indexable
    Chat.toSlot(e.slot, `${ChatColors.Green}[panel]${ChatColors.Default} closed (reason ${e.reason})`);
  });

  m.display(slot, PANEL_SECONDS);
}

/**
 * Re-open after a selection so the panel behaves like a persistent button strip rather than a
 * one-shot prompt. Displaying supersedes the old session (a NewMenu cancel), which `onCancel`
 * deliberately ignores.
 */
function reopen(slot: number, hud: DemoHud | undefined, style: MenuStyle): void {
  open(slot, hud, style);
}

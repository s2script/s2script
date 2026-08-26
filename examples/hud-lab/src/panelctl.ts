/**
 * Showing, hiding and reacting to a custom_hud_layout — the working flow.
 *
 * This mirrors Valve's own reference script for the zoo map's welcome dialog
 * (`maps/editor/zoo/scripts/setup.vjs` in pak01), which is the only complete worked example of the
 * API that exists:
 *
 *     function ShowWelcome(playerSlot) {
 *         GetWelcomeLayout().SetHasClassForPlayer(playerSlot, "dialog", "Dismissed", false);
 *         GetWelcomeLayout().SetInputCaptureEnabled(playerSlot, true);
 *     }
 *     Instance.OnCustomHudClicked((event) => {
 *         if (event.buttonId === "dismiss_button") HideWelcome(...);
 *     });
 *
 * THE THING THAT IS NOT OBVIOUS: a panel does not render for a player until that player has
 * PER-PLAYER STATE on the layout. The stylesheet's default (`#dialog { opacity: 1 }`) is not enough
 * — Valve still pushes an explicit "you do NOT have the hide class" entry on activate, and that
 * push is what makes the panel appear at all. An entity with no per-player entries shows nothing,
 * no matter how correct everything else is.
 *
 * So `show()` is two calls, not one, and neither is optional.
 */
import type { EntityRef } from "@s2script/sdk/entity";
import { setPlayerInputCapture } from "./hudstate";
import * as tierb from "./tierb";
import { HudPanelClassStatus } from "./offsets";

/** Which panel/class/button a layout uses. Defaults match the zoo welcome dialog, which every
 *  client already has — override per layout via the plugin config. */
export interface PanelSpec {
  /** The panel id the layout declares (`#dialog` in the zoo stylesheet). */
  panelId: string;
  /** The class that HIDES it (`#dialog.Dismissed { opacity: 0 }`). Applying it hides; removing shows. */
  hideClass: string;
  /** The button id the layout's click target carries. */
  buttonId: string;
}

export const ZOO_WELCOME: PanelSpec = {
  panelId: "dialog",
  hideClass: "Dismissed",
  buttonId: "dismiss_button",
};

/**
 * Our own kit, published as workshop addon 3790153369.
 *
 * `layout` is the value for the entity's `layout` keyvalue and MUST use the `.xml` source
 * extension — a client rejects `.vxml`/`.vxml_c` outright with
 * `Layout xml is an invalid resource name`.
 *
 * The panel ids and class names here are the contract with
 * examples/hud-lab/workshop/panorama/layout/custom_game/s2script_hud.xml. Rename one there and this
 * silently stops matching, which is exactly the drift a generated descriptor would prevent — see
 * the note at the bottom of the workshop README.
 */
export const S2_KIT: PanelSpec & { layout: string } = {
  layout: "panorama/layout/custom_game/s2script_kit.xml",
  panelId: "s2_dialog",
  hideClass: "s2-hidden",
  buttonId: "s2_btn_0",
};

/** The literal-text probe from the same addon — renders with no script involvement at all. */
export const S2_PROBE: PanelSpec & { layout: string } = {
  layout: "panorama/layout/custom_game/s2script_hud.xml",
  panelId: "dialog",
  hideClass: "s2-hidden",
  buttonId: "s2_btn_0",
};

/** Show the panel for one player: clear the hide class, then give them the cursor. */
export function show(
  calls: tierb.ResolvedCalls, layout: EntityRef, slot: number, spec: PanelSpec,
): string | null {
  const err = tierb.setHasClass(
    calls, layout, slot, spec.panelId, spec.hideClass, HudPanelClassStatus.DoesNotHaveClass,
  );
  if (err) return err;
  // Per-player, NOT the global flag on the embedded state — that one does not gate a player's
  // mouse, which cost a long time to work out.
  return setPlayerInputCapture(layout, slot, true);
}

/** Hide it again and take the cursor back. */
export function hide(
  calls: tierb.ResolvedCalls, layout: EntityRef, slot: number, spec: PanelSpec,
): string | null {
  const err = tierb.setHasClass(
    calls, layout, slot, spec.panelId, spec.hideClass, HudPanelClassStatus.HasClass,
  );
  if (err) return err;
  return setPlayerInputCapture(layout, slot, false);
}

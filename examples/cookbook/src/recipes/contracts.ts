import { tryUse, command, HookResult } from "@s2script/sdk";
import type { WorkshopService } from "@s2script/sdk/contracts/workshop";
import type { EconService } from "@s2script/cs2/econ";

/**
 * Standard interface contracts — consuming a capability the framework deliberately does NOT ship.
 *
 * Skins (`@s2script/cs2/econ`) and workshop/UGC (`@s2script/sdk/contracts/workshop`) are Valve-backend
 * and game-content concerns, not engine touchpoints, so s2script publishes only the *contract* and a
 * community plugin implements it. This recipe is the consumer side: `tryUse` returns `null` when
 * nothing has published the interface, which is the normal state on a stock server.
 *
 * Note the import is `import type` — there is no runtime module behind either contract.
 */
export const name = "contracts";
export const describe = "consume optional community interfaces: sm_econ, sm_workshop";

export function OnPluginStart(): void {
  // Optional deps: null while unpublished, so every call site stays guarded. A HARD dep
  // (use()) would instead hand back a proxy that throws once the producer unloads.
  const econ = tryUse<EconService>("econ");
  const workshop = tryUse<WorkshopService>("workshop");

  console.log(`[cookbook] contracts: econ=${econ ? "available" : "not installed"} ` +
    `workshop=${workshop ? "available" : "not installed"}`);

  command("sm_econ", (cmd) => {
    if (!econ) {
      cmd.reply("[cookbook] contracts: no econ plugin published — install one that implements " +
        "@s2script/cs2's EconService, or see packages/sdk/contracts/README.md");
      return HookResult.Handled;
    }
    const slot = cmd.argInt(0, -1);
    const loadout = econ.getLoadout(slot);
    cmd.reply(loadout
      ? `[cookbook] contracts: slot ${slot} loadout = ${JSON.stringify(loadout)}`
      : `[cookbook] contracts: slot ${slot} has no stored loadout`);
    return HookResult.Handled;
  });

  command("sm_workshop", (cmd) => {
    if (!workshop) {
      cmd.reply("[cookbook] contracts: no workshop plugin published — see " +
        "packages/sdk/contracts/README.md");
      return HookResult.Handled;
    }
    // Async by contract: nothing that talks to Steam may block the game frame.
    workshop.currentMap().then((map) => {
      console.log(map
        ? `[cookbook] contracts: workshop map ${map.title} (id ${map.id}, bsp ${map.mapName})`
        : "[cookbook] contracts: not running a workshop map");
    });
    cmd.reply("[cookbook] contracts: querying workshop map — see the console");
    return HookResult.Handled;
  });
}

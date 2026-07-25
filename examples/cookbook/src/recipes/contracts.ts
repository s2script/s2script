import type { Recipe } from "../recipe.ts";
import type { WorkshopService } from "@s2script/sdk/contracts/workshop";
import type { EconService } from "@s2script/cs2/econ";

/**
 * Standard interface contracts — consuming a capability the framework deliberately does NOT ship.
 *
 * Skins (`@s2script/cs2/econ`) and workshop/UGC (`@s2script/sdk/contracts/workshop`) are Valve-backend
 * and game-content concerns, not engine touchpoints, so s2script publishes only the *contract* and a
 * community plugin implements it. This recipe is the consumer side: `ctx.tryUse` returns `null` when
 * nothing has published the interface, which is the normal state on a stock server.
 *
 * Note the import is `import type` — there is no runtime module behind either contract.
 */
export const contractsRecipe: Recipe = {
  name: "contracts",
  describe: "consume optional community interfaces: sm_econ, sm_workshop",

  register(ctx) {
    // Optional deps: null while unpublished, so every call site stays guarded. A HARD dep
    // (ctx.use) would instead hand back a proxy that throws once the producer unloads.
    const econ = ctx.tryUse<EconService>("econ");
    const workshop = ctx.tryUse<WorkshopService>("workshop");

    console.log(`[cookbook] contracts: econ=${econ ? "available" : "not installed"} ` +
      `workshop=${workshop ? "available" : "not installed"}`);

    ctx.commands.register("sm_econ", (cmd) => {
      if (!econ) {
        cmd.reply("[cookbook] contracts: no econ plugin published — install one that implements " +
          "@s2script/cs2's EconService, or see packages/sdk/contracts/README.md");
        return;
      }
      const slot = cmd.argInt(0, -1);
      const loadout = econ.getLoadout(slot);
      cmd.reply(loadout
        ? `[cookbook] contracts: slot ${slot} loadout = ${JSON.stringify(loadout)}`
        : `[cookbook] contracts: slot ${slot} has no stored loadout`);
    });

    ctx.commands.register("sm_workshop", (cmd) => {
      if (!workshop) {
        cmd.reply("[cookbook] contracts: no workshop plugin published — see " +
          "packages/sdk/contracts/README.md");
        return;
      }
      // Async by contract: nothing that talks to Steam may block the game frame.
      workshop.currentMap().then((map) => {
        console.log(map
          ? `[cookbook] contracts: workshop map ${map.title} (id ${map.id}, bsp ${map.mapName})`
          : "[cookbook] contracts: not running a workshop map");
      });
      cmd.reply("[cookbook] contracts: querying workshop map — see the console");
    });
  },
};

import { config } from "@s2script/config";

// Slice 5E.2 live gate — plugin config materialization.
//  DECLARE: package.json s2script.config declares greeting(string)/maxUses(int)/enabled(bool) + defaults.
//  MATERIALIZE + AUTO-GENERATE: on first load the host writes addons/s2script/configs/_demo_hello.json
//   with the declared defaults (JSONC, //-commented), then materializes defaults + that file.
//  READ: config.getString/getInt/getBool return the materialized values (typed, degrade-safe).
//  LIVE-RELOAD (opt-in): registering config.onChange makes the loader watch the file — editing it
//   re-materializes and fires the handler WITHOUT a plugin reload (a plugin that never calls onChange
//   is read-only, unwatched). See CLAUDE/README.
export function onLoad(): void {
  console.log("[demo] onLoad — greeting=" + config.getString("greeting")
    + " maxUses=" + config.getInt("maxUses") + " enabled=" + config.getBool("enabled"));

  config.onChange((cfg) => {
    console.log("[demo] config changed — greeting=" + String(cfg.greeting)
      + " maxUses=" + String(cfg.maxUses) + " enabled=" + String(cfg.enabled));
  });
}

export function onUnload(): void { console.log("[demo] onUnload"); }

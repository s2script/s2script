// monorepo-plugin — one plugin split across npm workspace packages.
//
// Use this shape when a plugin outgrows a single src/ directory. The whole
// tree bundles into ONE .s2sp: sibling packages are INLINED at build time,
// not resolved at runtime.
//
// Not to be confused with cross-plugin interfaces (see greeter-plugin):
//   - workspace packages = a BUILD-TIME factoring of one plugin
//   - published interfaces = a RUNTIME contract between two separate plugins
// If two parts must load, unload, and version independently, they are two
// plugins, not two packages.
import { plugin } from "@s2script/sdk/plugin";
import { GreetingLog } from "@monorepo-example/core";
import { registerCommands } from "@monorepo-example/commands";

export default plugin((ctx) => {
  const log = new GreetingLog();
  registerCommands(ctx, log);
  console.log("[monorepo] loaded — try sm_greet and sm_latest");
});

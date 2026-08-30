// Producer: publishes @monorepo-example/producer@1.0.0 to this workspace.
//
// Bundles @monorepo-example/shared — a workspace LIBRARY, not a plugin (it has no
// s2script.publishes, so nothing can use() it). esbuild inlines it into this .s2sp at build
// time, same as examples/monorepo-plugin used to inline packages/core into its one plugin.
import { Tally, shout } from "@monorepo-example/shared";
import type { Greeter } from "../api";
import { publish } from "@s2script/sdk";

export function OnPluginStart(): void {
  // This plugin's OWN copy of Tally — bundling is a build-time COPY, not a runtime share.
  // plugins/consumer bundles the same source and counts on its OWN independent Tally.
  const tally = new Tally();

  // Typed against the contract: tsc fails the build if this drifts from api.d.ts.
  const impl: Greeter = {
    greet(name: string): string {
      tally.bump();
      return shout(name);
    },
    greetCount(): number {
      return tally.count;
    },
  };

  publish("@monorepo-example/producer", impl);
  console.log("[producer] loaded — publishing @monorepo-example/producer");
}

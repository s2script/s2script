// Consumer: hard-deps @monorepo-example/producer, a SIBLING plugin in this same repo.
//
// `import type { Greeter } from "@monorepo-example/producer"` resolves through the npm workspace
// symlink at node_modules/@monorepo-example/producer -> ../plugins/producer, straight to the
// producer's OWN api.d.ts on disk. THERE IS NO .s2script/types/@monorepo-example/producer/ COPY
// ANYWHERE IN THIS PLUGIN — that absence is the entire point of this example.
//
// Contrast examples/greeter-consumer, which keeps exactly that kind of hand-maintained copy at
// .s2script/types/@demo/greeter/index.d.ts: its producer, examples/greeter-plugin, is a registry
// dependency, not a workspace sibling, so there is no file on disk to resolve in place. Here there
// is, so `s2s build` typechecks against the producer's real contract and hashes THOSE SAME bytes
// into manifest.compiledAgainst — equal to the producer's own typesSha256 by construction, not by
// a copy kept in sync by hand.
import type { Greeter } from "@monorepo-example/producer";
import { Tally, shout } from "@monorepo-example/shared";
import { command } from "@s2script/sdk/commands";
import { use } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  // ctx.use returns a proxy that throws InterfaceUnavailable while the producer is unloaded, so a
  // producer reload degrades this command instead of crashing the plugin.
  const producer = use<Greeter>("@monorepo-example/producer");

  // This plugin's OWN Tally, from the SAME @monorepo-example/shared source the producer bundles.
  // Each build inlines its own copy — see packages/shared/src/index.ts — so this counter and the
  // producer's are two independent instances, never the same object across the plugin boundary.
  const asked = new Tally();

  command("sm_greet", (cmd) => {
    asked.bump();
    const name = cmd.arg(0) || "world";
    try {
      const message = producer.greet(name);
      cmd.reply(
        `${message} (producer has greeted ${producer.greetCount()}x; this plugin has asked ${asked.count}x)`,
      );
    } catch (e) {
      cmd.reply(`producer unavailable: ${String(e)}`);
    }
  });

  // shout() is called here too, straight from this plugin's own bundled copy of @monorepo-example/
  // shared — proof it never touches the producer's copy of the same source.
  console.log(`[consumer] loaded — try sm_greet (${shout("consumer")})`);
}

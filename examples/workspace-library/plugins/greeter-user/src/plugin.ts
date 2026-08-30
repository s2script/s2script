// Consumes @ws-example/greeter (../../libs/greeter) via s2script.libraries — the WORKSPACE-SIBLING
// resolution path (libraries.ts's resolveLibrarySibling), not the vendored `.s2script/libs/` copy
// examples/library-consumer demonstrates. Because the library sits right here in this same
// workspace, `s2s build` resolves `@ws-example/greeter` straight to `../../libs/greeter/src/index.ts`
// on disk — no `s2s add`, no vendored copy, no lockfile. Re-run `s2s build` after editing
// libs/greeter/src/index.ts and the change is picked up immediately, exactly like editing this
// file's own source.
import { greet } from "@ws-example/greeter";
import { command, HookResult } from "@s2script/sdk";

export function OnPluginStart(): void {
  command("sm_greet", (cmd) => {
    const name = cmd.argsFrom(0) || "world";
    cmd.reply(greet(name));
    return HookResult.Handled;
  });
}

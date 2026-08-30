// Two declared deps, neither a workspace sibling: one WITH a verified copy (real types, hashed
// into compiledAgainst) and one WITHOUT (the ambient `any` stub, absent from compiledAgainst).
// Together they are §11's compatibility hinge — this plugin must build exactly as it did before
// workspaces existed.
import type { Copied } from "@demo/copied";
import { anything } from "@demo/stubbed";
import { command } from "@s2script/sdk/commands";
import { use } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  const c = use<Copied>("@demo/copied");
  const s = use<{ go(): void }>("@demo/stubbed");
  command("nonws", (cmd) => {
    cmd.reply(String(c.ping(1) && anything !== undefined));
    s.go();
  });
}

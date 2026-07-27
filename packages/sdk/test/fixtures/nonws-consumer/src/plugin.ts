import { plugin } from "@s2script/sdk/plugin";
// Two declared deps, neither a workspace sibling: one WITH a verified copy (real types, hashed
// into compiledAgainst) and one WITHOUT (the ambient `any` stub, absent from compiledAgainst).
// Together they are §11's compatibility hinge — this plugin must build exactly as it did before
// workspaces existed.
import type { Copied } from "@demo/copied";
import { anything } from "@demo/stubbed";

export default plugin((ctx) => {
  const c = ctx.use<Copied>("@demo/copied");
  const s = ctx.use<{ go(): void }>("@demo/stubbed");
  ctx.commands.register("nonws", (cmd) => {
    cmd.reply(String(c.ping(1) && anything !== undefined));
    s.go();
  });
});

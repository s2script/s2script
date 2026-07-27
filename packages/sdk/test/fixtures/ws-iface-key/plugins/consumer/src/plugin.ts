import { plugin } from "@s2script/sdk/plugin";
// Named by the PACKAGE name of a map-form publisher: an ordinary REGISTRY dependency, so the
// verified copy under .s2script/types stays authoritative (§5.3.0, §11's compatibility hinge).
import type { Api as Mce } from "@fixture/ik-mce";
// Named by the interface that sibling actually PUBLISHES: resolved in place from the producer's
// own api.d.ts, with no copy anywhere.
import type { Api as Mapchooser } from "@fixture/ik-mapchooser";

export default plugin((ctx) => {
  const mce = ctx.use<Mce>("@fixture/ik-mce");
  const chooser = ctx.use<Mapchooser>("@fixture/ik-mapchooser");
  ctx.commands.register("ik_nominate", (cmd) => {
    mce.nominate("de_dust2");
    chooser.nominate("de_nuke");
    cmd.reply("nominated");
  });
});

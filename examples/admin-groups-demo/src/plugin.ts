// admin-groups-demo — logs the resolved admin model so the live gate can verify group/immunity/override
// resolution at the DATA level (bots read SteamID "0" -> never admins, so real targeting is a human test).
import { plugin } from "@s2script/sdk/plugin";
import { Admin } from "@s2script/sdk/admin";

// A synthetic SteamID64 the operator seeds into admins.json + admin_groups.json for the gate.
const SEED = "76561199000000001";

export default plugin((ctx) => {
  const a = Admin.get(SEED);
  console.log(`[admin-groups-demo] Admin.get(seed) = ${a ? `flags=${a.flags} immunity=${a.immunity} groups=[${a.groups.join(",")}]` : "null"}`);
  const g = Admin.getGroup("Full Admins");
  console.log(`[admin-groups-demo] getGroup('Full Admins') = ${g ? `flags=${g.flags} immunity=${g.immunity}` : "null"}`);
  // override lookups via the global native (proves admin_overrides.json + per-group overrides loaded)
  const ov = (globalThis as any).__s2_admin_override;
  if (typeof ov === "function") {
    console.log(`[admin-groups-demo] override sm_slap(global) = "${ov("", "sm_slap")}"`);
    console.log(`[admin-groups-demo] override sm_kick(seed)  = "${ov(SEED, "sm_kick")}"`);
  }
  ctx.commands.register("sm_admingroups_dump", (cmd) => {
    const x = Admin.get(SEED);
    cmd.reply(x ? `seed flags=${x.flags} imm=${x.immunity} groups=${x.groups.join(",")}` : "seed not an admin");
  });
});

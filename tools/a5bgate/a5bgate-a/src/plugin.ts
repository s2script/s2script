// a5bgate-a — live-gate fixture A for A5b (the eight CS2 ops retired into gamedata descriptors).
// NOT a shipped plugin. Pair with a5bgate-b, which is the SECOND plugin context: the cross-plugin
// checks (G4/G5) exist because the shim's dedupe was a host-global static and the JS replacement
// is per-plugin-context, which is the open question the review left for a human.
//
// Every line is prefixed [A5B-A] so `docker logs | grep A5B` reads as a transcript.
import { plugin } from "@s2script/sdk/plugin";
import { Events } from "@s2script/sdk/events";
import { Player, GameRules } from "@s2script/cs2";

export default plugin((ctx) => {
  const L = (m: string) => console.log(`[A5B-A] ${m}`);
  L("loaded");

  let frame = 0;
  ctx.server.onGameFrame(() => { frame += 1; });

  const liveVictim = () => Player.all().find((p) => (p.pawn?.health ?? 0) > 0);
  const anyPlayer = () => Player.all()[0];

  // ---------------------------------------------------------------- G2: per-op parity
  // Each op is a separate command so a failure names itself. Every one of these used to be a
  // hand-wired core op; they are now gamedata `calls` descriptors reached through pawn.js.
  ctx.commands.registerServer("a5b_slay", () => {
    const v = liveVictim();
    if (!v?.pawn) { L("slay: no live pawn"); return; }
    L(`slay: health=${v.pawn.health} -> slay()`);
    v.pawn.slay();
    L(`slay: returned; health now=${v.pawn?.health ?? "null"}`);
  });

  ctx.commands.registerServer("a5b_teams", () => {
    const p = anyPlayer();
    if (!p) { L("teams: no players"); return; }
    // switchTeam(1) must take the changeTeam route (team <= 1), switchTeam(2/3) the SwitchTeam one.
    L("teams: start");
    p.switchTeam(1);
    L("teams: switchTeam(1) returned [expect ChangeTeam route]");
    p.changeTeam(2);
    L("teams: changeTeam(2) returned");
    p.switchTeam(3);
    L("teams: switchTeam(3) returned [expect SwitchTeam route]");
  });

  ctx.commands.registerServer("a5b_items", () => {
    const v = liveVictim();
    if (!v?.pawn) { L("items: no live pawn"); return; }
    const w = v.pawn.giveNamedItem("weapon_ak47");
    L(`items: giveNamedItem(weapon_ak47) -> ${w ? "Weapon" : "null"} weapons=${v.pawn.weapons.length}`);
    if (w) L(`items: removeWeapon -> ${v.pawn.removeWeapon(w)} weapons=${v.pawn.weapons.length}`);
  });

  // ---------------------------------------------------------------- G3: dedupe + consume-before-call
  // Spec §9.2's two mandated checks. Both calls are in ONE dispatch, so the shim's old drain would
  // have collapsed them to one Respawn; the JS wrapper must do the same within this context.
  ctx.commands.registerServer("a5b_respawn2", () => {
    const dead = Player.all().find((p) => (p.pawn?.health ?? 1) <= 0) ?? anyPlayer();
    if (!dead) { L("respawn2: no player"); return; }
    const r1 = dead.respawn();
    const r2 = dead.respawn();
    // Both must report true — the deduped SECOND call returns true, not false (shim parity).
    L(`respawn2: first=${r1} second=${r2} (both true = deduped, not refused) frame=${frame}`);
  });

  // ---------------------------------------------------------------- G4/G5: the cross-plugin question
  // A and B both act on the SAME target in the SAME frame. The shim deduped host-globally; the JS
  // wrappers dedupe per context. b counts the resulting dispatches — 1 means something still
  // collapses them, 2 means the cross-plugin dedupe is genuinely gone.
  ctx.commands.registerServer("a5b_dupe_respawn", () => {
    const dead = Player.all().find((p) => (p.pawn?.health ?? 1) <= 0) ?? anyPlayer();
    if (!dead) { L("dupe_respawn: no player"); return; }
    L(`dupe_respawn: slot=${dead.slot} frame=${frame} — a and b both respawn it`);
    Events.fire("player_changename", { userid: 0, oldname: "a5b-dupe-respawn", newname: `${dead.slot}` });
    L(`dupe_respawn: a's own call -> ${dead.respawn()}`);
  });

  ctx.commands.registerServer("a5b_dupe_round", () => {
    L(`dupe_round: frame=${frame} — a and b both terminateRound(8)`);
    Events.fire("player_changename", { userid: 0, oldname: "a5b-dupe-round", newname: "0" });
    L(`dupe_round: a's own call -> ${GameRules.terminateRound(8)}`);
  });

  ctx.commands.registerServer("a5b_round", () => {
    L(`round: terminateRound(8) -> ${GameRules.terminateRound(8)} frame=${frame}`);
  });

  return { onUnload() { L("unloading"); } };
});

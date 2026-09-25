// Live-gate fixture for a gamedata refresh: exercises every re-derived signature a bot server can
// reach. Not shipped. Prefix [GDPROBE]. Drive with gd_hooks, gd_calls, gd_swap, then gd_report.
import { command, onOutput, Entity, SDKHook, SDKHookType, HookResult } from "@s2script/sdk";
import { Player, GameRules, CustomCameraMode, gameRules, TriggerZone } from "@s2script/cs2";
import { after } from "@s2script/sdk/timers";

const n = { postThink: 0, startTouch: 0, endTouch: 0, outStart: 0, outEnd: 0, terminateHook: 0, zStart: 0, zEnd: 0, zOutStart: 0, zOutEnd: 0 };

export function OnPluginStart(): void {
  const L = (m: string) => console.log(`[GDPROBE] ${m}`);
  L("loaded");
  // FireOutputInternal + onTerminateRound: registered in the load window.
  onOutput("func_buyzone", "OnStartTouch", () => { n.outStart += 1; });
  onOutput("func_buyzone", "OnEndTouch", () => { n.outEnd += 1; });
  onOutput("trigger_multiple", "OnStartTouch", () => { n.zOutStart += 1; });
  onOutput("trigger_multiple", "OnEndTouch", () => { n.zOutEnd += 1; });
  gameRules.onTerminateRound((v) => { n.terminateHook += 1; L(`onTerminateRound reason=${v.reason} delay=${v.delay}`); return HookResult.Continue; });

  command.server("gd_hooks", () => {
    let pawns = 0, zones = 0;
    for (const p of Player.all()) {
      if (p.pawn && SDKHook(p.pawn.ref, SDKHookType.PostThink, () => { n.postThink += 1; })) pawns += 1;
    }
    for (const z of Entity.findByClass("func_buyzone")) {
      const a = SDKHook(z, SDKHookType.StartTouch, () => { n.startTouch += 1; });
      const b = SDKHook(z, SDKHookType.EndTouch, () => { n.endTouch += 1; });
      if (a && b) zones += 1;
    }
    L(`hooks postThink pawns=${pawns} buyzones(start+end)=${zones}`);
  });

  command.server("gd_calls", () => {
    const p = Player.all().find((x) => x.pawn);
    const pawn = p?.pawn;
    if (!p || !pawn) { L("calls: no pawn"); return; }
    L(`setModelScale=${pawn.setModelScale(1.25)} back=${pawn.setModelScale(1)}`);
    L(`setBodyGroupByName=${pawn.ref.setBodyGroupByName("body", 0)}`);
    const cam = pawn.getCustomCamera();
    L(`getCustomCamera=${cam !== null}` + (cam
      ? ` setMode=${cam.setMode(CustomCameraMode.CONTROLLED)} mode=${cam.getMode()}` +
        ` follow=${cam.setFollowConfig({ followEntity: pawn.ref })} reset=${cam.setMode(0 as never)} mode=${cam.getMode()}`
      : ""));
    const team = p.teamNum;
    p.spectate();
    const spec = p.teamNum;
    if (team !== null && team > 1) p.changeTeam(team);
    L(`changeTeam team=${team} afterSpectate=${spec} restored=${p.teamNum}`);
    L(`terminateRound=${GameRules.terminateRound(9, 1)}`);
  });

  // Force an end-touch: move one bot from its own buy zone into an opposing bot's spawn.
  command.server("gd_swap", () => {
    const all = Player.all().filter((x) => x.pawn?.origin);
    const a = all[0];
    const b = all.find((x) => x.teamNum !== a?.teamNum);
    const to = b?.pawn?.origin;
    if (!a?.pawn || !to) { L("swap: need bots on both teams"); return; }
    L(`swap slot=${a.slot} -> near slot=${b!.slot} ok=${a.pawn.ref.teleport([to.x + 40, to.y, to.z + 8], null, [0, 0, 0])}`);
  });

  // A runtime trigger (CollisionUpdatePartition) around a bot, then move the bot out, in, out.
  command.server("gd_zone", () => {
    const p = Player.all().find((x) => x.pawn?.origin);
    const pawn = p?.pawn; const o = pawn?.origin;
    if (!pawn || !o) { L("zone: no pawn"); return; }
    const z = TriggerZone.create({ x: o.x - 64, y: o.y - 64, z: o.z - 8 }, { x: o.x + 64, y: o.y + 64, z: o.z + 96 });
    if (!z) { L("zone: create failed"); return; }
    const a = SDKHook(z.ref, SDKHookType.StartTouch, () => { n.zStart += 1; });
    const b = SDKHook(z.ref, SDKHookType.EndTouch, () => { n.zEnd += 1; });
    const at = (dz: number) => pawn.ref.teleport([o.x, o.y, o.z + dz], null, [0, 0, 0]);
    const r = () => `z=${pawn.origin?.z.toFixed(0)} hp=${pawn.health} zoneLive=${z.ref.isValid()} hookStart=${n.zStart} hookEnd=${n.zEnd} OnStartTouch=${n.zOutStart} OnEndTouch=${n.zOutEnd}`;
    L(`zone slot=${p!.slot} hooks=${a}/${b} ${r()}`);
    after(500, () => { L(`  after create ${r()}`); at(600);
      after(700, () => { L(`  after out ${r()}`); at(8);
        after(700, () => { L(`  after in ${r()}`); at(600);
          after(700, () => { L(`  after out2 ${r()}`); z.remove(); }); }); }); });
  });

  command.server("gd_round", () => { L(`terminateRound=${GameRules.terminateRound(9, 1)}`); });

  command.server("gd_report", () => {
    L(`REPORT postThink=${n.postThink} startTouch=${n.startTouch} endTouch=${n.endTouch} OnStartTouch=${n.outStart} OnEndTouch=${n.outEnd} onTerminateRound=${n.terminateHook}`);
  });
}

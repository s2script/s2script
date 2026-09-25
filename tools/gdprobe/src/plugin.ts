// Live-gate fixture for a gamedata refresh: exercises every re-derived signature a bot server can
// reach. Not shipped. Prefix [GDPROBE]. Drive with gd_hooks, then gd_calls, then gd_report.
import { command, onOutput, Entity, SDKHook, SDKHookType, HookResult } from "@s2script/sdk";
import { Player, GameRules, CustomCameraMode, gameRules } from "@s2script/cs2";

const n = { postThink: 0, startTouch: 0, endTouch: 0, outputs: 0, terminateHook: 0 };

export function OnPluginStart(): void {
  const L = (m: string) => console.log(`[GDPROBE] ${m}`);
  L("loaded");
  // FireOutputInternal + onTerminateRound: registered in the load window.
  onOutput("func_buyzone", "OnStartTouch", () => { n.outputs += 1; });
  onOutput("func_buyzone", "OnEndTouch", () => { n.outputs += 1; });
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

  command.server("gd_report", () => {
    L(`REPORT postThink=${n.postThink} startTouch=${n.startTouch} endTouch=${n.endTouch} outputs=${n.outputs} onTerminateRound=${n.terminateHook}`);
  });
}

// Live-gate demo for the usermsg-hook slice — the four TTT consumer shapes:
//   BombPlantSuppressor: onPre RadioText -> Handled (blanket radio-text block, toggleable)
//   SilentAWP/PoisonShots/SuppressedRound: onPre TEFireBullets -> typed reads + conditional suppress
// D-09 probe: readInt("player") is the shooter as a packed fixed32 entity handle — logged raw AND
// bit-split both ways (14/15-bit index) so the live gate settles the packing + pawn-vs-controller
// questions before any TTT consumer ships on it. All message/field names are CS2 knowledge and live
// HERE, never in core/shim (vendored proto: third_party/hl2sdk/game/shared/cs/cs_gameevents.proto).
import { plugin } from "@s2script/sdk/plugin";
import { UserMessages } from "@s2script/sdk/usermessages";
import { HookResult, type HookResultValue } from "@s2script/sdk/events";

let blockRadio = false;   // BombPlantSuppressor shape (TTT blanket-blocks ALL radio text)
let blockShots = false;   // SuppressedRound shape

UserMessages.onPre("CCSUsrMsg_RadioText", (m): HookResultValue | void => {
  console.log("[usermsg-demo] RadioText id=" + m.id + " msg_name=" + m.readString("msg_name") +
              " client=" + m.readInt("client") + " recipients=[" + m.recipients.join(",") + "]");
  if (blockRadio) return HookResult.Handled;      // TTT: Recipients.Clear()+Handled -> just Handled
});

UserMessages.onPre("CMsgTEFireBullets", (m): HookResultValue | void => {
  const item = m.readInt("item_def_index");       // TTT's weapon filter (the only typed read it used)
  const player = m.readInt("player");             // D-09 fix: the shooter handle, top-level fixed32
  const ox = m.readFloat("origin.x");             // dotted nested read — the capability CSSharp lacks
  console.log("[usermsg-demo] FireBullets item_def_index=" + item + " player=" + player +
              " origin.x=" + ox +
              (player !== null && player !== 16777215
                 ? " idx14=" + (player & 0x3fff) + "/ser" + (player >>> 14) +
                   " idx15=" + (player & 0x7fff) + "/ser" + (player >>> 15)
                 : " (no shooter)"));
  if (blockShots) return HookResult.Handled;      // SilentAWP/SuppressedRound suppress path
});

export default plugin((ctx) => {
  ctx.commands.register("sm_umtest", (cmd) => {
    const what = cmd.arg(0), on = cmd.arg(1) !== "0";
    if (what === "radio") blockRadio = on;
    else if (what === "shots") blockShots = on;
    else { cmd.reply("[usermsg-demo] usage: sm_umtest <radio|shots> <0|1>"); return; }
    cmd.reply("[usermsg-demo] block " + what + " = " + on);
  });

  console.log("[usermsg-demo] onLoad — RadioText + TEFireBullets hooks armed; sm_umtest registered");
});

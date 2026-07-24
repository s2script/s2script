import type { Recipe } from "../recipe.ts";
import { UserMessages } from "@s2script/sdk/usermessages";
import { HookResult, type HookResultValue } from "@s2script/sdk/events";

/**
 * UserMessages.onPre intercepts an outbound user message before delivery:
 * typed field reads (dotted nested paths supported, e.g. "origin.x"), and an
 * optional HookResult.Handled to suppress the send for every recipient. All
 * message/field names here are CS2 knowledge (vendored proto:
 * third_party/hl2sdk/game/shared/cs/cs_gameevents.proto), never core/shim
 * knowledge.
 *
 * The "player" field on CMsgTEFireBullets is the shooter as a packed fixed32
 * entity handle — logged raw AND bit-split both ways (14/15-bit index) since
 * the packing (pawn vs. controller index) is worth seeing directly.
 */
export const usermessagesRecipe: Recipe = {
  name: "usermessages",
  describe: "intercept RadioText + FireBullets user messages, typed reads + suppress (sm_usermsg)",
  register(ctx) {
    let blockRadio = false; // blanket-blocks ALL radio text
    let blockShots = false;

    UserMessages.onPre("CCSUsrMsg_RadioText", (m): HookResultValue | void => {
      console.log("[cookbook] usermsg RadioText id=" + m.id + " msg_name=" + m.readString("msg_name") +
                  " client=" + m.readInt("client") + " recipients=[" + m.recipients.join(",") + "]");
      if (blockRadio) return HookResult.Handled;
    });

    UserMessages.onPre("CMsgTEFireBullets", (m): HookResultValue | void => {
      const item = m.readInt("item_def_index");
      const player = m.readInt("player");  // the shooter handle, top-level fixed32
      const ox = m.readFloat("origin.x");  // dotted nested read — the capability CSSharp lacks
      console.log("[cookbook] usermsg FireBullets item_def_index=" + item + " player=" + player +
                  " origin.x=" + ox +
                  (player !== null && player !== 16777215
                     ? " idx14=" + (player & 0x3fff) + "/ser" + (player >>> 14) +
                       " idx15=" + (player & 0x7fff) + "/ser" + (player >>> 15)
                     : " (no shooter)"));
      if (blockShots) return HookResult.Handled;
    });

    ctx.commands.register("sm_usermsg", (cmd) => {
      const what = cmd.arg(0), on = cmd.arg(1) !== "0";
      if (what === "radio") blockRadio = on;
      else if (what === "shots") blockShots = on;
      else { cmd.reply("[cookbook] usage: sm_usermsg <radio|shots> <0|1>"); return; }
      cmd.reply("[cookbook] usermsg: block " + what + " = " + on);
    });
  },
};

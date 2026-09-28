import { Engine } from "@s2script/sdk/unsafe";
import { Clients, command, HookResult } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

/** The command is intentionally the only place that invokes the lethal function. */
export function OnPluginStart(): void {
  const commitSuicide = Engine.function("commitSuicide");
  const staleEntryProbe = Engine.function("staleEntryProbe");
  console.log(`[engine-function-demo] commitSuicide ${JSON.stringify(commitSuicide.status)}`);
  console.log(`[engine-function-demo] staleEntryProbe ${JSON.stringify(staleEntryProbe.status)}`);

  if (staleEntryProbe.available) {
    console.error("[engine-function-demo] staleEntryProbe unexpectedly passed its entry validator");
  }

  if (commitSuicide.available) {
    const first = commitSuicide.onPre(view => {
      view.force = true;
      console.log(`[engine-function-demo] PRE explode=${view.explode} force=${view.force}`);
      return HookResult.Changed;
    });
    console.log(`[engine-function-demo] first PRE ${first.status}: ${first.reason ?? "ok"}`);
    const disposed = first.dispose();
    console.log(`[engine-function-demo] first PRE disposed=${disposed}; status=${first.status}`);
    const replacement = commitSuicide.onPre(view => {
      view.force = true;
      console.log(`[engine-function-demo] PRE explode=${view.explode} force=${view.force}`);
      return HookResult.Changed;
    });
    console.log(`[engine-function-demo] replacement PRE ${replacement.status}: ${replacement.reason ?? "ok"}`);
    const post = commitSuicide.onPost(view => {
      console.log(`[engine-function-demo] POST explode=${view.explode} force=${view.force}`);
    });
    console.log(`[engine-function-demo] POST ${post.status}: ${post.reason ?? "ok"}`);
    console.log(`[engine-function-demo] binding after subscriptions ${JSON.stringify(commitSuicide.status)}`);
  }

  command("sm_ef_status", cmd => {
    cmd.reply(`[engine-function-demo] commitSuicide ${JSON.stringify(commitSuicide.status)}`);
    cmd.reply(`[engine-function-demo] staleEntryProbe ${JSON.stringify(staleEntryProbe.status)}`);
    return HookResult.Handled;
  });

  command("sm_ef_suicide_bot", cmd => {
    if (cmd.callerSlot !== -1) {
      cmd.reply("[engine-function-demo] server console only");
      return HookResult.Handled;
    }
    if (!commitSuicide.available) {
      cmd.reply(`[engine-function-demo] unavailable ${JSON.stringify(commitSuicide.status)}`);
      return HookResult.Handled;
    }
    const slotText = cmd.arg(0);
    if (!/^\d+$/.test(slotText)) {
      cmd.reply("[engine-function-demo] usage: sm_ef_suicide_bot <bot-slot>");
      return HookResult.Handled;
    }
    const slot = Number(slotText);
    const bot = Clients.fromSlot(slot);
    if (!bot?.isValid() || !bot.isBot || bot.signonState !== 6 || bot.userId < 0) {
      cmd.reply("[engine-function-demo] refused: slot needs a connected bot");
      return HookResult.Handled;
    }
    const player = Player.fromSlot(slot);
    const pawn = player?.pawn;
    if (!player || player.userId !== bot.userId ||
        !pawn?.isValid || pawn.health === null || pawn.health <= 0) {
      cmd.reply("[engine-function-demo] refused: slot needs an alive, fully spawned bot pawn");
      return HookResult.Handled;
    }
    const userId = bot.userId;
    const pawnId = pawn.ref.id;
    const beforeHealth = pawn.health;
    const beforeAlive = beforeHealth !== null && beforeHealth > 0;
    commitSuicide.call(pawn.ref, false, true);
    const stillSameBot = bot.isValid() && bot.isBot && bot.userId === userId;
    const afterPawn = stillSameBot ? Player.fromSlot(slot)?.pawn : null;
    const afterHealth = afterPawn?.health ?? null;
    const afterAlive = stillSameBot ? afterHealth !== null && afterHealth > 0 : null;
    const samePawn = afterPawn?.ref.id === pawnId;
    cmd.reply(`[engine-function-demo] bot slot=${slot} pawn=${pawnId} alive ${beforeAlive} -> ${afterAlive ?? "unknown"}; health ${beforeHealth} -> ${afterHealth}; sameBot=${stillSameBot}; samePawn=${samePawn}`);
    return HookResult.Handled;
  });
}

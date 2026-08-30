import type { Recipe } from "../recipe.ts";
import { WebSocket } from "@s2script/sdk/ws";
import { hook } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";

/**
 * WebSockets run off-thread on the shared tokio runtime; callbacks are
 * marshalled back onto a game frame. The frame counter proves the tick keeps
 * advancing while the socket connects and echoes.
 */
export const wsRecipe: Recipe = {
  name: "ws",
  describe: "connect a websocket without blocking the tick (sm_ws)",
  register() {
    let frames = 0;
    hook.gameFrame(() => { frames += 1; });

    command("sm_ws", (cmd) => {
      const start = frames;
      cmd.reply("connecting…");
      WebSocket.connect("wss://ws.postman-echo.com/raw")
        .then((ws) => {
          ws.onMessage((data) => {
            console.log(`[cookbook] echo=${data}; tick advanced ${frames - start} frames meanwhile`);
            ws.close();
          });
          ws.onClose((code, reason) => console.log(`[cookbook] ws closed code=${code} reason=${reason}`));
          ws.onError((e) => console.log(`[cookbook] ws error=${e}`));
          ws.send("hello-from-s2script");
          cmd.reply("connected + sent — watch the log for the echo");
        })
        .catch((e: unknown) => cmd.reply(`connect failed: ${String(e)}`));
    });
  },
};

// ws-demo — connects to a public WebSocket echo service, sends a message, logs the echoed reply, and
// logs the frame counter to prove the connection didn't block the tick.
import { plugin } from "@s2script/sdk/plugin";
import { WebSocket } from "@s2script/sdk/ws";

let frames = 0;

export default plugin(async (ctx) => {
  ctx.server.onGameFrame(() => { frames++; });
  const start = frames;
  try {
    const ws = await WebSocket.connect("wss://ws.postman-echo.com/raw");
    ws.onMessage((data) => {
      console.log("[ws-demo] echo=" + data + "; tick advanced " + (frames - start) + " frames while connecting/echoing");
      ws.close();
    });
    ws.onClose((code, reason) => console.log("[ws-demo] closed code=" + code + " reason=" + reason));
    ws.onError((e) => console.log("[ws-demo] error=" + e));
    ws.send("hello-from-s2script");
    console.log("[ws-demo] connected + sent (frames=" + frames + ")");
  } catch (e) {
    console.log("[ws-demo] connect ERROR: " + String(e));
  }

  return {
    onUnload(): void { console.log("[ws-demo] onUnload"); },
  };
});

import type { Recipe } from "../recipe.ts";
import { Net } from "@s2script/sdk/net";

const dec = (b: Uint8Array) => Array.from(b).map((c) => String.fromCharCode(c)).join("");

// A2S_INFO query: 0xFFFFFFFF 'T' "Source Engine Query\0"
const A2S_INFO = new Uint8Array([
  0xff, 0xff, 0xff, 0xff, 0x54,
  ...Array.from("Source Engine Query").map((c) => c.charCodeAt(0)),
  0x00,
]);

/**
 * @s2script/net proves a TCP round trip to a public HTTP server and a UDP A2S
 * query (challenge handshake) against this server itself. Both sockets run
 * off-thread on the shared tokio runtime; the frame counter proves the tick
 * keeps advancing while they're in flight.
 */
export const netRecipe: Recipe = {
  name: "net",
  describe: "TCP + UDP round trip without blocking the tick (sm_net)",
  register(ctx) {
    let frames = 0;
    ctx.server.onGameFrame(() => { frames += 1; });

    async function tcp(): Promise<void> {
      try {
        const before = frames;
        const s = await Net.connectTcp("example.com", 80);
        s.onData((b) => {
          const line = dec(b).split("\r\n")[0];
          console.log(`[cookbook] net TCP example.com:80 -> "${line}" frames+=${frames - before}`);
          s.close();
        });
        s.onError((e) => console.log(`[cookbook] net TCP error: ${e}`));
        s.send("GET / HTTP/1.0\r\nHost: example.com\r\n\r\n");
      } catch (e) { console.log(`[cookbook] net TCP connect failed: ${e}`); }
    }

    async function a2s(): Promise<void> {
      try {
        const before = frames;
        const u = await Net.udp();
        u.onMessage((_from, b) => {
          const header = b[4];
          if (header === 0x41) {
            // S2C_CHALLENGE — resend with the 4-byte challenge
            const q = new Uint8Array(A2S_INFO.length + 4);
            q.set(A2S_INFO, 0); q.set(b.slice(5, 9), A2S_INFO.length);
            u.sendTo("127.0.0.1", 27015, q);
          } else if (header === 0x49) {
            // A2S_INFO reply — name is the null-terminated string after byte 6
            let i = 6, name = "";
            while (i < b.length && b[i] !== 0) { name += String.fromCharCode(b[i]); i++; }
            console.log(`[cookbook] net UDP A2S self-query -> server="${name}" frames+=${frames - before}`);
            u.close();
          }
        });
        u.sendTo("127.0.0.1", 27015, A2S_INFO);
      } catch (e) { console.log(`[cookbook] net UDP failed: ${e}`); }
    }

    ctx.commands.register("sm_net", (cmd) => {
      cmd.reply("firing TCP + UDP round trips…");
      tcp();
      a2s();
    });
  },
};

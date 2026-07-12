// net-demo — proves @s2script/net end-to-end: a TCP round-trip to a public HTTP server + a UDP A2S
// query to our own CS2 server (challenge handshake), and that the game frame advances during both.
import { Net } from "@s2script/net";
import { OnGameFrame } from "@s2script/frame";

let frames = 0;
OnGameFrame.subscribe(() => { frames++; });
const dec = (b: Uint8Array) => Array.from(b).map((c) => String.fromCharCode(c)).join("");

async function tcp(): Promise<void> {
  try {
    const before = frames;
    const s = await Net.connectTcp("example.com", 80);
    s.onData((b) => {
      const line = dec(b).split("\r\n")[0];
      console.log(`[net-demo] TCP example.com:80 -> "${line}" frames+=${frames - before}`);
      s.close();
    });
    s.onError((e) => console.log(`[net-demo] TCP error: ${e}`));
    s.send("GET / HTTP/1.0\r\nHost: example.com\r\n\r\n");
  } catch (e) { console.log(`[net-demo] TCP connect failed: ${e}`); }
}

// A2S_INFO query: 0xFFFFFFFF 'T' "Source Engine Query\0"
const A2S_INFO = new Uint8Array([0xff,0xff,0xff,0xff,0x54, ...Array.from("Source Engine Query").map(c=>c.charCodeAt(0)), 0x00]);
async function a2s(): Promise<void> {
  try {
    const before = frames;
    const u = await Net.udp();
    u.onMessage((_from, b) => {
      const header = b[4];
      if (header === 0x41) {                     // S2C_CHALLENGE — resend with the 4-byte challenge
        const q = new Uint8Array(A2S_INFO.length + 4);
        q.set(A2S_INFO, 0); q.set(b.slice(5, 9), A2S_INFO.length);
        u.sendTo("127.0.0.1", 27015, q);
      } else if (header === 0x49) {              // A2S_INFO reply — name is the null-terminated string after byte 6
        let i = 6, name = "";
        while (i < b.length && b[i] !== 0) { name += String.fromCharCode(b[i]); i++; }
        console.log(`[net-demo] UDP A2S self-query -> server="${name}" frames+=${frames - before}`);
        u.close();
      }
    });
    u.sendTo("127.0.0.1", 27015, A2S_INFO);
  } catch (e) { console.log(`[net-demo] UDP failed: ${e}`); }
}

export function onLoad(): void {
  console.log("[net-demo] onLoad — TCP + UDP");
  tcp();
  a2s();
}

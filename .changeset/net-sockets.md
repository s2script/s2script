---
"@s2script/net": patch
---

New `@s2script/net` package: raw TCP + UDP client sockets (binary `Uint8Array`), off the game thread over the shared async runtime. `Net.connectTcp(host, port)` (send/onData/onClose/onError) and `Net.udp()` (sendTo/onMessage) — unblocks A2S server queries, IRC, custom protocols.

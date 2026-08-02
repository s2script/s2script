---
"@s2script/sdk": minor
---

Let `WebSocket.connect` send headers on the opening handshake

`connect(url)` could not carry a credential, which put a whole class of server out of reach: anything
that authenticates the **upgrade** rather than the first frame. There was no workaround from a
plugin — the SDK exposed no headers and no subprotocol, so the usual "smuggle the token as a
subprotocol" trick was unavailable too.

MAUL is the case that surfaced it. Its `/v2/ws` is header-only by deliberate design — the `?token=`
query fallback was removed because query strings leak into access logs, proxies and referrers — so a
CS2 plugin could not open an authenticated socket at all.

```ts
const ws = await WebSocket.connect("wss://maul.example/v2/ws", {
  headers: { Authorization: `Bearer ${jwt}` },
});
```

The parameter is optional, so every existing call is unchanged.

Headers the handshake owns are **refused**, not silently dropped: `Host`, `Connection`, `Upgrade`,
any `Sec-WebSocket-*`, `Content-Length`, `Transfer-Encoding`. Setting one of those does not produce a
customised connection — it produces a corrupt or spoofed one, and `Host` in particular is request
smuggling against whatever sits in front of the target. Values containing control characters are
refused for the same reason: a `\r\n` in a value is how a second header would be injected.

A refusal arrives as a rejected connect promise, on the same path a bad URL or a refused socket
takes, so a plugin that believes it authenticated never ends up holding an anonymous connection
instead.

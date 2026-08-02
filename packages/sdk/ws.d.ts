/** @s2script/ws — client WebSocket. NO runtime code (injected as __s2pkg_ws). */

/** An open WebSocket connection — a per-plugin handle over a copied message stream (no live socket crosses to JS). */
export interface WebSocket {
  /** Register a handler invoked for each inbound text frame. */
  onMessage(handler: (data: string) => void): void;
  /** Register a handler for connection close; `code`/`reason` come from the close frame. */
  onClose(handler: (code: number, reason: string) => void): void;
  /** Register a handler for a transport error; `err` is the error text. */
  onError(handler: (err: string) => void): void;
  /** Send a text frame. */
  send(data: string): void;
  /** Close the connection. */
  close(): void;
}
/** Options for {@link WebSocket.connect}. */
export interface WebSocketInit {
  /**
   * Extra headers to send on the opening handshake — the usual reason being a
   * credential the server requires, e.g. `Authorization: Bearer <token>`.
   *
   * Headers the handshake owns are refused, and the connect rejects rather than
   * silently dropping them: `Host`, `Connection`, `Upgrade`, any
   * `Sec-WebSocket-*`, `Content-Length`, `Transfer-Encoding`. Values containing
   * control characters are refused for the same reason.
   */
  headers?: Record<string, string>;
}

/** Entry point for opening WebSocket connections. */
export declare const WebSocket: {
  /**
   * Connect to a WebSocket server (`wss://` for TLS) off the game thread.
   * @returns Resolves on the open handshake with the live {@link WebSocket} handle.
   * @throws Rejects on connect failure (bad URL, refused, TLS/handshake error, or a refused header).
   * @example
   * import { WebSocket } from "@s2script/sdk/ws";
   * const ws = await WebSocket.connect("wss://ws.postman-echo.com/raw");
   * ws.onMessage((data) => { console.log("echo:", data); ws.close(); });
   * ws.send("hello-from-s2script");
   * @example
   * // A server that authenticates the handshake rather than the first frame.
   * const ws = await WebSocket.connect("wss://maul.example/v2/ws", {
   *   headers: { Authorization: `Bearer ${jwt}` },
   * });
   */
  connect(url: string, init?: WebSocketInit): Promise<WebSocket>;
};

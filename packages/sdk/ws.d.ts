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
/** Entry point for opening WebSocket connections. */
export declare const WebSocket: {
  /**
   * Connect to a WebSocket server (`wss://` for TLS) off the game thread.
   * @returns Resolves on the open handshake with the live {@link WebSocket} handle.
   * @throws Rejects on connect failure (bad URL, refused, TLS/handshake error).
   * @example
   * import { WebSocket } from "@s2script/sdk/ws";
   * const ws = await WebSocket.connect("wss://ws.postman-echo.com/raw");
   * ws.onMessage((data) => { console.log("echo:", data); ws.close(); });
   * ws.send("hello-from-s2script");
   */
  connect(url: string): Promise<WebSocket>;
};

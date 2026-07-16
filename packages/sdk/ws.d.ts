/** @s2script/ws — client WebSocket. NO runtime code (injected as __s2pkg_ws). */
export interface WebSocket {
  onMessage(handler: (data: string) => void): void;
  onClose(handler: (code: number, reason: string) => void): void;
  onError(handler: (err: string) => void): void;
  send(data: string): void;
  close(): void;
}
export declare const WebSocket: {
  /** Connect to a WebSocket server (wss:// for TLS). Resolves on the open handshake; rejects on connect failure. */
  connect(url: string): Promise<WebSocket>;
};

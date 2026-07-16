/** @s2script/net — engine-generic raw TCP + UDP sockets (binary), off the game thread. */
export interface TcpSocket {
  send(data: Uint8Array | string): void;
  onData(handler: (bytes: Uint8Array) => void): void;
  onClose(handler: () => void): void;
  onError(handler: (err: string) => void): void;
  close(): void;
}
export interface UdpSocket {
  sendTo(host: string, port: number, data: Uint8Array | string): void;
  onMessage(handler: (from: { host: string; port: number }, bytes: Uint8Array) => void): void;
  close(): void;
}
export declare const Net: {
  /** Connect a TCP client. Rejects on connect failure. */
  connectTcp(host: string, port: number): Promise<TcpSocket>;
  /** Bind a UDP socket on an ephemeral local port. */
  udp(): Promise<UdpSocket>;
};

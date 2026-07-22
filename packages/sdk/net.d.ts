/** @s2script/net — engine-generic raw TCP + UDP sockets (binary), off the game thread. */

/** A connected TCP client socket — a per-plugin handle over a copied byte stream (no live socket crosses to JS). */
export interface TcpSocket {
  /** Send bytes; a `string` is sent as its UTF-8 encoding. */
  send(data: Uint8Array | string): void;
  /** Register a handler invoked for each inbound chunk of bytes. */
  onData(handler: (bytes: Uint8Array) => void): void;
  /** Register a handler for the peer closing the connection. */
  onClose(handler: () => void): void;
  /** Register a handler for a transport error; `err` is the error text. */
  onError(handler: (err: string) => void): void;
  /** Close the connection. */
  close(): void;
}
/** A bound UDP socket — a per-plugin handle for connectionless datagrams. */
export interface UdpSocket {
  /** Send a datagram to `host:port`; a `string` is sent as its UTF-8 encoding. */
  sendTo(host: string, port: number, data: Uint8Array | string): void;
  /** Register a handler invoked for each inbound datagram; `from` is the sender's address. */
  onMessage(handler: (from: { host: string; port: number }, bytes: Uint8Array) => void): void;
  /** Close the socket. */
  close(): void;
}
/** Entry point for opening raw TCP + UDP sockets. */
export declare const Net: {
  /**
   * Connect a TCP client to `host:port` off the game thread.
   * @returns Resolves on connect with the live {@link TcpSocket} handle.
   * @throws Rejects on connect failure (refused, unreachable, DNS error).
   * @example
   * import { Net } from "@s2script/sdk/net";
   * const sock = await Net.connectTcp("127.0.0.1", 9000);
   * sock.onData((bytes) => console.log("got " + bytes.length + " bytes"));
   * sock.send("hello");
   */
  connectTcp(host: string, port: number): Promise<TcpSocket>;
  /**
   * Bind a UDP socket on an ephemeral local port.
   * @returns Resolves with the bound {@link UdpSocket} handle.
   */
  udp(): Promise<UdpSocket>;
};

/** A general protobuf user-message builder. Build then send in one synchronous burst. */
export class UserMessage {
  constructor(name: string);
  setInt(field: string, value: number): this;
  setFloat(field: string, value: number): this;
  setString(field: string, value: string): this;
  setBool(field: string, value: boolean): this;
  /** Infer the setter from the JS value type. */
  set(field: string, value: number | string | boolean): this;
  /** Send to one slot or a list of slots. Returns true if delivered to >=1 real client. */
  send(slots: number | number[]): boolean;
  /** Broadcast to all connected non-bot clients. */
  sendAll(): boolean;
}

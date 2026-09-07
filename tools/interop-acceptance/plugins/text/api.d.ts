import type { Notification, Hook, Transform } from "@s2script/sdk/interfaces";
export interface Payload { text: string; mode: string }
export interface Report { action: number; result: number; original: string; final: string }
export interface Contract {
  methods: { probe(mode: string): Report; malformed(): number; ping(depth: number): void };
  forwards: {
    OnSignal: Notification<Payload>;
    OnRequest: Hook<Payload>;
    OnFormat: Transform<Payload, "text">;
  };
}

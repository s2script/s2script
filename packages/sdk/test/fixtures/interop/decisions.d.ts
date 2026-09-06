import type { Notification, Hook, Transform } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: { getCount(): number; setCount(n: number): void };
  forwards: {
    OnCountChanged: Notification<{ count: number }>;
    OnRequest: Hook<{ identity: string }>;
    OnFormat: Transform<{ identity: string; text: string }, "text">;
  };
}

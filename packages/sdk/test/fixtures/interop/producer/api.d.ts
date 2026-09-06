import type { Notification } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: { getCount(): number; setCount(count: number): void };
  forwards: { OnCountChanged: Notification<{ count: number; detail?: { label: string } }> };
}

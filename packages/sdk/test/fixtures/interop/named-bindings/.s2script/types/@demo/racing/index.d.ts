import type { Notification } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: {};
  forwards: { OnRunFinished: Notification<{ elapsedMs: number }> };
}

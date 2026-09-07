import { bindForwards } from "@s2script/sdk/plugin";
import type { Subscription } from "@s2script/sdk/interfaces";

export function OnRaceFinished(event: { elapsedMs: number }): void {
  console.log(event.elapsedMs);
}

export function OnParkourFinished(event: { checkpoints: number }): void {
  console.log(event.checkpoints);
}

const racing: Subscription = bindForwards("@demo/racing", {
  OnRunFinished: OnRaceFinished,
});
const parkour: Subscription = bindForwards("@demo/parkour", {
  OnRunFinished: OnParkourFinished,
});
racing.dispose();
parkour.dispose();

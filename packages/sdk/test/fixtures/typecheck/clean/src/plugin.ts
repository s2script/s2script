import { Player } from "@s2script/cs2";
import dep from "@other/dep";
export function onLoad(): void {
  const p = Player.fromSlot(0);
  if (p && p.health !== null) console.log("hp=" + p.health);
  console.log("dep=" + String(dep));
}

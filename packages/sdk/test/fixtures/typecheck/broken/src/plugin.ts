import { Player } from "@s2script/cs2";
export function onLoad(): void {
  const hp: number = Player.fromSlot(0)!.health;   // TS2322: Type 'number | null' is not assignable to 'number'
  console.log(hp);
}

import { Entity, EntityRef } from "@s2script/sdk/entity";
export function onLoad(r: EntityRef): void {
  const hp: number = Entity.forRef(r)!.health;   // TS2322: number | null → number
  console.log(hp);
}

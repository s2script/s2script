// Hand-written ambient type for the @demo/ent inter-plugin interface.
// Interface .d.ts codegen is deferred (Slice 5+); until then a consumer declares the producer's
// published shape by hand so `import { pawnRef } from "@demo/ent"` reads typed in an editor. `pawnRef` hands back an
// EntityRef — the same @s2script/entity type the entity system uses — that the wire rehydrates into a
// LIVE ref bound to THIS context's natives (Task 1's replacer/reviver).
declare module "@demo/ent" {
  import { EntityRef } from "@s2script/sdk/entity";
  interface Ent {
    pawnRef(slot: number): EntityRef | null;
    pawnHealth(slot: number): number | null;
  }
  const _default: Ent;
  export = _default;
}

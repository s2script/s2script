export interface EntityRef { readonly index: number; readonly serial: number; }
export declare const Entity: { forRef(r: EntityRef): { health: number | null } | null };

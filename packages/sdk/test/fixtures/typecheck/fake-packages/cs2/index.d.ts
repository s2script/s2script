export interface Player { readonly slot: number; readonly health: number | null; }
export declare const Player: { fromSlot(slot: number): Player | null; all(): Player[]; };

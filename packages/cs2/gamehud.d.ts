// @s2script/cs2 — GameHud: the in-round game HUD (top rail, kill feed, status card, toast, pop-up vote, drawer).
//
// Paints `s2script_gamehud.xml` from the hudkit workshop addon. The layout spawns with
// `observable: false`, so every paint is per player: each call takes an audience.

/** Accent themes, named by colour. Any gamemode picks one. */
export type GameHudTheme = "amber" | "red" | "orange" | "teal" | "blue" | "violet" | "green";

/**
 * Text, fill and edge colours. These are the chat colour tags, so a phrase colours the same in chat
 * and on the HUD (grey general text, white emphasis, yellow numbers, green string values, blue
 * commands and hints, orange actions, red warnings, lightred the target, lightblue a failed or
 * invalid target, darkred the admin). `accent` follows the theme.
 */
export type GameHudTone =
  | "default" | "white" | "darkred" | "lightpurple" | "green" | "olive" | "lime" | "red" | "grey"
  | "yellow" | "silver" | "blue" | "darkblue" | "bluegrey" | "lightblue" | "purple" | "lightred"
  | "orange" | "accent";

/**
 * Who sees a paint: one player or slot, a list of them, a filter over active slots, or `"all"`
 * (the default). The filter receives a 0-based player slot.
 */
export type GameHudAudience =
  | "all"
  | number
  | { readonly slot: number }
  | ReadonlyArray<number | { readonly slot: number }>
  | ((slot: number) => boolean);

/**
 * Text on the HUD takes chat colour tags. A label holds one colour, so a leading tag (for example
 * `"{red}Traitor"`) picks the label's colour and every tag is then stripped.
 */
export type GameHudText = string | number;

export interface GameHudRailSide {
  readonly score?: GameHudText;
  readonly label?: GameHudText;
  /** Edge, score and pip colour. Defaults to the label's leading tag, then `blue`. */
  readonly tone?: GameHudTone;
  /** Living players, drawn as lit pips out of `total` (at most 10). */
  readonly alive?: number;
  readonly total?: number;
}

export interface GameHudRailSpec {
  readonly left?: GameHudRailSide;
  readonly right?: GameHudRailSide;
  readonly clock?: GameHudText;
  /** The line under the rail, such as "Round 4 of 12". */
  readonly sub?: GameHudText;
}

/** A claimed region. Released on `release()` or when the plugin unloads. */
export interface GameHudRegion {
  isValid(): boolean;
  /** Hide the region for everyone and free it for another plugin. */
  release(): void;
}

export interface GameHudRail extends GameHudRegion {
  /** Paint the rail. Fields you leave out keep their last value. */
  set(audience: GameHudAudience, spec: GameHudRailSpec): void;
  hide(audience?: GameHudAudience): void;
}

export interface GameHudBar {
  readonly label: GameHudText;
  /** 0 to 100. The bar moves in 10% steps. */
  readonly value: number;
  /** The value text. Defaults to the rounded value. */
  readonly text?: GameHudText;
  readonly tone?: GameHudTone;
}

export interface GameHudStatusSpec {
  /** The role line, such as `"{red}Traitor"`. Its colour comes from `tone`, its leading tag, or the theme accent. */
  readonly role?: GameHudText;
  readonly tone?: GameHudTone;
  readonly hint?: GameHudText;
  /** Up to two bars. Missing bars are hidden. */
  readonly bars?: ReadonlyArray<GameHudBar>;
}

export interface GameHudStatus extends GameHudRegion {
  set(audience: GameHudAudience, spec: GameHudStatusSpec): void;
  hide(audience?: GameHudAudience): void;
}

export interface GameHudFeedEntry {
  /** Omit for a world or suicide kill. */
  readonly attacker?: GameHudText;
  readonly victim: GameHudText;
  /** Weapon text, shown when there is no icon. */
  readonly weapon?: GameHudText;
  /**
   * A Panorama image path for the weapon icon, sent as is (tokens such as `{images}` are kept),
   * for example `"file://{images}/icons/equipment/ak47.svg"`.
   */
  readonly icon?: string;
  readonly headshot?: boolean;
  /** A short leading tag, such as `"{red}RDM"`. */
  readonly tag?: GameHudText;
  /** The row's edge colour. Defaults to the tag's leading tag, then `grey`. */
  readonly tone?: GameHudTone;
  readonly attackerTone?: GameHudTone;
  readonly victimTone?: GameHudTone;
  /** Highlight the row for the players involved. */
  readonly attackerSlot?: number;
  readonly victimSlot?: number;
  /** How long the row stays, in seconds. Default 6. */
  readonly seconds?: number;
}

export interface GameHudFeed extends GameHudRegion {
  /**
   * Add a row for an audience (default everyone). Each player keeps their own six rows, newest on
   * top. If the HUD refuses the paint for a player, the row goes to them in chat instead.
   */
  push(entry: GameHudFeedEntry, audience?: GameHudAudience): void;
  clear(audience?: GameHudAudience): void;
}

export interface GameHudToastSpec {
  readonly title: GameHudText;
  readonly sub?: GameHudText;
  readonly tone?: GameHudTone;
  /** Default 3. */
  readonly seconds?: number;
}

/** A vote option: plain text, or text with a tone. */
export type GameHudVoteOption = GameHudText | { readonly label: GameHudText; readonly tone?: GameHudTone };

export interface GameHudVoteResult {
  /** Votes per option, in option order. */
  readonly counts: ReadonlyArray<number>;
  /** The option index with the most votes, or null for a tie or no votes. */
  readonly winner: number | null;
  /** Each voter's choice, keyed by player slot. */
  readonly votes: Readonly<Record<number, number>>;
}

export interface GameHudVoteSpec {
  /** The header, such as "Map vote". Default "Vote". */
  readonly title?: GameHudText;
  readonly question: GameHudText;
  /** 2 to 8 options. Option 1 is typed as `1` in chat. */
  readonly options: ReadonlyArray<GameHudVoteOption>;
  /** Who may vote. Default everyone. */
  readonly voters?: GameHudAudience;
  /** Who sees the vote. Defaults to the voters; voters always see it. */
  readonly viewers?: GameHudAudience;
  /** Default 20. */
  readonly seconds?: number;
  /** A player voted or changed their vote. */
  onVote?(slot: number, option: number): void;
  /** The timer ran out or `end()` was called. Not called on `cancel()`. */
  onEnd?(result: GameHudVoteResult): void;
}

export interface GameHudVote {
  isOpen(): boolean;
  counts(): number[];
  /** Cast for a voter, as if they clicked. False when they cannot vote or the option is out of range. */
  cast(player: number | { readonly slot: number }, option: number): boolean;
  /** End now and report the result. */
  end(): void;
  /** End now without a result. */
  cancel(): void;
}

export interface GameHudDrawerRow {
  readonly title: GameHudText;
  readonly sub?: GameHudText;
  /** Right-aligned text, such as a price. */
  readonly meta?: GameHudText;
  /** A Panorama image path, sent as is. */
  readonly image?: string;
  readonly tone?: GameHudTone;
  readonly selected?: boolean;
  /** Greyed out and not clickable. */
  readonly disabled?: boolean;
  /** Your own data; handed back to `onSelect`. */
  readonly [key: string]: unknown;
}

export interface GameHudDrawerTab {
  readonly id: string;
  readonly label: GameHudText;
  /** Up to 8 rows. */
  readonly rows: ReadonlyArray<GameHudDrawerRow>;
}

export interface GameHudDrawerSpec {
  readonly title?: GameHudText;
  /** Header text on the right, such as `"{yellow}3 credits"`. */
  readonly right?: GameHudText;
  readonly footer?: GameHudText;
  /** Up to 5 tabs. Without tabs, `rows` are shown. */
  readonly tabs?: ReadonlyArray<GameHudDrawerTab>;
  readonly rows?: ReadonlyArray<GameHudDrawerRow>;
  /** The tab id to open on. */
  readonly tab?: string;
  onSelect?(row: GameHudDrawerRow, drawer: GameHudDrawer): void;
  onTab?(id: string, drawer: GameHudDrawer): void;
  /** `fromPlayer` is true when the player pressed Close. */
  onClose?(fromPlayer: boolean): void;
}

export interface GameHudDrawer {
  isOpen(): boolean;
  /** Repaint with changed fields; fields you leave out keep their value. */
  update(spec: GameHudDrawerSpec): void;
  setTab(id: string): void;
  close(): void;
}

export declare const GameHud: {
  readonly themes: ReadonlyArray<GameHudTheme>;
  readonly tones: ReadonlyArray<GameHudTone>;
  /** Claim the top rail. Null when another plugin holds it. Calling again returns the same claim. */
  rail(): GameHudRail | null;
  /** Claim the status card. Null when another plugin holds it. */
  status(): GameHudStatus | null;
  /** Claim the kill feed. Null when another plugin holds it. */
  feed(): GameHudFeed | null;
  /**
   * Show a toast. Shared by every plugin: the newest toast replaces the one on screen. If the HUD
   * refuses it for a player, the toast goes to them in chat.
   */
  toast(audience: GameHudAudience, spec: GameHudToastSpec): void;
  /** Set the accent theme for an audience. Shared: last write wins. */
  theme(audience: GameHudAudience, theme: GameHudTheme): void;
  /**
   * Pop up a vote. It does not take the cursor: voters type the option number in chat (the line
   * is not shown), or press Tab to click. One vote runs at a time; null while another is open.
   */
  vote(spec: GameHudVoteSpec): GameHudVote | null;
  /**
   * Open a drawer for one player: tabs and clickable rows, with a Close button. It takes the
   * cursor. Opening again for the same player repaints it, keeping fields the spec leaves out. Null when another plugin holds that
   * player's drawer.
   */
  drawer(player: number | { readonly slot: number }, spec: GameHudDrawerSpec): GameHudDrawer | null;
};

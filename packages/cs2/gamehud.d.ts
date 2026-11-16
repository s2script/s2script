// @s2script/cs2 — GameHud: the gamemode HUD kit. In-round pieces (top rail, kill feed, status card,
// toast, round-end summary) and panels (pop-up vote, drawer, sheet, prompt, intro, spectate hub).
//
// Paints `s2script_gamehud.xml` and `s2script_gamepanels.xml` from the game-kit workshop addon. The layout spawns with
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
  /** Repaint with a new spec. */
  update(spec: GameHudDrawerSpec): void;
  setTab(id: string): void;
  close(): void;
}

/** A numbered list item. Its number is `n`, else its place in the whole section (1-based). */
export type GameHudSheetListItem = GameHudText | { readonly text: GameHudText; readonly n?: GameHudText };

export interface GameHudSheetToggle {
  readonly id?: string;
  readonly label: GameHudText;
  readonly on?: boolean;
  /** Text on the right. Defaults to "On" or "Off". */
  readonly value?: GameHudText;
  readonly disabled?: boolean;
  readonly [key: string]: unknown;
}

export interface GameHudSheetCard {
  readonly id?: string;
  readonly title: GameHudText;
  readonly price?: GameHudText;
  readonly desc?: GameHudText;
  readonly selected?: boolean;
  readonly disabled?: boolean;
  readonly [key: string]: unknown;
}

export interface GameHudSheetAction {
  readonly id?: string;
  readonly label: GameHudText;
  readonly tone?: GameHudTone;
  readonly disabled?: boolean;
}

export interface GameHudSheetTableRow {
  readonly id?: string;
  /** Up to 6 cells, one per column. */
  readonly cells: ReadonlyArray<GameHudText>;
  /** Up to 2 buttons at the end of the row. */
  readonly actions?: ReadonlyArray<GameHudSheetAction>;
  readonly [key: string]: unknown;
}

/** A section's body. Each kind pages itself. */
export type GameHudSheetBody =
  /** Numbered rows, `perPage` at a time (default 6, at most 8). */
  | { readonly kind: "list"; readonly items: ReadonlyArray<GameHudSheetListItem>; readonly perPage?: number }
  /** Paragraphs, 4 per page. */
  | { readonly kind: "text"; readonly paragraphs: ReadonlyArray<GameHudText> }
  /** Clickable switches, 8 per page. A click reports the wanted state; repaint with `update`. */
  | { readonly kind: "toggles"; readonly items: ReadonlyArray<GameHudSheetToggle> }
  /** Clickable cards, 9 per page. */
  | { readonly kind: "grid"; readonly items: ReadonlyArray<GameHudSheetCard> }
  /** Columns and rows with row buttons, 8 rows per page. */
  | { readonly kind: "table"; readonly columns: ReadonlyArray<GameHudText>; readonly rows: ReadonlyArray<GameHudSheetTableRow> };

export interface GameHudSheetSection {
  readonly id: string;
  /** The side tab text. */
  readonly label: GameHudText;
  /** Consecutive sections with the same group sit under one header. */
  readonly group?: GameHudText;
  /** Small text on the side tab, such as a count. */
  readonly meta?: GameHudText;
  /** The body heading. Defaults to the label. */
  readonly heading?: GameHudText;
  /** The line under the heading. Defaults to "Page 2 of 3" when the body has pages. */
  readonly kicker?: GameHudText;
  readonly body: GameHudSheetBody;
}

export interface GameHudSheetEvent {
  readonly section: string;
  readonly kind: "toggle" | "item" | "action";
  /** The item's index in the whole section. */
  readonly index: number;
  readonly item: GameHudSheetToggle | GameHudSheetCard | GameHudSheetTableRow;
  /** For a toggle: the state the player asked for. */
  readonly on?: boolean;
  /** For a table button: its id (or label). */
  readonly action?: string;
}

export interface GameHudSheetSpec {
  readonly title?: GameHudText;
  readonly right?: GameHudText;
  readonly footer?: GameHudText;
  /**
   * Side tabs that do not fit (16 slots) collapse to group headers except the open section's
   * group, and past that scroll.
   */
  readonly sections: ReadonlyArray<GameHudSheetSection>;
  /** The section id to open on. */
  readonly section?: string;
  /** A list number in that section to open on and highlight, such as rule 3. */
  readonly highlight?: GameHudText;
  onSelect?(event: GameHudSheetEvent, sheet: GameHudSheet): void;
  onSection?(id: string, sheet: GameHudSheet): void;
  onClose?(fromPlayer: boolean): void;
}

export interface GameHudSheet {
  isOpen(): boolean;
  /** Repaint with a new spec. The open section and page stay unless the spec names a section. */
  update(spec: GameHudSheetSpec): void;
  /** Open a section, optionally on the page of a list number, highlighted. */
  show(section?: string, highlight?: GameHudText): void;
  /** Go to a page (0-based). */
  page(n: number): void;
  close(): void;
}

export interface GameHudPromptAnswer {
  readonly id?: string;
  readonly label: GameHudText;
  readonly tone?: GameHudTone;
  readonly disabled?: boolean;
}

export interface GameHudPromptResult {
  /** The answer's index, or null when no answer was picked. */
  readonly index: number | null;
  /** The answer's id (or its text). */
  readonly id: unknown;
  readonly reason: "answer" | "closed" | "timeout" | "replaced" | "cancelled";
}

export interface GameHudPromptSpec {
  readonly title?: GameHudText;
  readonly question: GameHudText;
  /** A line of context under the question. */
  readonly why?: GameHudText;
  /** 1 to 4 answers. */
  readonly answers: ReadonlyArray<GameHudText | GameHudPromptAnswer>;
  readonly footer?: GameHudText;
  /** Close with reason "timeout" after this many seconds. No timer by default. */
  readonly seconds?: number;
  onAnswer?(result: GameHudPromptResult): void;
}

export interface GameHudPrompt {
  isOpen(): boolean;
  /** Close with reason "cancelled". */
  close(): void;
}

export interface GameHudSummaryStat {
  readonly key: GameHudText;
  readonly value: GameHudText;
}

export interface GameHudSummarySpec {
  /** A small line above the title, such as "Round 4". */
  readonly eyebrow?: GameHudText;
  /** Such as `"{red}Traitors win"`. Its tone comes from `tone`, its leading tag, or the theme accent. */
  readonly title: GameHudText;
  readonly sub?: GameHudText;
  readonly tone?: GameHudTone;
  /** Up to 4 headline numbers. */
  readonly stats?: ReadonlyArray<GameHudSummaryStat>;
  /** Up to 4 columns and 5 rows. A row is a list of cells or `{ cells }`. */
  readonly table?: {
    readonly columns: ReadonlyArray<GameHudText>;
    readonly rows: ReadonlyArray<ReadonlyArray<GameHudText> | { readonly cells: ReadonlyArray<GameHudText> }>;
  };
  /** Default 8. */
  readonly seconds?: number;
}

export interface GameHudHubItem {
  readonly id?: string;
  readonly label: GameHudText;
  /** Right-aligned text, such as "3 watching". */
  readonly meta?: GameHudText;
  readonly tone?: GameHudTone;
  readonly selected?: boolean;
  readonly disabled?: boolean;
  readonly [key: string]: unknown;
}

export interface GameHudHubCard {
  readonly heading?: GameHudText;
  readonly name?: GameHudText;
  readonly sub?: GameHudText;
  /** Up to 6 key/value cells. */
  readonly fields?: ReadonlyArray<{ readonly key: GameHudText; readonly value: GameHudText }>;
}

interface GameHudHubSpecBase<H> {
  readonly eyebrow?: GameHudText;
  readonly title?: GameHudText;
  readonly sub?: GameHudText;
  /** The menu header. Defaults to "Menu" on the intro and "Spectating" on the spectate hub. */
  readonly listTitle?: GameHudText;
  readonly listRight?: GameHudText;
  /** Up to 8 menu rows. */
  readonly items?: ReadonlyArray<GameHudHubItem | string>;
  readonly footer?: GameHudText;
  readonly card?: GameHudHubCard | null;
  /** Show the Close button. Default true. */
  readonly closeable?: boolean;
  readonly closeLabel?: GameHudText;
  onSelect?(item: GameHudHubItem, hub: H): void;
  onClose?(fromPlayer: boolean): void;
}

export type GameHudIntroSpec = GameHudHubSpecBase<GameHudIntro>;

export interface GameHudIntro {
  isOpen(): boolean;
  kind(): "intro" | "spectate";
  update(spec: GameHudIntroSpec): void;
  close(): void;
}

export interface GameHudSpectatePlayer {
  readonly slot: number;
  readonly name: GameHudText;
  readonly sub?: GameHudText;
  /** The card's edge. Defaults to the name's leading tag, then `grey`. */
  readonly tone?: GameHudTone;
  /** How a click follows them. Default `"eye"`. */
  readonly mode?: "eye" | "chase";
}

/** A camera shot. Shots move the player's own observer camera, so they only apply while observing. */
export type GameHudShot =
  | {
    readonly id?: string;
    readonly kind: "follow";
    readonly label?: GameHudText;
    readonly where?: GameHudText;
    readonly target: number;
    /** Default `"eye"`. */
    readonly mode?: "eye" | "chase";
  }
  | {
    readonly id?: string;
    readonly kind: "fixed";
    readonly label?: GameHudText;
    readonly where?: GameHudText;
    readonly position: { readonly x: number; readonly y: number; readonly z: number };
    readonly angles?: { readonly pitch: number; readonly yaw: number };
  };

export interface GameHudSpectateSpec extends GameHudHubSpecBase<GameHudSpectate> {
  /** Up to 12 players in the picker. A click follows that player. */
  readonly players?: ReadonlyArray<GameHudSpectatePlayer>;
  readonly shots?: ReadonlyArray<GameHudShot>;
  /** The shot to start on, by id or index. Default the first. */
  readonly shot?: string | number;
  /** Advance to the next shot every N seconds. Picking a player stops it. Default off. */
  readonly shotSeconds?: number;
  /** Take the cursor. Default true; with false, Tab hands it over. */
  readonly cursor?: boolean;
  onPick?(player: GameHudSpectatePlayer, hub: GameHudSpectate): void;
  onShot?(shot: GameHudShot, hub: GameHudSpectate): void;
}

export interface GameHudSpectate {
  isOpen(): boolean;
  kind(): "intro" | "spectate";
  update(spec: GameHudSpectateSpec): void;
  /** Follow a player. False when the engine refused the camera command. */
  follow(player: number | { readonly slot: number }, mode?: "eye" | "chase"): boolean;
  /** Go to a shot by id or index. */
  shot(which: string | number): boolean;
  nextShot(): boolean;
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
  /**
   * Show the round-end summary. Shared by every plugin: the newest replaces the one on screen. It
   * does not take the cursor. If the HUD refuses it for a player, the title and stats go to chat.
   */
  summary(audience: GameHudAudience, spec: GameHudSummarySpec): void;
  hideSummary(audience?: GameHudAudience): void;
  /** Set the accent theme for an audience. Shared: last write wins. */
  theme(audience: GameHudAudience, theme: GameHudTheme): void;
  /**
   * Pop up a vote. It does not take the cursor: voters type the option number in chat (the line
   * is not shown), or press Tab to click. One vote runs at a time; null while another is open.
   */
  vote(spec: GameHudVoteSpec): GameHudVote | null;
  /**
   * Open a drawer for one player: tabs and clickable rows, with a Close button. It takes the
   * cursor. Opening again for the same player repaints it. Null when another plugin holds that
   * player's drawer.
   */
  drawer(player: number | { readonly slot: number }, spec: GameHudDrawerSpec): GameHudDrawer | null;
  /**
   * Open a sheet for one player: side tabs, a paged body and a Close button. It takes the cursor.
   * Opening again for the same player repaints it. Null when another plugin holds that player's sheet.
   */
  sheet(player: number | { readonly slot: number }, spec: GameHudSheetSpec): GameHudSheet | null;
  /**
   * Ask one player a question with clickable answers. It takes the cursor and has a Close button.
   * A new prompt replaces this plugin's open one. If the HUD refuses it, the question goes to chat
   * and the player answers by typing the number. Null when another plugin holds that player's prompt.
   */
  prompt(player: number | { readonly slot: number }, spec: GameHudPromptSpec): GameHudPrompt | null;
  /**
   * Show the intro: the mode's brand, a menu and an optional profile card. It takes the cursor.
   * The intro and the spectate hub share one place per player: opening one replaces the other.
   * Null when another plugin holds that player's hub.
   */
  intro(player: number | { readonly slot: number }, spec: GameHudIntroSpec): GameHudIntro | null;
  /**
   * Show the spectate hub: a menu, a player picker and a camera shot list. Null when another
   * plugin holds that player's hub.
   */
  spectate(player: number | { readonly slot: number }, spec: GameHudSpectateSpec): GameHudSpectate | null;
};

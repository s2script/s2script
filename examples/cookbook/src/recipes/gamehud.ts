import { command, config, hook, HookResult } from "@s2script/sdk";
import { GameHud, Player } from "@s2script/cs2";
import type {
  GameHudFeed, GameHudRail, GameHudSheetSection, GameHudSpectatePlayer, GameHudStatus, GameHudTheme,
} from "@s2script/cs2";

/**
 * The gamemode HUD kit, end to end. Needs the game-kit workshop addon (3790153369) mounted.
 *
 * In-round pieces paint per player and follow the game:
 *   sm_gk_on [theme]      claim the top rail, status card and kill feed; track kills and rounds
 *   sm_gk_off             release them (they also free themselves when the plugin unloads)
 *   sm_gk_toast <text>    a toast for everyone
 *   sm_gk_summary         a sample round-end summary (also shown on every round_end while on)
 *
 * Panels:
 *   sm_gk_vote            a pop-up vote for everyone: type 1-3 in chat, or Tab to click
 *   sm_gk_shop            a drawer with tabs and clickable rows
 *   sm_rules [target] [section] [n]
 *                         the rules sheet, read from configs/gamehud-rules.json (written on first
 *                         use). Opens on your team's section, or the one named, on rule n, highlighted:
 *                         `sm_rules @me rebels 3`, `sm_rules bob guards 2`
 *   sm_gk_ask             a prompt with a 15 second timer
 *   sm_gk_intro           the intro menu; Spectate opens the spectate hub, which has a player
 *                         picker, a camera shot list, team picks and a way back to the intro
 *
 * Every surface falls back to chat for a player whose HUD refuses it.
 */
export const name = "gamehud";
export const describe = "the gamemode HUD kit: rail, feed, status, summary, vote, drawer, rules sheet, prompt, intro, spectate hub (sm_gk_*)";

const T = 2;
const CT = 3;
const SPEC = 1;

let rail: GameHudRail | null = null;
let status: GameHudStatus | null = null;
let feed: GameHudFeed | null = null;
let round = 0;
let credits: Record<number, number> = {};

// ── rules config ─────────────────────────────────────────────────────────────────────────────
// One file an operator edits. Sections keep their order; a section's rules are numbered by their
// place in it, so "rebels 3" is the same rule on every page and after every reload.
const RULES_FILE = "gamehud-rules.json";
const DEFAULT_RULES = {
  title: "Server rules",
  sections: [
    { id: "general", label: "General", rules: [
      "Be respectful in chat and voice.", "No cheating, scripting or exploiting.", "Do not ghost: dead players stay quiet about the round.",
      "No mic spam or soundboards.", "Admins have the final say.", "Report problems with !calladmin.", "English in global chat, please.",
    ] },
    { id: "guards", label: "Guards", group: "Teams", rules: [
      "Give clear orders and repeat them once.", "No freekilling: a prisoner must break an order first.", "Warn once before shooting a rebel.",
    ] },
    { id: "rebels", label: "Rebels", group: "Teams", rules: [
      "Rebelling is allowed after the first order.", "A rebel may be shot on sight.", "Last prisoner may ask for a last request.",
    ] },
    { id: "about", label: "About", paragraphs: [
      "This server runs on s2script. The rules above are read from configs/gamehud-rules.json.",
      "Edit that file and run sm_rules again: there is nothing to rebuild.",
    ] },
  ],
};

interface RulesFile {
  title?: string;
  sections?: { id: string; label: string; group?: string; rules?: string[]; paragraphs?: string[] }[];
}

function loadRules(): RulesFile {
  const raw = config.readFile(RULES_FILE);
  if (raw === null) {
    config.writeFile(RULES_FILE, JSON.stringify(DEFAULT_RULES, null, 2) + "\n");
    return DEFAULT_RULES;
  }
  try {
    return JSON.parse(raw) as RulesFile;
  } catch (e) {
    console.log("[cookbook] gamehud: " + RULES_FILE + " is not valid JSON, using the built-in rules: " + String(e));
    return DEFAULT_RULES;
  }
}

function rulesSections(file: RulesFile): GameHudSheetSection[] {
  return (file.sections ?? []).map((s) => ({
    id: s.id,
    label: s.label,
    group: s.group,
    meta: s.rules ? String(s.rules.length) : undefined,
    body: s.paragraphs ? { kind: "text" as const, paragraphs: s.paragraphs } : { kind: "list" as const, items: s.rules ?? [] },
  }));
}

/** The section a player most likely wants: their team's, else the first. */
function sectionFor(slot: number): string | undefined {
  const team = Player.fromSlot(slot)?.teamNum;
  if (team === T) return "rebels";
  if (team === CT) return "guards";
  return undefined;
}

function openRules(slot: number, section?: string, rule?: string): void {
  const file = loadRules();
  GameHud.sheet(slot, {
    title: file.title ?? "Rules",
    right: "{grey}sm_rules <section> <n>",
    footer: "Rule numbers stay the same on every page.",
    sections: rulesSections(file),
    section: section ?? sectionFor(slot),
    highlight: rule,
  });
}

// ── in-round ─────────────────────────────────────────────────────────────────────────────────
function teamCounts(team: number): { alive: number; total: number } {
  let alive = 0;
  let total = 0;
  for (const p of Player.all()) {
    if (p.teamNum !== team) continue;
    total++;
    if (p.pawnIsAlive) alive++;
  }
  return { alive, total };
}

function paintRail(): void {
  if (!rail) return;
  const t = teamCounts(T);
  const ct = teamCounts(CT);
  rail.set("all", {
    left: { label: "{red}Prisoners", score: t.alive, alive: t.alive, total: t.total },
    right: { label: "{blue}Guards", score: ct.alive, alive: ct.alive, total: ct.total },
    sub: "Round " + round,
  });
}

function paintStatus(slot: number): void {
  if (!status) return;
  const p = Player.fromSlot(slot);
  const hp = p?.pawn?.health ?? 0;
  const team = p?.teamNum;
  status.set(slot, {
    role: team === T ? "{red}Prisoner" : team === CT ? "{blue}Guard" : "{grey}Spectator",
    hint: "Rules: {blue}!rules",
    bars: [{ label: "Health", value: Math.max(0, Math.min(100, hp)), tone: "green" }, { label: "Credits", value: Math.min(100, (credits[slot] ?? 0) * 10), text: String(credits[slot] ?? 0), tone: "yellow" }],
  });
}

function showSummary(winner: number): void {
  const t = teamCounts(T);
  const ct = teamCounts(CT);
  GameHud.summary("all", {
    eyebrow: "Round " + round,
    title: winner === T ? "{red}Prisoners win" : winner === CT ? "{blue}Guards win" : "{grey}Round draw",
    sub: t.alive + " prisoners and " + ct.alive + " guards left standing",
    stats: [{ key: "Prisoners", value: t.alive + "/" + t.total }, { key: "Guards", value: ct.alive + "/" + ct.total }],
    table: {
      columns: ["Player", "Team", "Credits"],
      rows: Player.all().slice(0, 5).map((p) => [p.playerName ?? "?", p.teamNum === T ? "Prisoner" : p.teamNum === CT ? "Guard" : "Spec", credits[p.slot] ?? 0]),
    },
  });
}

function nameOf(slot: number): string {
  return Player.fromSlot(slot)?.playerName ?? "world";
}

// ── intro and spectate ───────────────────────────────────────────────────────────────────────
function openIntro(slot: number): void {
  const p = Player.fromSlot(slot);
  GameHud.intro(slot, {
    eyebrow: "Welcome to",
    title: "Cookbook Jail",
    sub: "A demo of the s2script game HUD kit",
    items: [
      { id: "play", label: "{orange}Play", meta: "join a team" },
      { id: "spectate", label: "Spectate", meta: Player.all().filter((x) => x.pawnIsAlive).length + " alive" },
      { id: "rules", label: "Rules" },
    ],
    card: { heading: "You", name: p?.playerName ?? "", sub: "slot " + slot, fields: [{ key: "Credits", value: credits[slot] ?? 0 }, { key: "Round", value: round }] },
    onSelect: (item, hub) => {
      if (item.id === "rules") { openRules(slot); return; }
      if (item.id === "spectate") { openSpectate(slot); return; }
      hub.close();
      joinSmallerTeam(slot);
    },
  });
}

/** The demo's team rule: join the smaller side, prisoners on a tie. */
function joinSmallerTeam(slot: number): void {
  const p = Player.fromSlot(slot);
  if (!p) return;
  const team = teamCounts(CT).total < teamCounts(T).total ? CT : T;
  p.changeTeam(team);
  GameHud.toast(slot, { title: team === T ? "{red}You are a prisoner" : "{blue}You are a guard", sub: "Read {blue}!rules" });
}

function spectatePlayers(viewer: number): GameHudSpectatePlayer[] {
  return Player.all()
    .filter((p) => p.slot !== viewer && p.pawnIsAlive)
    .slice(0, 12)
    .map((p) => ({ slot: p.slot, name: (p.teamNum === T ? "{red}" : "{blue}") + (p.playerName ?? "?"), sub: (p.pawn?.health ?? 0) + " HP" }));
}

function openSpectate(slot: number): void {
  const players = spectatePlayers(slot);
  const self = Player.fromSlot(slot);
  if (self && self.teamNum !== SPEC && self.pawnIsAlive) self.changeTeam(SPEC);
  GameHud.spectate(slot, {
    eyebrow: "Spectating",
    title: "Cookbook Jail",
    items: [
      { id: "join", label: "{orange}Join a team", meta: "smaller side" },
      { id: "next", label: "Next shot" },
      { id: "back", label: "Back" },
    ],
    players,
    shots: [
      // A fixed shot: replace with a spot on your map (`getpos` prints yours).
      { id: "overview", kind: "fixed", label: "Overview", where: "map origin", position: { x: 0, y: 0, z: 512 }, angles: { pitch: 70, yaw: 0 } },
      ...players.slice(0, 3).map((p) => ({ id: "p" + p.slot, kind: "follow" as const, label: "Following " + p.name, target: p.slot })),
    ],
    shotSeconds: 8,
    footer: "Click a player to follow them.",
    onSelect: (item, hub) => {
      if (item.id === "next") { hub.nextShot(); return; }
      if (item.id === "back") { openIntro(slot); return; }
      hub.close();
      joinSmallerTeam(slot);
    },
  });
}

export function OnPluginStart(): void {
  command("sm_gk_on", (cmd) => {
    rail = GameHud.rail();
    status = GameHud.status();
    feed = GameHud.feed();
    const theme = cmd.arg(0) as GameHudTheme;
    if (theme && GameHud.themes.includes(theme)) GameHud.theme("all", theme);
    paintRail();
    for (const p of Player.all()) paintStatus(p.slot);
    cmd.reply(`[cookbook] gamehud: rail=${rail ? "ok" : "held by another plugin"} status=${status ? "ok" : "held"} feed=${feed ? "ok" : "held"}`);
    return HookResult.Handled;
  });

  command("sm_gk_off", (cmd) => {
    rail?.release(); status?.release(); feed?.release();
    rail = status = feed = null;
    GameHud.hideSummary("all");
    cmd.reply("[cookbook] gamehud: released");
    return HookResult.Handled;
  });

  command("sm_gk_toast", (cmd) => {
    GameHud.toast("all", { title: cmd.argString || "{orange}Hello", sub: "from sm_gk_toast" });
    return HookResult.Handled;
  });

  command("sm_gk_summary", () => { showSummary(T); return HookResult.Handled; });

  command("sm_gk_vote", (cmd) => {
    const v = GameHud.vote({
      title: "Vote",
      question: "Extend the map?",
      options: [{ label: "Extend 15 minutes", tone: "green" }, "Change map", { label: "Don't care", tone: "grey" }],
      seconds: 20,
      onEnd: (r) => {
        const pick = r.winner === null ? "No winner" : ["Extended", "Changing map", "Nobody cares"][r.winner];
        GameHud.toast("all", { title: pick, sub: r.counts.join(" / ") + " votes" });
      },
    });
    if (!v) cmd.reply("[cookbook] gamehud: another vote is running");
    return HookResult.Handled;
  });

  command("sm_gk_shop", (cmd) => {
    const slot = cmd.callerSlot;
    if (slot < 0) { cmd.reply("[cookbook] gamehud: run this as a player"); return HookResult.Handled; }
    const have = (): number => credits[slot] ?? 0;
    const tabs = (): { id: string; label: string; rows: { id: string; title: string; sub: string; meta: string; cost: number; disabled: boolean }[] }[] => [
      { id: "gear", label: "Gear", rows: [
        { id: "armor", title: "Armour", sub: "Kevlar and helmet", meta: "2", cost: 2, disabled: have() < 2 },
        { id: "health", title: "Health shot", sub: "+50 HP", meta: "1", cost: 1, disabled: have() < 1 },
      ] },
      { id: "fun", label: "Fun", rows: [
        { id: "taunt", title: "{orange}Taunt", sub: "Everyone hears it", meta: "0", cost: 0, disabled: false },
      ] },
    ];
    GameHud.drawer(slot, {
      title: "Shop",
      right: `{yellow}${have()} credits`,
      tabs: tabs(),
      footer: "Earn credits by surviving rounds.",
      onSelect: (row, drawer) => {
        const cost = Number(row.cost);
        if (have() < cost) return;
        credits[slot] = have() - cost;
        GameHud.toast(slot, { title: "Bought " + String(row.title), tone: "green" });
        drawer.update({ title: "Shop", right: `{yellow}${have()} credits`, tabs: tabs(), footer: "Earn credits by surviving rounds." });
        paintStatus(slot);
      },
    });
    return HookResult.Handled;
  });

  command("sm_rules", (cmd) => {
    // sm_rules [target] [section] [n]: a lone section name or number is for the caller.
    let targets = cmd.callerSlot >= 0 ? [cmd.callerSlot] : [];
    let i = 0;
    const first = cmd.arg(0);
    const sections = rulesSections(loadRules()).map((s) => s.id);
    if (first && !sections.includes(first) && !/^\d+$/.test(first)) {
      targets = Player.target(first, cmd.callerSlot).map((p) => p.slot);
      i = 1;
    }
    let section: string | undefined;
    if (cmd.arg(i) && sections.includes(cmd.arg(i))) { section = cmd.arg(i); i++; }
    const rule = cmd.arg(i) || undefined;
    if (targets.length === 0) { cmd.reply("[cookbook] usage: sm_rules [target] [" + sections.join("|") + "] [n]"); return HookResult.Handled; }
    for (const slot of targets) openRules(slot, section, rule);
    return HookResult.Handled;
  });

  command("sm_gk_ask", (cmd) => {
    const slot = cmd.callerSlot;
    if (slot < 0) { cmd.reply("[cookbook] gamehud: run this as a player"); return HookResult.Handled; }
    GameHud.prompt(slot, {
      title: "Quick question",
      question: "Was your last death fair?",
      why: "Answers go to the server log. This closes in 15 seconds.",
      answers: [{ id: "fair", label: "{green}It was fair" }, { id: "report", label: "{red}Report it" }, { id: "skip", label: "{grey}Skip" }],
      seconds: 15,
      onAnswer: (r) => console.log(`[cookbook] gamehud: slot ${slot} answered ${String(r.id)} (${r.reason})`),
    });
    return HookResult.Handled;
  });

  command("sm_gk_intro", (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply("[cookbook] gamehud: run this as a player"); return HookResult.Handled; }
    openIntro(cmd.callerSlot);
    return HookResult.Handled;
  });

  hook.on("player_death", (e) => {
    const victim = e.getPlayerSlot("userid");
    const attacker = e.getPlayerSlot("attacker");
    const weapon = e.getString("weapon");
    if (feed) {
      feed.push({
        attacker: attacker >= 0 && attacker !== victim ? nameOf(attacker) : undefined,
        victim: nameOf(victim),
        weapon,
        icon: weapon ? `file://{images}/icons/equipment/${weapon}.svg` : undefined,
        headshot: e.getBool("headshot"),
        attackerSlot: attacker,
        victimSlot: victim,
        attackerTone: Player.fromSlot(attacker)?.teamNum === T ? "red" : "blue",
        victimTone: Player.fromSlot(victim)?.teamNum === T ? "red" : "blue",
      });
    }
    paintRail();
    paintStatus(victim);
  });

  hook.on("player_hurt", (e) => { paintStatus(e.getPlayerSlot("userid")); });
  hook.on("player_spawn", (e) => { paintStatus(e.getPlayerSlot("userid")); paintRail(); });

  hook.on("round_start", () => {
    round++;
    GameHud.hideSummary("all");
    paintRail();
  });

  hook.on("round_end", (e) => {
    for (const p of Player.all()) if (p.pawnIsAlive) credits[p.slot] = (credits[p.slot] ?? 0) + 1;
    if (rail) showSummary(e.getInt("winner"));
  });
}

export function OnMapStart(): void {
  round = 0;
  credits = {};
}

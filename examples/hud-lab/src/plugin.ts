/**
 * hud-lab — a test harness for the CS2 update that added `custom_hud_layout`, `cs_player_camera`,
 * the new bomb callbacks, and the movetype accessors.
 *
 * WHAT THIS IS. cs_script (Valve's per-map JS VM) got the new API; s2script is a different host, so
 * none of `CustomHudLayout.SetHasClass`, `Instance.OnCustomHudClicked`, `CSPlayerPawn.GetCamera`
 * are callable from a plugin. This harness reimplements what is reachable and reports precisely
 * what is not, so that after an update you can tell the three cases apart on a live server:
 *
 *   TIER A  reachable today  — raw offset writes, entity I/O, game events, generated schema fields
 *   TIER B  ui / @s2script/cs2 — promoted HUD engine calls (SetHasClass/DialogVariable/InputCapture)
 *   TIER C  resolved — inbound clicks via ui.onCustomHudClicked (game-package hook)
 *
 * `sm_hud_status` prints all three. Start there.
 *
 * SAFETY. Every offset this plugin writes through is transcribed from a schema dump that this repo's
 * own tooling has not confirmed (games/cs2/gamedata/schema-catalog.json predates the update). Writes
 * are gated on `probeLayout` and read back afterwards; see src/offsets.ts for the full caveat and
 * the steps to retire it.
 *
 * COMMANDS
 *   sm_hud_status                              tier A/B/C report + live layout readout
 *   sm_hud_probe [start] [len]                 raw byte window, to verify offsets against a dump
 *   sm_hud_create [layout]                     create + spawn a custom_hud_layout
 *   sm_hud_list                                every custom_hud_layout in the world
 *   sm_hud_remove                              remove the ones this plugin created
 *   sm_hud_capture <0|1>                       SetInputCaptureEnabled (tier A, raw write)
 *   sm_hud_class <panel> <class> <0|1|-1> [t]  SetHasClass / …ForPlayer (tier B)
 *   sm_hud_var <panel> <var> <value> [target]  SetDialogVariableString / …ForPlayer (tier B)
 *   sm_hud_visible <target> <0|1>              SetHUDVisibility entity input
 *   sm_cam_create                              create a cs_player_camera at the caller
 *   sm_cam_enable <0|1>                        probe the camera's Enable/Disable inputs
 *   sm_cam_angles <0|1>                        probe SetIsControllingAngles
 *   sm_cam_remove                              remove the cameras this plugin created
 *   sm_bomb_watch <0|1>                        log the four new plant/defuse callbacks
 *   sm_bomb_info                               C4 field readout
 *   sm_bomb_abort                              probe C4.AbortPlant as an entity input
 *   sm_movetype [target] [type]                GetMoveType / SetMoveType
 *   sm_hud_menu [chat|center]                  SourceMod menu feature test
 *   sm_buymenu [chat|center]                   categorised buy menu
 */
import { ADMFLAG, config, Chat, hook, command, HookResult } from "@s2script/sdk";
import type { CommandInvocation, Client } from "@s2script/sdk";
import { Player, Pawn, ui } from "@s2script/cs2";
import type { Hud } from "@s2script/cs2";
import { LIVE_HUD, LIVE_PANELS } from "./livehud";
import { LiveDemo } from "./livedemo";

import * as layout from "./layout";
import * as camera from "./camera";
import * as bomb from "./bomb";
import * as movement from "./movement";
import * as menus from "./menus";
import { DemoHud, sendOnce } from "./demohud";
import { LAYOUT, STATE, LAYOUT_SIZE, globalStateField } from "./offsets";
import {
  readLayoutInfo, isInputCaptureEnabled, setInputCaptureEnabled, dumpWindow, probeLayout,
  readPlayerClasses, setPlayerClassStatus, playerStateCount,
  readPlayerInputCapture, setPlayerInputCapture,
} from "./hudstate";

/** Prefix every log line so a live server's console stays greppable. */
const TAG = "[hud-lab]";

function log(line: string): void {
  console.log(`${TAG} ${line}`);
}

let kitHud!: Hud;
let hud!: DemoHud;
let demo!: LiveDemo;

/** Standing eye height above the pawn origin. The camera spawns here rather than at the feet so
 *  that "the view did not change" is a real signal instead of an obvious offset. */
const EYE_HEIGHT = 64;

export function OnPluginStart(): void {
  /** Shipped workshop kit driven through the game-package `ui` API. */
  kitHud = ui.hud();

  kitHud.onClick("s2_btn_3", (slot) => {
    kitHud.hide(slot, "s2_dialog");
    Chat.toSlot(slot, "[hud] dismissed");
  });

  ui.onCustomHudClicked((view) => {
    const ref = view.player;
    const who = ref
      ? Player.all().find((p) => p.ref.index === ref.index && p.ref.id === ref.id) ?? null
      : null;
    const slot = who?.slot ?? -1;
    const name = who?.playerName ?? (slot >= 0 ? `slot ${slot}` : "<unknown>");
    log(`RAW CLICK button="${view.buttonId}" by ${name} (slot ${slot})`);
  });

  // The working centre-panel HTML HUD (separate from custom_hud_layout).
  hud = new DemoHud();

  bomb.install(log);

  // ── Map start ───────────────────────────────────────────────────────────────────────────────
  // Do not spawn a custom_hud_layout from OnMapStart — the world is not up yet. `ui` waits
  // for an active client, then spawns any layout registered via hud() / createLayout.

  // ── Status ──────────────────────────────────────────────────────────────────────────────────
  command.admin("sm_hud_status", ADMFLAG.GENERIC, (cmd) => {
    cmd.reply(`${TAG} ── TIER A (reachable today) ──`);
    const all = layout.findAll();
    cmd.reply(`  custom_hud_layout entities in world: ${all.length}`);
    for (const ref of all) {
      const info = readLayoutInfo(ref);
      cmd.reply(
        `  #${info.index} name="${info.targetname ?? "?"}" layoutPtr=${info.layoutPtr} ` +
        `panelIds=${info.panelIds} classNames=${info.classNames} dialogVars=${info.dialogVariableNames}`,
      );
      cmd.reply(
        `        inputCapture=${info.inputCapture} playerStates=${info.playerLayoutStates} ` +
        `globalHasClasses=${info.globalHasClasses} globalDialogVars=${info.globalDialogVars}`,
      );
      if (!info.probe.ok) cmd.reply(`        PROBE FAILED: ${info.probe.reasons.join("; ")}`);
    }
    if (all.length === 0) cmd.reply("  (none — run sm_hud_create, or load a map that ships one)");
    cmd.reply(`  cs_player_camera entities in world: ${camera.findAll().length}`);
    cmd.reply(`  bomb watch: ${bomb.isWatching() ? "ON" : "off"} (${bomb.WATCHED_EVENTS.length} events subscribed)`);

    cmd.reply(`  demo HUD viewers: ${hud.count}`);

    cmd.reply(`${TAG} ── TIER B (ui / @s2script/cs2) ──`);
    cmd.reply("  layout: panorama/layout/custom_game/s2script_hud.xml");
    cmd.reply("  addons: 3790153369 (MultiAddonManager)");
    cmd.reply(`${TAG} ── TIER C (ui.onCustomHudClicked) ──`);
    cmd.reply("  ARMED via game-package hook — use sm_kit / sm_hud_show, then click s2_btn_*");
    return HookResult.Handled;
  });

  // ── Offset verification ─────────────────────────────────────────────────────────────────────
  command.admin("sm_hud_probe", ADMFLAG.ROOT, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout in the world — run sm_hud_create first`); return HookResult.Handled; }
    // Default window brackets the global state, the field this plugin actually writes.
    const start = cmd.argInt(0, LAYOUT.globalLayoutState);
    const len = Math.min(cmd.argInt(1, 64), 256);
    cmd.reply(`${TAG} entity #${ref.index}, sizeof=${LAYOUT_SIZE}, window +${start}..+${start + len}`);
    cmd.reply(`  m_bInputCaptureEnabled is expected at +${globalStateField(STATE.inputCaptureEnabled)}`);
    for (const line of dumpWindow(ref, start, len)) cmd.reply(`  ${line}`);
    return HookResult.Handled;
  });

  // ── Why does the MAP's entity render and ours not? ──────────────────────────────────────────
  // A full byte-diff of a map-authored custom_hud_layout against one we spawned. Everything the
  // plugin reads by name already matches (layout pointer set, playerStates=12, same counts), so the
  // difference — whatever makes the client draw one and ignore the other — is in a field this
  // plugin has no name for. This finds it without needing one.
  command.admin("sm_hud_diff", ADMFLAG.ROOT, (cmd) => {
    const all = layout.findAll();
    const mine = all.find((e) => e.name === layout.OWNED_TARGETNAME);
    const theirs = all.find((e) => e.name !== layout.OWNED_TARGETNAME);
    if (!mine || !theirs) {
      cmd.reply(`${TAG} need BOTH a map-authored and a plugin-created layout in the world`);
      cmd.reply(`${TAG} load editor/zoo/script_zoo, then sm_hud_create`);
      return HookResult.Handled;
    }
    cmd.reply(`${TAG} diffing map-authored #${theirs.index} vs ours #${mine.index} over ${LAYOUT_SIZE} bytes`);
    const runs: string[] = [];
    let runStart = -1;
    for (let off = 0; off < LAYOUT_SIZE; off++) {
      const a = theirs.readUInt8(off);
      const b = mine.readUInt8(off);
      const differs = a !== b;
      if (differs && runStart < 0) runStart = off;
      if ((!differs || off === LAYOUT_SIZE - 1) && runStart >= 0) {
        const end = differs ? off : off - 1;
        // Pointers and per-instance junk differ by nature; report the RUN, and let the reader judge
        // which look like flags (short runs of small values) rather than addresses (8-byte, huge).
        const av: number[] = [], bv: number[] = [];
        for (let k = runStart; k <= Math.min(end, runStart + 15); k++) {
          av.push(theirs.readUInt8(k) ?? 0); bv.push(mine.readUInt8(k) ?? 0);
        }
        runs.push(
          `  +${String(runStart).padStart(4)}..${String(end).padStart(4)} ` +
          `map=[${av.map((x) => x.toString(16).padStart(2, "0")).join(" ")}] ` +
          `ours=[${bv.map((x) => x.toString(16).padStart(2, "0")).join(" ")}]`,
        );
        runStart = -1;
      }
    }
    if (runs.length === 0) { cmd.reply("  IDENTICAL — the difference is not in this entity's memory"); return HookResult.Handled; }
    cmd.reply(`  ${runs.length} differing run(s):`);
    // Cap the reply: a full dump would flood rcon and the interesting fields are the early ones.
    for (const r of runs.slice(0, 24)) cmd.reply(r);
    if (runs.length > 24) cmd.reply(`  … ${runs.length - 24} more`);
    return HookResult.Handled;
  });

  // ── Layout entity lifecycle ─────────────────────────────────────────────────────────────────
  command.admin("sm_hud_create", ADMFLAG.ROOT, (cmd) => {
    const name = cmd.argCount > 0 ? cmd.argsFrom(0) : config.getString("layout");
    const result = layout.create(name);
    if (result.error) { cmd.reply(`${TAG} ${result.error}`); return HookResult.Handled; }
    const info = readLayoutInfo(result.ref!);
    cmd.reply(`${TAG} created entity #${info.index} with keyvalues ${JSON.stringify(result.keyvalues)}`);
    cmd.reply(
      `  layoutPtr=${info.layoutPtr} panelIds=${info.panelIds} classNames=${info.classNames} ` +
      `dialogVars=${info.dialogVariableNames}`,
    );
    // These counts say NOTHING about whether the asset loaded — they are the INTERNING TABLES,
    // filled in by SetHasClass/SetDialogVariableString calls naming a panel id or class. A freshly
    // spawned entity reads 0 even with a perfectly good layout behind it, because nothing has
    // referenced a name yet. (Proven: on the zoo map, the map-authored entity read panels=1 only
    // because its own script had called the setters; an entity of ours pointing at the SAME asset
    // read 0.) An earlier version of this command reported 0 as "no layout asset loaded", which was
    // simply wrong and sent people hunting for a workshop subscription they did not need.
    if (info.panelIds === 0 && info.classNames === 0) {
      cmd.reply("  panelIds=0 classNames=0 — normal for a fresh entity; these count INTERNED NAMES,");
      cmd.reply("  not asset state. They rise when sm_hud_class / sm_hud_var name a panel or class.");
    }
    cmd.reply(`  layout pointer: ${info.hasLayoutString ? "SET" : "NULL — the keyvalue did not take"}`);
    if (!info.probe.ok) cmd.reply(`  PROBE FAILED: ${info.probe.reasons.join("; ")}`);
    return HookResult.Handled;
  });

  command.admin("sm_hud_list", ADMFLAG.GENERIC, (cmd) => {
    const all = layout.findAll();
    if (all.length === 0) { cmd.reply(`${TAG} no custom_hud_layout entities`); return HookResult.Handled; }
    for (const ref of all) {
      const info = readLayoutInfo(ref);
      const owned = info.targetname === layout.OWNED_TARGETNAME ? " (ours)" : " (map-authored)";
      cmd.reply(
        `${TAG} #${info.index}${owned} layout=${info.hasLayoutString ? "set" : "none"} ` +
        `panels=${info.panelIds} classes=${info.classNames} vars=${info.dialogVariableNames} ` +
        `capture=${info.inputCapture}`,
      );
    }
    return HookResult.Handled;
  });

  command.admin("sm_hud_remove", ADMFLAG.ROOT, (cmd) => {
    cmd.reply(`${TAG} removed ${layout.removeOwned()} plugin-created layout entit(ies)`);
    return HookResult.Handled;
  });

  // ── Tier A: input capture ───────────────────────────────────────────────────────────────────
  command.admin("sm_hud_capture", ADMFLAG.ROOT, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout — run sm_hud_create first`); return HookResult.Handled; }
    if (cmd.argCount === 0) {
      cmd.reply(`${TAG} IsInputCaptureEnabled = ${isInputCaptureEnabled(ref)}`);
      return HookResult.Handled;
    }
    const on = cmd.arg(0) === "1" || cmd.arg(0).toLowerCase() === "true";
    const err = setInputCaptureEnabled(ref, on);
    cmd.reply(err ? `${TAG} SetInputCaptureEnabled(${on}) refused — ${err}` : `${TAG} SetInputCaptureEnabled(${on}) OK (read back ${on})`);
    return HookResult.Handled;
  });

  // ── Tier B: classes and dialog variables ────────────────────────────────────────────────────
  command.admin("sm_hud_class", ADMFLAG.ROOT, (cmd) => {
    if (cmd.argCount < 3) { cmd.reply(`${TAG} usage: sm_hud_class <panelId> <className> <1|0|-1> [target]`); return HookResult.Handled; }
    const slot = resolveOptionalSlot(cmd, 3, cmd.callerSlot);
    const status = parseClassStatus(cmd.arg(2));
    const err = kitHud.setClass(slot, cmd.arg(0), cmd.arg(1), status === 1);
    cmd.reply(err ? `${TAG} ${err}` : `${TAG} setClass slot ${slot} "${cmd.arg(0)}" "${cmd.arg(1)}" -> ${status}`);
    return HookResult.Handled;
  });

  command.admin("sm_hud_var", ADMFLAG.ROOT, (cmd) => {
    if (cmd.argCount < 3) { cmd.reply(`${TAG} usage: sm_hud_var <panelId> <variableName> <value> [target]`); return HookResult.Handled; }
    const slot = resolveOptionalSlot(cmd, 3, cmd.callerSlot);
    const err = kitHud.setText(slot, cmd.arg(0), cmd.argsFrom(2));
    cmd.reply(err ? `${TAG} ${err}` : `${TAG} setText slot ${slot} "${cmd.arg(0)}"`);
    return HookResult.Handled;
  });

  // ── Tier A: the SetHUDVisibility entity input ───────────────────────────────────────────────
  command.admin("sm_hud_visible", ADMFLAG.SLAY, (cmd) => {
    if (cmd.argCount < 2) { cmd.reply(`${TAG} usage: sm_hud_visible <target> <0|1>`); return HookResult.Handled; }
    const on = cmd.arg(1) === "1";
    let n = 0;
    for (const p of Player.target(cmd.arg(0), cmd.callerSlot)) {
      const pawn = p.pawn;
      // The datadesc names this input on CBasePlayerPawn, so it is fired at the PAWN, not the
      // controller — a common way to get a silent no-op here.
      if (pawn?.isValid && pawn.ref.acceptInput("SetHUDVisibility", on ? "1" : "0")) n++;
    }
    cmd.reply(`${TAG} queued SetHUDVisibility=${on ? 1 : 0} on ${n} pawn(s) — queued, not confirmed`);
    return HookResult.Handled;
  });

  // ── Camera ──────────────────────────────────────────────────────────────────────────────────
  command.admin("sm_cam_create", ADMFLAG.ROOT, (cmd) => {
    const pawn = requireCallerPawn(cmd);
    if (!pawn) return HookResult.Handled;
    const origin = pawn.origin;
    const angles = pawn.angles;
    if (!origin || !angles) { cmd.reply(`${TAG} cannot read caller transform`); return HookResult.Handled; }
    const result = camera.create([origin.x, origin.y, origin.z + EYE_HEIGHT], [angles.x, angles.y, angles.z]);
    cmd.reply(result.error ? `${TAG} ${result.error}` : `${TAG} created cs_player_camera #${result.ref?.index}`);
    return HookResult.Handled;
  });

  command.admin("sm_cam_enable", ADMFLAG.ROOT, (cmd) => {
    const pawn = requireCallerPawn(cmd);
    if (!pawn) return HookResult.Handled;
    const cam = camera.findAll()[0];
    if (!cam) { cmd.reply(`${TAG} no cs_player_camera — run sm_cam_create first`); return HookResult.Handled; }
    const on = cmd.arg(0) === "1";
    const queued = camera.setEnabled(cam, on, pawn.ref);
    cmd.reply(`${TAG} ${on ? "Enable" : "Disable"} queued=${queued} — an unknown input queues then drops silently, so judge by your view, not this line`);
    return HookResult.Handled;
  });

  command.admin("sm_cam_angles", ADMFLAG.ROOT, (cmd) => {
    const pawn = requireCallerPawn(cmd);
    if (!pawn) return HookResult.Handled;
    const cam = camera.findAll()[0];
    if (!cam) { cmd.reply(`${TAG} no cs_player_camera — run sm_cam_create first`); return HookResult.Handled; }
    const queued = camera.setControllingAngles(cam, cmd.arg(0) === "1", pawn.ref);
    cmd.reply(`${TAG} ${camera.PROBED_INPUTS.controlAngles} queued=${queued} (probe — input name is a guess)`);
    return HookResult.Handled;
  });

  command.admin("sm_cam_remove", ADMFLAG.ROOT, (cmd) => {
    cmd.reply(`${TAG} removed ${camera.removeOwned()} plugin-created camera(s)`);
    return HookResult.Handled;
  });

  // ── Bomb ────────────────────────────────────────────────────────────────────────────────────
  command.admin("sm_bomb_watch", ADMFLAG.GENERIC, (cmd) => {
    const on = cmd.argCount === 0 ? !bomb.isWatching() : cmd.arg(0) === "1";
    cmd.reply(`${TAG} bomb watch ${bomb.setWatching(on) ? "ON" : "off"} — logging ${bomb.WATCHED_EVENTS.join(", ")}`);
    return HookResult.Handled;
  });

  command.admin("sm_bomb_info", ADMFLAG.GENERIC, (cmd) => {
    const info = bomb.readBomb();
    if (!info.found) { cmd.reply(`${TAG} no weapon_c4 in the world`); return HookResult.Handled; }
    cmd.reply(
      `${TAG} c4 #${info.index} startedArming=${info.startedArming} armedTime=${info.armedTime} ` +
      `planted=${info.bombPlanted} viaUse=${info.isPlantingViaUse} carrier=${info.carrierName ?? "none"}`,
    );
    cmd.reply("  GetPlantFinishTime ~ armedTime; there is no m_flPlantStartTime field — cs_script derives it.");
    return HookResult.Handled;
  });

  command.admin("sm_bomb_abort", ADMFLAG.SLAY, (cmd) => {
    const result = bomb.abortPlant();
    cmd.reply(result.reason ? `${TAG} ${result.reason}` : `${TAG} AbortPlant queued=${result.queued} — confirm with sm_bomb_info (startedArming should clear)`);
    return HookResult.Handled;
  });

  // ── Movement ────────────────────────────────────────────────────────────────────────────────
  command.admin("sm_movetype", ADMFLAG.SLAY, (cmd) => {
    const pattern = cmd.argCount > 0 ? cmd.arg(0) : "@me";
    const targets = Player.target(pattern, cmd.callerSlot);
    if (targets.length === 0) { cmd.reply(`${TAG} no matching players`); return HookResult.Handled; }

    // No second argument = read-only. Listing the testable values on a bad token beats a bare
    // "invalid" when the update may well have added a value we do not know about.
    if (cmd.argCount < 2) {
      for (const p of targets) {
        const pawn = p.pawn;
        if (pawn?.isValid) cmd.reply(`${TAG} ${p.playerName}: ${movement.moveTypeName(movement.get(pawn))}`);
      }
      return HookResult.Handled;
    }
    const value = movement.parseMoveType(cmd.arg(1));
    if (value === null) {
      cmd.reply(`${TAG} unknown movetype "${cmd.arg(1)}" — try: ${movement.TESTABLE.map((t) => t.name).join(", ")}`);
      return HookResult.Handled;
    }
    for (const p of targets) {
      const pawn = p.pawn;
      if (!pawn?.isValid) continue;
      const r = movement.set(pawn, value);
      cmd.reply(`${TAG} ${p.playerName}: set ${movement.moveTypeName(value)} -> read back ${movement.moveTypeName(r.readBack)}${r.ok ? "" : " (MISMATCH)"}`);
    }
    return HookResult.Handled;
  });

  // ── Menus ───────────────────────────────────────────────────────────────────────────────────
  command.admin("sm_hud_menu", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_hud_menu needs an in-game caller`); return HookResult.Handled; }
    menus.showDemo(cmd.callerSlot, menus.parseStyle(cmd.arg(0)), log);
    return HookResult.Handled;
  });

  command.admin("sm_buymenu", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_buymenu needs an in-game caller`); return HookResult.Handled; }
    menus.showBuyMenu(cmd.callerSlot, menus.parseStyle(cmd.arg(0)), log);
    return HookResult.Handled;
  });

  // ── Drive OUR OWN published kit ──────────────────────────────────────────────────────────────
  // One command that exercises everything still unproven: create the entity against our workshop
  // addon, set dialog variables (the {s:} binding is untested), and turn on input capture so the
  // buttons become clickable and the click hook can fire.
  // ── sm_live — drive the PRODUCTION layout (s2script_hud_live.xml) ────────────────────────────
  //
  // This is the one to look at. Unlike the probe, every slot is a {s:} binding, so it renders as an
  // empty frame until filled — which is exactly why this command fills it before revealing it.
  const liveHud = ui.hud(LIVE_HUD);
  liveHud.onClick("motd_ok", (slot) => {
    liveHud.hide(slot, LIVE_PANELS.motd);
    liveHud.cursor(slot, false);
    Chat.toSlot(slot, "[hud] MOTD dismissed");
    log(`motd_ok clicked by slot ${slot} -> hidden, cursor released`);
  });

  // ── The live demo: HUD driven by real game state ─────────────────────────────────────────────
  demo = new LiveDemo(liveHud, log);

  // Kill feed from the real event. `player_death` carries the slots; the weapon is a string field.
  hook.on("player_death", (ev) => {
    if (demo.count === 0) return;                       // nobody watching — do no work at all
    const aSlot = ev.getPlayerSlot("attacker");
    const vSlot = ev.getPlayerSlot("userid");
    demo.pushKill({
      attacker: aSlot >= 0 ? (Player.fromSlot(aSlot)?.playerName ?? `slot ${aSlot}`) : "world",
      weapon: ev.getString("weapon").replace(/^weapon_/, ""),
      victim: vSlot >= 0 ? (Player.fromSlot(vSlot)?.playerName ?? `slot ${vSlot}`) : "?",
      headshot: ev.getBool("headshot"),
    });
  });

  command.admin("sm_hud", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_hud needs an in-game caller`); return HookResult.Handled; }
    const spawn = ui.createLayout();
    if (spawn !== null) { cmd.reply(`${TAG} ${spawn}`); return HookResult.Handled; }
    const slot = cmd.callerSlot;
    const arg = cmd.arg(0).toLowerCase();
    const want = arg === "" ? !demo.has(slot) : (arg === "1" || arg === "on");
    if (want) {
      demo.start(slot);
      cmd.reply(`${TAG} live HUD ON — round clock, scoreboard, your K/D, kill feed. ${demo.count} viewer(s)`);
      cmd.reply(`${TAG} sm_motd for the interactive panel; sm_hud off to stop`);
    } else {
      demo.stop(slot);
      cmd.reply(`${TAG} live HUD off`);
    }
    return HookResult.Handled;
  });

  command.admin("sm_motd", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_motd needs an in-game caller`); return HookResult.Handled; }
    const slot = cmd.callerSlot;
    const name = Player.fromSlot(slot)?.playerName ?? `slot ${slot}`;
    const errs: string[] = [];
    const put = (r: string | null) => { if (r) errs.push(r); };

    // Scoreboard + timer — always-on chrome.
    put(liveHud.set(slot, "timer_label", "ROUND"));
    put(liveHud.set(slot, "timer_value", "1:42"));
    put(liveHud.setMeter(slot, "timer", 70));
    put(liveHud.set(slot, "team_ct_name", "COUNTER-TERRORISTS"));
    put(liveHud.set(slot, "team_ct_score", "7"));
    put(liveHud.set(slot, "team_t_name", "TERRORISTS"));
    put(liveHud.set(slot, "team_t_score", "5"));

    // Player card.
    put(liveHud.set(slot, "pcard_name", name));
    put(liveHud.set(slot, "pcard_meta", "driven live by s2script"));
    put(liveHud.set(slot, "pcard_badge_t", "MVP"));
    put(liveHud.set(slot, "pcard_k", "18"));
    put(liveHud.set(slot, "pcard_d", "9"));
    put(liveHud.set(slot, "pcard_a", "4"));
    put(liveHud.set(slot, "pcard_hs", "61%"));
    put(liveHud.set(slot, "pcard_form_label", "LAST 5"));

    // One kill-feed row, and reveal it (rows start collapsed).
    put(liveHud.set(slot, "feed_0_a", name));
    put(liveHud.set(slot, "feed_0_w", "ak47"));
    put(liveHud.set(slot, "feed_0_v", "bot Kadeem"));
    put(liveHud.set(slot, "feed_0_t", "HS"));
    put(liveHud.show(slot, LIVE_PANELS.feed[0]));

    // MOTD — the interactive part. Reveal it and take the cursor so its button is clickable.
    put(liveHud.set(slot, "motd_title", "s2script HUD"));
    put(liveHud.set(slot, "motd_sub", "server-driven Panorama"));
    put(liveHud.set(slot, "motd_h0", "Layout"));
    put(liveHud.set(slot, "motd_p0", "workshop addon 3790153369, mounted per-client"));
    put(liveHud.set(slot, "motd_h1", "Drive"));
    put(liveHud.set(slot, "motd_p1", "ui.hud — SetHasClass / SetDialogVariableString"));
    put(liveHud.set(slot, "motd_h2", "Clicks"));
    put(liveHud.set(slot, "motd_p2", "CustomHudClickedReceiver detour -> ui.onCustomHudClicked"));
    put(liveHud.set(slot, "motd_note", "click OK to dismiss"));
    put(liveHud.set(slot, "motd_ok_t", "OK"));
    put(liveHud.show(slot, LIVE_PANELS.motd, { cursor: true }));

    cmd.reply(errs.length
      ? `${TAG} ${errs.length} call(s) refused; first: ${errs[0]}`
      : `${TAG} live HUD driven for ${name} — click OK to dismiss`);
    return HookResult.Handled;
  });

  command.admin("sm_kit", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_kit needs an in-game caller`); return HookResult.Handled; }
    const slot = cmd.callerSlot;

    kitHud.setText(slot, "s2_dialog_title", "s2script kit");
    kitHud.setText(slot, "s2_dialog_kicker", "LIVE");
    kitHud.setText(slot, "s2_dialog_body", `driven via ui.hud() for ${Player.fromSlot(slot)?.playerName ?? "you"}`);
    kitHud.setText(slot, "s2_btn_0_text", "Alpha");
    kitHud.setText(slot, "s2_btn_1_text", "Bravo");
    kitHud.setText(slot, "s2_btn_2_text", "Charlie");
    kitHud.setText(slot, "s2_btn_3_text", "Close");
    kitHud.setMeter(slot, "meter", 50);

    const err = kitHud.show(slot, "s2_dialog", { cursor: true });
    cmd.reply(err ? `${TAG} show refused: ${err}` : `${TAG} kit visible — click s2_btn_0..s2_btn_3`);
    return HookResult.Handled;
  });

  command.admin("sm_hud_show", ADMFLAG.GENERIC, (cmd) => {
    const slot = resolveOptionalSlot(cmd, 0, cmd.callerSlot);
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_show [target]`); return HookResult.Handled; }
    const err = kitHud.show(slot, "s2_dialog", { cursor: true });
    cmd.reply(err ? `${TAG} ${err}` : `${TAG} shown for slot ${slot} (class cleared + cursor on)`);
    return HookResult.Handled;
  });

  command.admin("sm_hud_hide", ADMFLAG.GENERIC, (cmd) => {
    const slot = resolveOptionalSlot(cmd, 0, cmd.callerSlot);
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_hide [target]`); return HookResult.Handled; }
    const err = kitHud.hide(slot, "s2_dialog");
    cmd.reply(err ? `${TAG} ${err}` : `${TAG} hidden for slot ${slot}`);
    return HookResult.Handled;
  });

  // ── The NATIVE panel: drive a map-authored custom_hud_layout with no engine call ───────────
  //
  // On a map whose cs_script already applied a class (e.g. editor/zoo/script_zoo's welcome_layout),
  // the HUDPanelHasClass_t entry exists and its m_eClassStatus is a plain int32 — so showing and
  // hiding a REAL Valve HUD panel is a raw write through two pointer hops. No CUtlString, no
  // engine call, none of the ABI problem that crashed the server.
  command.admin("sm_hud_states", ADMFLAG.GENERIC, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout in the world`); return HookResult.Handled; }
    cmd.reply(`${TAG} #${ref.index} playerStates=${playerStateCount(ref)}`);
    const only = cmd.argCount > 0 ? cmd.argInt(0, -1) : -1;
    let shown = 0;
    for (let slot = 0; slot < 12; slot++) {
      if (only >= 0 && slot !== only) continue;
      const entries = readPlayerClasses(ref, slot);
      if (entries.length === 0) continue;
      shown++;
      for (const e of entries) {
        cmd.reply(
          `  slot ${slot} entry ${e.index}: panelIdx=${e.panelIdIndex} classIdx=${e.classNameIndex} ` +
          `status=${e.status} (${e.status === 1 ? "HasClass" : e.status === 0 ? "DoesNotHaveClass" : "Undefined"})`,
        );
      }
    }
    if (shown === 0) cmd.reply("  no slot has a class entry yet — join the map and let its script apply one");
    return HookResult.Handled;
  });

  command.admin("sm_hud_cursor", ADMFLAG.GENERIC, (cmd) => {
    const slot = cmd.argCount > 1 ? cmd.argInt(1, cmd.callerSlot) : cmd.callerSlot;
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_cursor <0|1> [slot]`); return HookResult.Handled; }
    if (cmd.argCount === 0) {
      cmd.reply(`${TAG} usage: sm_hud_cursor <0|1> [slot]`);
      return HookResult.Handled;
    }
    const on = cmd.arg(0) === "1" || cmd.arg(0).toLowerCase() === "on";
    const err = kitHud.cursor(slot, on);
    cmd.reply(err ? `${TAG} refused: ${err}` : `${TAG} slot ${slot} cursor -> ${on}`);
    return HookResult.Handled;
  });

  command.admin("sm_hud_toggle", ADMFLAG.GENERIC, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout in the world`); return HookResult.Handled; }
    const slot = cmd.argCount > 0 ? cmd.argInt(0, cmd.callerSlot) : cmd.callerSlot;
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_toggle [slot] [entry] [0|1]  (needs an in-game caller or a slot)`); return HookResult.Handled; }
    const entryIdx = cmd.argInt(1, 0);
    const entries = readPlayerClasses(ref, slot);
    if (entries.length === 0) {
      cmd.reply(`${TAG} slot ${slot} has no class entry — the map's script applies one when you interact with the panel`);
      return HookResult.Handled;
    }
    const current = entries[entryIdx]?.status ?? -1;
    const want = cmd.argCount > 2 ? cmd.argInt(2, 1) : (current === 1 ? 0 : 1);
    const err = setPlayerClassStatus(ref, slot, entryIdx, want);
    cmd.reply(err
      ? `${TAG} refused: ${err}`
      : `${TAG} slot ${slot} entry ${entryIdx}: status ${current} -> ${want} (NATIVE panel, raw write + notify)`);
    return HookResult.Handled;
  });

  // ── The HUD that works ───────────────────────────────────────────────────────────────────────
  command.admin("sm_hud_demo", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_hud_demo needs an in-game caller`); return HookResult.Handled; }
    const arg = cmd.arg(0).toLowerCase();
    const want = arg === "" ? !hud.has(cmd.callerSlot) : arg === "1" || arg === "on";
    if (want) {
      hud.show(cmd.callerSlot);
      cmd.reply(`${TAG} demo HUD ON (${hud.count} viewer(s)) — centre-panel HTML, repainted every tick`);
      if (!Pawn.forSlot(cmd.callerSlot)?.isValid) {
        cmd.reply(`${TAG} note: you have no live pawn yet, so it stays blank until you spawn`);
      }
    } else {
      hud.hide(cmd.callerSlot);
      cmd.reply(`${TAG} demo HUD off — it clears within ~1s (the panel expires on its own)`);
    }
    return HookResult.Handled;
  });

  command.admin("sm_hud_demo_all", ADMFLAG.ROOT, (cmd) => {
    const on = cmd.arg(0) !== "0" && cmd.arg(0).toLowerCase() !== "off";
    if (!on) { hud.hideAll(); cmd.reply(`${TAG} demo HUD cleared for everyone`); return HookResult.Handled; }
    let n = 0;
    for (const p of Player.target("@all", cmd.callerSlot)) { hud.show(p.slot); n++; }
    cmd.reply(`${TAG} demo HUD ON for ${n} player(s)`);
    return HookResult.Handled;
  });

  command.admin("sm_hud_say", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.argCount === 0) { cmd.reply(`${TAG} usage: sm_hud_say <text>  (one-shot centre-panel message)`); return HookResult.Handled; }
    const text = cmd.argsFrom(0);
    // Deliberately NOT escaped: this is the command for trying centre-panel markup by hand
    // (<font color='#ff0000'>, <br>, fontSize-l/m/sm/s). Root-gated for that reason.
    let n = 0;
    for (const p of Player.target("@all", cmd.callerSlot)) if (sendOnce(p.slot, text)) n++;
    cmd.reply(`${TAG} sent to ${n} player(s)`);
    return HookResult.Handled;
  });

  // ── shared helpers ──────────────────────────────────────────────────────────────────────────

  /** The caller's own live pawn, or null after replying with why not. */
  function requireCallerPawn(cmd: CommandInvocation): Pawn | null {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} this command needs an in-game caller`); return null; }
    const pawn = Pawn.forSlot(cmd.callerSlot);
    if (!pawn?.isValid) { cmd.reply(`${TAG} you have no live pawn`); return null; }
    return pawn;
  }

  /** Parse EHudPanelClassStatus_t token for sm_hud_class diagnostics. */
  function parseClassStatus(token: string): number {
    if (token === "-1" || token.toLowerCase() === "undefined") return -1;
    const on = token === "1" || token.toLowerCase() === "true" || token.toLowerCase() === "on";
    return on ? 1 : 0;
  }

  /**
   * An optional trailing target argument, resolved to a single slot; -1 means "no target given",
   * which selects the GLOBAL variant of a Tier-B setter rather than the per-player one.
   */
  function resolveOptionalSlot(cmd: CommandInvocation, argIndex: number, fallback = -1): number {
    if (cmd.argCount <= argIndex) return fallback;
    const token = cmd.arg(argIndex);
    // A BARE INTEGER is taken as a raw slot index, not a target pattern. The setter bounds-checks
    // the slot against m_vecPlayerLayoutStates itself (cmp esi,[rdi+0x790]) and early-returns on a
    // bad one, so an unoccupied slot is safe — and it is the only way to exercise these calls from
    // RCON with nobody connected.
    if (/^\d+$/.test(token)) return Number(token);
    const targets = Player.target(token, cmd.callerSlot);
    return targets.length > 0 ? targets[0].slot : -1;
  }
}

/** A disconnecting player must not leave diff-cache entries behind. */
export function OnClientDisconnect(client: Client): void {
  demo.stop(client.slot);
  demo.forget(client.slot);
}

/** Collapse the live layout as players become active (panels default VISIBLE in the markup). */
export function OnClientActive(client: Client): void {
  demo.hideAll(client.slot);
  if (!config.getBool("auto_show")) return;
  const err = kitHud.show(client.slot, "s2_dialog", { cursor: true });
  log(err ? `auto-show refused for slot ${client.slot}: ${err}` : `auto-shown for slot ${client.slot}`);
}

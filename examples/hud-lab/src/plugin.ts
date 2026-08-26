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
 *   TIER B  needs a signature — the networked-vector setters (see gamedata/hud-lab.gamedata.jsonc)
 *   TIER C  needs a core change — inbound CS_UM_CustomHudClicked has no plugin-level path at all
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
import { plugin } from "@s2script/sdk/plugin";
import { ADMFLAG } from "@s2script/sdk/admin";
import { config } from "@s2script/sdk/config";
import { Engine } from "@s2script/sdk/unsafe";
import { Chat } from "@s2script/sdk/chat";
import { Player, Pawn } from "@s2script/cs2";
import type { CommandInvocation } from "@s2script/sdk/commands";

import * as layout from "./layout";
import * as camera from "./camera";
import * as bomb from "./bomb";
import * as movement from "./movement";
import * as menus from "./menus";
import * as tierb from "./tierb";
import * as panelctl from "./panelctl";
import { DemoHud, sendOnce } from "./demohud";
import { LAYOUT, STATE, LAYOUT_SIZE, globalStateField } from "./offsets";
import {
  readLayoutInfo, isInputCaptureEnabled, setInputCaptureEnabled, dumpWindow, probeLayout,
  readPlayerClasses, setPlayerClassStatus, playerStateCount,
  readPlayerInputCapture, setPlayerInputCapture,
} from "./hudstate";

/** Prefix every log line so a live server's console stays greppable. */
const TAG = "[hud-lab]";

/** Standing eye height above the pawn origin. The camera spawns here rather than at the feet so
 *  that "the view did not change" is a real signal instead of an obvious offset. */
const EYE_HEIGHT = 64;

export default plugin((ctx) => {
  const calls = tierb.resolveAll();
  const log = (line: string): void => console.log(`${TAG} ${line}`);

  // The working HUD. One shared per-frame subscription, armed only while someone is watching.
  const hud = new DemoHud(ctx);

  // Say at load why the HUD setters are unreachable — an operator should learn this from the boot
  // log, not from a command that appears to do nothing.
  for (const line of tierb.statusLines(calls)) log(line);

  bomb.install(ctx, log);

  // ── HUD CLICKS ───────────────────────────────────────────────────────────────────────────────
  // The last missing piece: a real click on a real HUD button, delivered server-side.
  //
  // `Engine.hook` returns null when the descriptor failed a load-time gate (signature miss, no
  // operator allow-list entry), so this is guarded once here rather than at the callback.
  const onClick = Engine.hook("onCustomHudClicked");
  console.log(`${TAG} onCustomHudClicked -> ${Engine.hookStatus("onCustomHudClicked")}`);
  if (onClick) {
    onClick((view) => {
      // `player` is the CONTROLLER that clicked, books-gated by the framework — the thunk aims the
      // receiver at the click's own argument rather than at the detour's `this`.
      // There is no Player.fromEntity, so match the receiver's entity index against the connected
      // controllers. Index+id together, not index alone: a recycled slot must not impersonate the
      // clicker.
      const ref = view.player;
      const who = ref
        ? Player.all().find((p) => p.ref.index === ref.index && p.ref.id === ref.id) ?? null
        : null;
      const slot = who?.slot ?? -1;
      const name = who?.playerName ?? (slot >= 0 ? `slot ${slot}` : "<unknown>");
      log(`CLICK button="${view.buttonId}" by ${name} (slot ${slot})`);
      if (slot >= 0) {
        Chat.toSlot(slot, `[hud] you clicked: ${view.buttonId}`);
        // ACT on it. On a stock map there is no cs_script to respond, so the dialog stays up
        // unless we hide it ourselves — which is exactly what Valve's own handler does
        // (OnCustomHudClicked -> HideWelcome). This is the whole point of having the hook.
        const ref = layout.preferred();
        if (ref && view.buttonId === panelctl.ZOO_WELCOME.buttonId) {
          const err = panelctl.hide(calls, ref, slot, panelctl.ZOO_WELCOME);
          log(err ? `  hide refused: ${err}` : `  dismissed for slot ${slot}`);
        }
      }
      // NO HookResult returned — this is an observer. Suppressing would stop the click reaching
      // every other registered receiver, including the map's own cs_script (which is what makes
      // the zoo map's Dismiss button actually dismiss).
    });
  }


  // ── Map start ───────────────────────────────────────────────────────────────────────────────
  // Auto-creation is OFF by default: a bad `layout` value would then run on every map load, and a
  // spawn failure at map start is far harder to read than one at a command prompt.
  // NO auto-create at OnMapStart. Tried it; it SEGFAULTED the server, immediately after
  // "[s2script] map start" with the crash breadcrumb naming this plugin. Spawning a
  // custom_hud_layout that early is not safe — the entity/resource systems are still coming up.
  //
  // Both working reference implementations (cs2-customhud, CustomHudProbeSW2) spawn on COMMAND for
  // the same reason. `sm_hud_create` is the supported path.

  // ── Status ──────────────────────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_status", ADMFLAG.GENERIC, (cmd) => {
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

    cmd.reply(`${TAG} ── TIER B (armed via the utlstring arg kind) ──`);
    for (const line of tierb.statusLines(calls)) cmd.reply(line);

    cmd.reply(`${TAG} ── TIER C (needs a core change) ──`);
    cmd.reply("  Instance.OnCustomHudClicked / CS_UM_CustomHudClicked (id 390): UNREACHABLE.");
    cmd.reply("  UserMessages.onPre is outbound-only, and the inbound-hook shape vocabulary has no");
    cmd.reply("  two-slot `this_i64` for a (this, const Msg&) handler. Button clicks cannot be");
    cmd.reply("  observed by any plugin until core grows an inbound-usermessage capability.");
  });

  // ── Offset verification ─────────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_probe", ADMFLAG.ROOT, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout in the world — run sm_hud_create first`); return; }
    // Default window brackets the global state, the field this plugin actually writes.
    const start = cmd.argInt(0, LAYOUT.globalLayoutState);
    const len = Math.min(cmd.argInt(1, 64), 256);
    cmd.reply(`${TAG} entity #${ref.index}, sizeof=${LAYOUT_SIZE}, window +${start}..+${start + len}`);
    cmd.reply(`  m_bInputCaptureEnabled is expected at +${globalStateField(STATE.inputCaptureEnabled)}`);
    for (const line of dumpWindow(ref, start, len)) cmd.reply(`  ${line}`);
  });

  // ── Why does the MAP's entity render and ours not? ──────────────────────────────────────────
  // A full byte-diff of a map-authored custom_hud_layout against one we spawned. Everything the
  // plugin reads by name already matches (layout pointer set, playerStates=12, same counts), so the
  // difference — whatever makes the client draw one and ignore the other — is in a field this
  // plugin has no name for. This finds it without needing one.
  ctx.commands.registerAdmin("sm_hud_diff", ADMFLAG.ROOT, (cmd) => {
    const all = layout.findAll();
    const mine = all.find((e) => e.name === layout.OWNED_TARGETNAME);
    const theirs = all.find((e) => e.name !== layout.OWNED_TARGETNAME);
    if (!mine || !theirs) {
      cmd.reply(`${TAG} need BOTH a map-authored and a plugin-created layout in the world`);
      cmd.reply(`${TAG} load editor/zoo/script_zoo, then sm_hud_create`);
      return;
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
    if (runs.length === 0) { cmd.reply("  IDENTICAL — the difference is not in this entity's memory"); return; }
    cmd.reply(`  ${runs.length} differing run(s):`);
    // Cap the reply: a full dump would flood rcon and the interesting fields are the early ones.
    for (const r of runs.slice(0, 24)) cmd.reply(r);
    if (runs.length > 24) cmd.reply(`  … ${runs.length - 24} more`);
  });

  // ── Layout entity lifecycle ─────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_create", ADMFLAG.ROOT, (cmd) => {
    const name = cmd.argCount > 0 ? cmd.argsFrom(0) : config.getString("layout");
    const result = layout.create(name);
    if (result.error) { cmd.reply(`${TAG} ${result.error}`); return; }
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
  });

  ctx.commands.registerAdmin("sm_hud_list", ADMFLAG.GENERIC, (cmd) => {
    const all = layout.findAll();
    if (all.length === 0) { cmd.reply(`${TAG} no custom_hud_layout entities`); return; }
    for (const ref of all) {
      const info = readLayoutInfo(ref);
      const owned = info.targetname === layout.OWNED_TARGETNAME ? " (ours)" : " (map-authored)";
      cmd.reply(
        `${TAG} #${info.index}${owned} layout=${info.hasLayoutString ? "set" : "none"} ` +
        `panels=${info.panelIds} classes=${info.classNames} vars=${info.dialogVariableNames} ` +
        `capture=${info.inputCapture}`,
      );
    }
  });

  ctx.commands.registerAdmin("sm_hud_remove", ADMFLAG.ROOT, (cmd) => {
    cmd.reply(`${TAG} removed ${layout.removeOwned()} plugin-created layout entit(ies)`);
  });

  // ── Tier A: input capture ───────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_capture", ADMFLAG.ROOT, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout — run sm_hud_create first`); return; }
    if (cmd.argCount === 0) {
      cmd.reply(`${TAG} IsInputCaptureEnabled = ${isInputCaptureEnabled(ref)}`);
      return;
    }
    const on = cmd.arg(0) === "1" || cmd.arg(0).toLowerCase() === "true";
    const err = setInputCaptureEnabled(ref, on);
    cmd.reply(err ? `${TAG} SetInputCaptureEnabled(${on}) refused — ${err}` : `${TAG} SetInputCaptureEnabled(${on}) OK (read back ${on})`);
  });

  // ── Tier B: classes and dialog variables ────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_class", ADMFLAG.ROOT, (cmd) => {
    if (cmd.argCount < 3) { cmd.reply(`${TAG} usage: sm_hud_class <panelId> <className> <1|0|-1> [target]`); return; }
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout — run sm_hud_create first`); return; }
    const slot = resolveOptionalSlot(cmd, 3, cmd.callerSlot);
    const err = tierb.setHasClass(calls, ref, slot, cmd.arg(0), cmd.arg(1), tierb.parseClassStatus(cmd.arg(2)));
    if (err) { cmd.reply(`${TAG} ${err}`); return; }
    const info = readLayoutInfo(ref);
    cmd.reply(`${TAG} setHasClassForPlayer(slot ${slot}, "${cmd.arg(0)}", "${cmd.arg(1)}", ${cmd.arg(2)})`);
    cmd.reply(`  panelIds=${info.panelIds} classNames=${info.classNames} — a RISING count means the engine interned our name`);
  });

  ctx.commands.registerAdmin("sm_hud_var", ADMFLAG.ROOT, (cmd) => {
    if (cmd.argCount < 3) { cmd.reply(`${TAG} usage: sm_hud_var <panelId> <variableName> <value> [target]`); return; }
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout — run sm_hud_create first`); return; }
    const slot = resolveOptionalSlot(cmd, 3, cmd.callerSlot);
    const err = tierb.setDialogVariable(calls, ref, slot, cmd.arg(0), cmd.arg(1), cmd.argsFrom(2));
    if (err) { cmd.reply(`${TAG} ${err}`); return; }
    const info = readLayoutInfo(ref);
    cmd.reply(`${TAG} setDialogVariableStringForPlayer(slot ${slot}, "${cmd.arg(0)}", "${cmd.arg(1)}", "${cmd.argsFrom(2)}")`);
    cmd.reply(`  dialogVars=${info.dialogVariableNames} — a RISING count means the engine interned our variable`);
  });

  // ── Tier A: the SetHUDVisibility entity input ───────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_visible", ADMFLAG.SLAY, (cmd) => {
    if (cmd.argCount < 2) { cmd.reply(`${TAG} usage: sm_hud_visible <target> <0|1>`); return; }
    const on = cmd.arg(1) === "1";
    let n = 0;
    for (const p of Player.target(cmd.arg(0), cmd.callerSlot)) {
      const pawn = p.pawn;
      // The datadesc names this input on CBasePlayerPawn, so it is fired at the PAWN, not the
      // controller — a common way to get a silent no-op here.
      if (pawn?.isValid && pawn.ref.acceptInput("SetHUDVisibility", on ? "1" : "0")) n++;
    }
    cmd.reply(`${TAG} queued SetHUDVisibility=${on ? 1 : 0} on ${n} pawn(s) — queued, not confirmed`);
  });

  // ── Camera ──────────────────────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_cam_create", ADMFLAG.ROOT, (cmd) => {
    const pawn = requireCallerPawn(cmd);
    if (!pawn) return;
    const origin = pawn.origin;
    const angles = pawn.angles;
    if (!origin || !angles) { cmd.reply(`${TAG} cannot read caller transform`); return; }
    const result = camera.create([origin.x, origin.y, origin.z + EYE_HEIGHT], [angles.x, angles.y, angles.z]);
    cmd.reply(result.error ? `${TAG} ${result.error}` : `${TAG} created cs_player_camera #${result.ref?.index}`);
  });

  ctx.commands.registerAdmin("sm_cam_enable", ADMFLAG.ROOT, (cmd) => {
    const pawn = requireCallerPawn(cmd);
    if (!pawn) return;
    const cam = camera.findAll()[0];
    if (!cam) { cmd.reply(`${TAG} no cs_player_camera — run sm_cam_create first`); return; }
    const on = cmd.arg(0) === "1";
    const queued = camera.setEnabled(cam, on, pawn.ref);
    cmd.reply(`${TAG} ${on ? "Enable" : "Disable"} queued=${queued} — an unknown input queues then drops silently, so judge by your view, not this line`);
  });

  ctx.commands.registerAdmin("sm_cam_angles", ADMFLAG.ROOT, (cmd) => {
    const pawn = requireCallerPawn(cmd);
    if (!pawn) return;
    const cam = camera.findAll()[0];
    if (!cam) { cmd.reply(`${TAG} no cs_player_camera — run sm_cam_create first`); return; }
    const queued = camera.setControllingAngles(cam, cmd.arg(0) === "1", pawn.ref);
    cmd.reply(`${TAG} ${camera.PROBED_INPUTS.controlAngles} queued=${queued} (probe — input name is a guess)`);
  });

  ctx.commands.registerAdmin("sm_cam_remove", ADMFLAG.ROOT, (cmd) => {
    cmd.reply(`${TAG} removed ${camera.removeOwned()} plugin-created camera(s)`);
  });

  // ── Bomb ────────────────────────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_bomb_watch", ADMFLAG.GENERIC, (cmd) => {
    const on = cmd.argCount === 0 ? !bomb.isWatching() : cmd.arg(0) === "1";
    cmd.reply(`${TAG} bomb watch ${bomb.setWatching(on) ? "ON" : "off"} — logging ${bomb.WATCHED_EVENTS.join(", ")}`);
  });

  ctx.commands.registerAdmin("sm_bomb_info", ADMFLAG.GENERIC, (cmd) => {
    const info = bomb.readBomb();
    if (!info.found) { cmd.reply(`${TAG} no weapon_c4 in the world`); return; }
    cmd.reply(
      `${TAG} c4 #${info.index} startedArming=${info.startedArming} armedTime=${info.armedTime} ` +
      `planted=${info.bombPlanted} viaUse=${info.isPlantingViaUse} carrier=${info.carrierName ?? "none"}`,
    );
    cmd.reply("  GetPlantFinishTime ~ armedTime; there is no m_flPlantStartTime field — cs_script derives it.");
  });

  ctx.commands.registerAdmin("sm_bomb_abort", ADMFLAG.SLAY, (cmd) => {
    const result = bomb.abortPlant();
    cmd.reply(result.reason ? `${TAG} ${result.reason}` : `${TAG} AbortPlant queued=${result.queued} — confirm with sm_bomb_info (startedArming should clear)`);
  });

  // ── Movement ────────────────────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_movetype", ADMFLAG.SLAY, (cmd) => {
    const pattern = cmd.argCount > 0 ? cmd.arg(0) : "@me";
    const targets = Player.target(pattern, cmd.callerSlot);
    if (targets.length === 0) { cmd.reply(`${TAG} no matching players`); return; }

    // No second argument = read-only. Listing the testable values on a bad token beats a bare
    // "invalid" when the update may well have added a value we do not know about.
    if (cmd.argCount < 2) {
      for (const p of targets) {
        const pawn = p.pawn;
        if (pawn?.isValid) cmd.reply(`${TAG} ${p.playerName}: ${movement.moveTypeName(movement.get(pawn))}`);
      }
      return;
    }
    const value = movement.parseMoveType(cmd.arg(1));
    if (value === null) {
      cmd.reply(`${TAG} unknown movetype "${cmd.arg(1)}" — try: ${movement.TESTABLE.map((t) => t.name).join(", ")}`);
      return;
    }
    for (const p of targets) {
      const pawn = p.pawn;
      if (!pawn?.isValid) continue;
      const r = movement.set(pawn, value);
      cmd.reply(`${TAG} ${p.playerName}: set ${movement.moveTypeName(value)} -> read back ${movement.moveTypeName(r.readBack)}${r.ok ? "" : " (MISMATCH)"}`);
    }
  });

  // ── Menus ───────────────────────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_menu", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_hud_menu needs an in-game caller`); return; }
    menus.showDemo(cmd.callerSlot, menus.parseStyle(cmd.arg(0)), log);
  });

  ctx.commands.registerAdmin("sm_buymenu", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_buymenu needs an in-game caller`); return; }
    menus.showBuyMenu(cmd.callerSlot, menus.parseStyle(cmd.arg(0)), log);
  });

  // ── Drive OUR OWN published kit ──────────────────────────────────────────────────────────────
  // One command that exercises everything still unproven: create the entity against our workshop
  // addon, set dialog variables (the {s:} binding is untested), and turn on input capture so the
  // buttons become clickable and the click hook can fire.
  ctx.commands.registerAdmin("sm_kit", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_kit needs an in-game caller`); return; }
    const slot = cmd.callerSlot;

    // Always target OUR entity, never a map-authored one — layout.preferred() favours the map's,
    // which is what made an earlier "success" actually be Valve's panel rather than ours.
    let ref = layout.ownEntity();
    if (!ref) {
      const created = layout.create(panelctl.S2_KIT.layout);
      if (created.error) { cmd.reply(`${TAG} ${created.error}`); return; }
      ref = created.ref;
      cmd.reply(`${TAG} created entity #${ref?.index} -> ${panelctl.S2_KIT.layout}`);
    }
    if (!ref) { cmd.reply(`${TAG} no layout entity`); return; }

    // Dialog variables: the last untested mechanism. Each targets a panel id from our own markup.
    const vars: Array<[string, string, string]> = [
      ["s2_dialog_title", "title", "s2script kit"],
      ["s2_dialog_kicker", "kicker", "LIVE"],
      ["s2_dialog_body", "body", `driven from the server for ${Player.fromSlot(slot)?.playerName ?? "you"}`],
      ["s2_btn_0_text", "btn0", "Alpha"],
      ["s2_btn_1_text", "btn1", "Bravo"],
      ["s2_btn_2_text", "btn2", "Charlie"],
      ["s2_btn_3_text", "btn3", "Close"],
    ];
    let set = 0;
    for (const [panelId, name, value] of vars) {
      const err = tierb.setDialogVariable(calls, ref, slot, panelId, name, value);
      if (err) { cmd.reply(`${TAG} setDialogVariable(${panelId}) refused: ${err}`); break; }
      set++;
    }
    cmd.reply(`${TAG} set ${set}/${vars.length} dialog variable(s)`);

    // Input capture: without it there is no cursor and no click detection at all.
    const capErr = setPlayerInputCapture(ref, slot, true);
    cmd.reply(capErr ? `${TAG} input capture refused: ${capErr}` : `${TAG} input capture ON — buttons should be clickable`);
    cmd.reply(`${TAG} click a button; ids are s2_btn_0..s2_btn_3`);
  });

  // ── Show / hide a panel: the sequence that actually works ───────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_show", ADMFLAG.GENERIC, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout — run sm_hud_create first`); return; }
    const slot = resolveOptionalSlot(cmd, 0, cmd.callerSlot);
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_show [target]  (needs an in-game caller or a slot)`); return; }
    const err = panelctl.show(calls, ref, slot, panelctl.ZOO_WELCOME);
    cmd.reply(err ? `${TAG} ${err}` : `${TAG} shown for slot ${slot} (class cleared + cursor on)`);
  });

  ctx.commands.registerAdmin("sm_hud_hide", ADMFLAG.GENERIC, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout — run sm_hud_create first`); return; }
    const slot = resolveOptionalSlot(cmd, 0, cmd.callerSlot);
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_hide [target]`); return; }
    const err = panelctl.hide(calls, ref, slot, panelctl.ZOO_WELCOME);
    cmd.reply(err ? `${TAG} ${err}` : `${TAG} hidden for slot ${slot}`);
  });

  // Auto-show on spawn, the way Valve's script does it (OnPlayerActivate -> ShowWelcome).
  ctx.clients.onActive((client) => {
    const ref = layout.preferred();
    if (!ref || !config.getBool("auto_show")) return;
    const err = panelctl.show(calls, ref, client.slot, panelctl.ZOO_WELCOME);
    log(err ? `auto-show refused for slot ${client.slot}: ${err}` : `auto-shown for slot ${client.slot}`);
  });

  // ── The NATIVE panel: drive a map-authored custom_hud_layout with no engine call ───────────
  //
  // On a map whose cs_script already applied a class (e.g. editor/zoo/script_zoo's welcome_layout),
  // the HUDPanelHasClass_t entry exists and its m_eClassStatus is a plain int32 — so showing and
  // hiding a REAL Valve HUD panel is a raw write through two pointer hops. No CUtlString, no
  // engine call, none of the ABI problem that crashed the server.
  ctx.commands.registerAdmin("sm_hud_states", ADMFLAG.GENERIC, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout in the world`); return; }
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
  });

  ctx.commands.registerAdmin("sm_hud_cursor", ADMFLAG.GENERIC, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout in the world`); return; }
    const slot = cmd.argCount > 1 ? cmd.argInt(1, cmd.callerSlot) : cmd.callerSlot;
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_cursor <0|1> [slot]`); return; }
    if (cmd.argCount === 0) {
      cmd.reply(`${TAG} slot ${slot} inputCapture=${readPlayerInputCapture(ref, slot)} (per-player), global=${isInputCaptureEnabled(ref)}`);
      return;
    }
    const on = cmd.arg(0) === "1" || cmd.arg(0).toLowerCase() === "on";
    const err = setPlayerInputCapture(ref, slot, on);
    cmd.reply(err ? `${TAG} refused: ${err}` : `${TAG} slot ${slot} inputCapture -> ${on} (mouse cursor ${on ? "ON" : "off"})`);
  });

  ctx.commands.registerAdmin("sm_hud_toggle", ADMFLAG.GENERIC, (cmd) => {
    const ref = layout.preferred();
    if (!ref) { cmd.reply(`${TAG} no custom_hud_layout in the world`); return; }
    const slot = cmd.argCount > 0 ? cmd.argInt(0, cmd.callerSlot) : cmd.callerSlot;
    if (slot < 0) { cmd.reply(`${TAG} usage: sm_hud_toggle [slot] [entry] [0|1]  (needs an in-game caller or a slot)`); return; }
    const entryIdx = cmd.argInt(1, 0);
    const entries = readPlayerClasses(ref, slot);
    if (entries.length === 0) {
      cmd.reply(`${TAG} slot ${slot} has no class entry — the map's script applies one when you interact with the panel`);
      return;
    }
    const current = entries[entryIdx]?.status ?? -1;
    const want = cmd.argCount > 2 ? cmd.argInt(2, 1) : (current === 1 ? 0 : 1);
    const err = setPlayerClassStatus(ref, slot, entryIdx, want);
    cmd.reply(err
      ? `${TAG} refused: ${err}`
      : `${TAG} slot ${slot} entry ${entryIdx}: status ${current} -> ${want} (NATIVE panel, raw write + notify)`);
  });

  // ── The HUD that works ───────────────────────────────────────────────────────────────────────
  ctx.commands.registerAdmin("sm_hud_demo", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} sm_hud_demo needs an in-game caller`); return; }
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
  });

  ctx.commands.registerAdmin("sm_hud_demo_all", ADMFLAG.ROOT, (cmd) => {
    const on = cmd.arg(0) !== "0" && cmd.arg(0).toLowerCase() !== "off";
    if (!on) { hud.hideAll(); cmd.reply(`${TAG} demo HUD cleared for everyone`); return; }
    let n = 0;
    for (const p of Player.target("@all", cmd.callerSlot)) { hud.show(p.slot); n++; }
    cmd.reply(`${TAG} demo HUD ON for ${n} player(s)`);
  });

  ctx.commands.registerAdmin("sm_hud_say", ADMFLAG.GENERIC, (cmd) => {
    if (cmd.argCount === 0) { cmd.reply(`${TAG} usage: sm_hud_say <text>  (one-shot centre-panel message)`); return; }
    const text = cmd.argsFrom(0);
    // Deliberately NOT escaped: this is the command for trying centre-panel markup by hand
    // (<font color='#ff0000'>, <br>, fontSize-l/m/sm/s). Root-gated for that reason.
    let n = 0;
    for (const p of Player.target("@all", cmd.callerSlot)) if (sendOnce(p.slot, text)) n++;
    cmd.reply(`${TAG} sent to ${n} player(s)`);
  });

  // ── shared helpers ──────────────────────────────────────────────────────────────────────────

  /** The caller's own live pawn, or null after replying with why not. */
  function requireCallerPawn(cmd: CommandInvocation): Pawn | null {
    if (cmd.callerSlot < 0) { cmd.reply(`${TAG} this command needs an in-game caller`); return null; }
    const pawn = Pawn.forSlot(cmd.callerSlot);
    if (!pawn?.isValid) { cmd.reply(`${TAG} you have no live pawn`); return null; }
    return pawn;
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
});

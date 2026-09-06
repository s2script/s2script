import { Clients, command, HookResult } from "@s2script/sdk";
import type { Client, CommandInvocation } from "@s2script/sdk";
import { hudkit } from "@s2script/cs2";
import type { Modal, ModalView, UiErrorCode, UiResult } from "@s2script/cs2";

const TAG = "[ui-b]";
const FOCUS = { mode: "exclusive", priority: 20 } as const;
let modal: Modal | null = null;
let modalClaimError: UiErrorCode | null = null;
const views = new Map<number, ModalView>();
let providerCalls = 0;
let clicks = 0;
let closeClicks = 0;
let openAttempts = 0;
let openSuccesses = 0;
let openFailures = 0;
let invalidationRequests = 0;

function claimModal(): void {
  if (modal) return;
  const result = hudkit.tryModal({
    title: "Plugin B — exclusive focus",
    subtitle: "Priority 20; closing should restore plugin A",
    rows: (slot) => {
      providerCalls++;
      return [{
        id: "b-action",
        a: "Count one B click",
        b: `slot=${slot}`,
        c: `provider=${providerCalls}`,
      }];
    },
    onPick: (slot, index, row, view) => {
      if (!view.isValid() || row.id !== "b-action") return;
      clicks++;
      console.log(`${TAG} click ${JSON.stringify({ slot, index, stableId: row.id, clicks })}`);
    },
    buttons: [{
      text: "Close B",
      variant: "bad",
      onClick: (slot, view) => {
        closeClicks++;
        view.close();
        views.delete(slot);
        console.log(`${TAG} close-button ${JSON.stringify({ slot, closeClicks })}`);
      },
    }],
    pageSize: 1,
    width: "sm",
  });
  if (result.ok) {
    modal = result.value;
    modalClaimError = null;
  } else {
    modalClaimError = result.error.code;
  }
}

function resolveSlot(cmd: CommandInvocation, argument = 0): number | null {
  const slot = cmd.argCount > argument ? cmd.argInt(argument, -1) : cmd.callerSlot;
  const client = Clients.fromSlot(slot);
  if (slot < 0 || !client?.isValid() || client.signonState !== 6) {
    cmd.reply(`${TAG} invalid slot ${slot}; pass an active zero-based player slot`);
    return null;
  }
  return slot;
}

function summarize<T>(result: UiResult<T>): { ok: true } | {
  ok: false;
  code: UiErrorCode;
  message: string;
} {
  return result.ok
    ? { ok: true }
    : { ok: false, code: result.error.code, message: result.error.message };
}

function closeSlot(slot: number): void {
  const view = views.get(slot);
  if (view?.isValid()) view.close();
  views.delete(slot);
}

function releaseAll(): void {
  for (const slot of [...views.keys()]) closeSlot(slot);
  modal?.release();
  modal = null;
}

function status(slot: number | null) {
  const client = slot === null ? null : Clients.fromSlot(slot);
  const view = slot === null ? null : views.get(slot) ?? null;
  return {
    plugin: "@demo/ui-multiplugin-b",
    focus: FOCUS,
    providerCalls,
    clicks,
    closeClicks,
    invalidationRequests,
    opens: { attempts: openAttempts, successes: openSuccesses, failures: openFailures },
    claim: { modal: modal !== null, error: modalClaimError },
    client: client && {
      slot: client.slot,
      valid: client.isValid(),
      userId: client.userId,
      steamId: client.steamId,
      signonState: client.signonState,
    },
    view: view && {
      valid: view.isValid(),
      open: view.isOpen(),
      lastUpdateResult: view.lastUpdateResult(),
    },
  };
}

export function OnPluginStart(): void {
  claimModal();

  command("sm_ui_b_open", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    claimModal();
    if (!modal) {
      cmd.reply(`${TAG} ${JSON.stringify({ ok: false, code: modalClaimError ?? "Released" })}`);
      return HookResult.Handled;
    }
    openAttempts++;
    const result = modal.tryOpenResult(slot, { cursor: true, focus: FOCUS });
    if (result.ok) {
      openSuccesses++;
      views.set(slot, result.value);
    } else openFailures++;
    cmd.reply(`${TAG} ${JSON.stringify({
      action: "open",
      slot,
      result: summarize(result),
      focus: FOCUS,
      providerCalls,
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_b_refresh", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    if (!modal) {
      cmd.reply(`${TAG} ${JSON.stringify({ ok: false, code: modalClaimError ?? "Released" })}`);
      return HookResult.Handled;
    }
    const result = modal.tryRefresh(slot);
    cmd.reply(`${TAG} ${JSON.stringify({
      action: "refresh",
      slot,
      result: summarize(result),
      providerCalls,
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_b_invalidate", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    const view = views.get(slot);
    if (!view?.isValid()) {
      cmd.reply(`${TAG} ${JSON.stringify({ ok: false, code: "Released", slot })}`);
      return HookResult.Handled;
    }
    const before = providerCalls;
    invalidationRequests++;
    view.invalidate();
    cmd.reply(`${TAG} ${JSON.stringify({
      action: "invalidate",
      ok: true,
      slot,
      providerCallsBefore: before,
      providerCallsAfter: providerCalls,
      immediateProviderDelta: providerCalls - before,
      lastUpdateResult: view.lastUpdateResult(),
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_b_close", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    closeSlot(slot);
    cmd.reply(`${TAG} ${JSON.stringify({ action: "close", ok: true, slot })}`);
    return HookResult.Handled;
  });

  command("sm_ui_b_release", (cmd) => {
    releaseAll();
    cmd.reply(`${TAG} ${JSON.stringify({ action: "release", ok: true, views: views.size })}`);
    return HookResult.Handled;
  });

  command("sm_ui_b_status", (cmd) => {
    const requested = cmd.argCount > 0 ? cmd.argInt(0, -1) : cmd.callerSlot;
    cmd.reply(`${TAG} ${JSON.stringify(status(requested >= 0 ? requested : null))}`);
    return HookResult.Handled;
  });
}

export function OnClientDisconnect(client: Client): void {
  closeSlot(client.slot);
}

export function OnPluginEnd(): void {
  releaseAll();
}

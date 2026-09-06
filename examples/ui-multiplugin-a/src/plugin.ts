import { Clients, command, HookResult } from "@s2script/sdk";
import type { Client, CommandInvocation } from "@s2script/sdk";
import { hudkit } from "@s2script/cs2";
import type {
  Badge,
  BadgeView,
  Modal,
  ModalView,
  UiErrorCode,
  UiResult,
  UiSurfaceHandle,
} from "@s2script/cs2";

const TAG = "[ui-a]";
const FOCUS = { mode: "exclusive", priority: 10 } as const;

interface DomainRecord {
  readonly id: string;
  readonly baseLabel: string;
  readonly label: string;
}

interface LastClick {
  readonly slot: number;
  readonly absoluteIndex: number;
  readonly stableId: string | null;
  readonly paintedLabel: string;
  readonly currentLabel: string | null;
  readonly currentSourceIdAtIndex: string | null;
  readonly sourceChangedAtIndex: boolean;
  readonly currentDomainFound: boolean;
  readonly sourceRevision: number;
  readonly lastSuccessfulSyncOperationRevision: number | null;
}

const domain = new Map<string, DomainRecord>([
  ["alpha", { id: "alpha", baseLabel: "Alpha", label: "Alpha" }],
  ["bravo", { id: "bravo", baseLabel: "Bravo", label: "Bravo" }],
  ["charlie", { id: "charlie", baseLabel: "Charlie", label: "Charlie" }],
]);
let order = ["alpha", "bravo", "charlie"];
let sourceRevision = 0;
let providerCalls = 0;
let modal: Modal | null = null;
let badge: Badge | null = null;
let modalClaimError: UiErrorCode | null = null;
let badgeClaimError: UiErrorCode | null = null;
const modalViews = new Map<number, ModalView>();
const badgeViews = new Map<number, BadgeView>();
const banners = new Map<number, UiSurfaceHandle>();
// Successful synchronous open/refresh operations may be covered and perform no paint.
const successfulSyncOperationRevision = new Map<number, number>();
const lastProvidedRevision = new Map<number, number>();
let clicks = 0;
let revalidatedClicks = 0;
let rejectedClicks = 0;
let lastClick: LastClick | null = null;
let invalidationRequests = 0;
const ownerCounters = {
  attempts: 0,
  acquired: 0,
  busy: 0,
  failed: 0,
  disposed: 0,
  unexpectedSecondAcquire: 0,
};

function rows(slot: number) {
  providerCalls++;
  lastProvidedRevision.set(slot, sourceRevision);
  return order.map((id) => {
    const record = domain.get(id);
    return {
      id,
      a: record?.label ?? `missing:${id}`,
      b: `id=${id}`,
      c: `source=${sourceRevision}`,
    };
  });
}

function claimPools(): void {
  if (!modal) {
    const result = hudkit.tryModal({
      title: "Plugin A — keyed rows",
      subtitle: (slot) => `slot ${slot} · source revision ${sourceRevision}`,
      rows,
      detail: (_slot, row, index) => [
        `painted id: ${row?.id ?? "none"}`,
        `absolute index: ${index}`,
        "Reorder source, then click before repaint.",
      ],
      onPick: (slot, absoluteIndex, row, view) => {
        clicks++;
        const stableId = row.id ?? null;
        const current = stableId === null ? null : domain.get(stableId) ?? null;
        const currentSourceIdAtIndex = order[absoluteIndex] ?? null;
        const validClient = Clients.fromSlot(slot);
        const accepted = view.isValid() && validClient?.isValid() === true && current !== null;
        if (accepted) revalidatedClicks++;
        else rejectedClicks++;
        lastClick = {
          slot,
          absoluteIndex,
          stableId,
          paintedLabel: row.a,
          currentLabel: current?.label ?? null,
          currentSourceIdAtIndex,
          sourceChangedAtIndex: stableId !== currentSourceIdAtIndex,
          currentDomainFound: current !== null,
          sourceRevision,
          lastSuccessfulSyncOperationRevision: successfulSyncOperationRevision.get(slot) ?? null,
        };
        console.log(`${TAG} click ${JSON.stringify({ accepted, ...lastClick })}`);
      },
      buttons: [{
        text: "Close A",
        variant: "ghost",
        onClick: (slot, view) => {
          clicks++;
          view.close();
          modalViews.delete(slot);
          const badgeView = badgeViews.get(slot);
          if (badgeView?.isValid()) badgeView.hide();
          badgeViews.delete(slot);
          disposeBanner(slot);
          console.log(`${TAG} close-button ${JSON.stringify({ slot, clicks })}`);
        },
      }],
      pageSize: 3,
      width: "md",
    });
    if (result.ok) {
      modal = result.value;
      modalClaimError = null;
    } else {
      modalClaimError = result.error.code;
    }
  }
  if (!badge) {
    const result = hudkit.tryBadge({ corner: "tl", title: "PLUGIN A", accent: "accent" });
    if (result.ok) {
      badge = result.value;
      badgeClaimError = null;
    } else {
      badgeClaimError = result.error.code;
    }
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

function ownBanner(slot: number): { ok: true; reused: boolean } | {
  ok: false;
  code: UiErrorCode;
  message: string;
} {
  const existing = banners.get(slot);
  if (existing?.isValid()) return { ok: true, reused: true };
  if (existing) banners.delete(slot);
  ownerCounters.attempts++;
  const result = hudkit.forSlot(slot).tryOwnBanner({
    text: `Plugin A owns this banner · source ${sourceRevision}`,
    holdSeconds: 0,
  });
  if (!result.ok) {
    if (result.error.code === "Busy") ownerCounters.busy++;
    else ownerCounters.failed++;
    return { ok: false, code: result.error.code, message: result.error.message };
  }
  ownerCounters.acquired++;
  banners.set(slot, result.value);
  return { ok: true, reused: false };
}

function disposeBanner(slot: number): void {
  const handle = banners.get(slot);
  if (!handle) return;
  handle.dispose();
  ownerCounters.disposed++;
  banners.delete(slot);
}

function closeSlot(slot: number): void {
  const view = modalViews.get(slot);
  if (view?.isValid()) view.close();
  modalViews.delete(slot);
  const badgeView = badgeViews.get(slot);
  if (badgeView?.isValid()) badgeView.hide();
  badgeViews.delete(slot);
  disposeBanner(slot);
}

function releaseAll(): void {
  const slots = new Set([...modalViews.keys(), ...badgeViews.keys(), ...banners.keys()]);
  for (const slot of slots) closeSlot(slot);
  modal?.release();
  badge?.release();
  modal = null;
  badge = null;
}

function status(slot: number | null) {
  const client = slot === null ? null : Clients.fromSlot(slot);
  const modalView = slot === null ? null : modalViews.get(slot) ?? null;
  const badgeView = slot === null ? null : badgeViews.get(slot) ?? null;
  const banner = slot === null ? null : banners.get(slot) ?? null;
  return {
    plugin: "@demo/ui-multiplugin-a",
    source: { revision: sourceRevision, order, rows: order.map((id) => domain.get(id)) },
    provider: {
      calls: providerCalls,
      invalidationRequests,
      lastProvidedRevision: slot === null ? null : lastProvidedRevision.get(slot) ?? null,
      lastSuccessfulSyncOperationRevision: slot === null
        ? null
        : successfulSyncOperationRevision.get(slot) ?? null,
    },
    clicks: { total: clicks, revalidated: revalidatedClicks, rejected: rejectedClicks, last: lastClick },
    owners: ownerCounters,
    claims: {
      modal: modal !== null,
      modalError: modalClaimError,
      badge: badge !== null,
      badgeError: badgeClaimError,
    },
    client: client && {
      slot: client.slot,
      valid: client.isValid(),
      userId: client.userId,
      steamId: client.steamId,
      signonState: client.signonState,
    },
    modalView: modalView && {
      valid: modalView.isValid(),
      open: modalView.isOpen(),
      lastUpdateResult: modalView.lastUpdateResult(),
    },
    badgeView: badgeView && { valid: badgeView.isValid() },
    banner: banner && { valid: banner.isValid() },
  };
}

export function OnPluginStart(): void {
  claimPools();

  command("sm_ui_a_open", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    claimPools();
    if (!modal || !badge) {
      cmd.reply(`${TAG} ${JSON.stringify({ ok: false, modalClaimError, badgeClaimError })}`);
      return HookResult.Handled;
    }

    const open = modal.tryOpenResult(slot, { cursor: true, focus: FOCUS });
    if (open.ok) {
      modalViews.set(slot, open.value);
      successfulSyncOperationRevision.set(slot, sourceRevision);
    }
    const badgeView = badge.forSlot(slot);
    const badgeShow = badgeView.tryShow({ title: "PLUGIN A", text: `source ${sourceRevision}` });
    if (badgeShow.ok) badgeViews.set(slot, badgeView);
    const banner = ownBanner(slot);
    cmd.reply(`${TAG} ${JSON.stringify({
      action: "open",
      slot,
      modal: summarize(open),
      badge: summarize(badgeShow),
      banner,
      sourceRevision,
      providerCalls,
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_a_reorder", (cmd) => {
    const before = [...order];
    order = [order[1], order[2], order[0]];
    sourceRevision++;
    const changed = domain.get("alpha");
    if (changed) {
      domain.set("alpha", { ...changed, label: `${changed.baseLabel} current r${sourceRevision}` });
    }
    cmd.reply(`${TAG} ${JSON.stringify({
      action: "reorder",
      painted: false,
      sourceRevision,
      before,
      after: order,
      providerCalls,
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_a_refresh", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    if (!modal) {
      cmd.reply(`${TAG} ${JSON.stringify({ ok: false, code: modalClaimError ?? "Released" })}`);
      return HookResult.Handled;
    }
    const result = modal.tryRefresh(slot);
    if (result.ok) successfulSyncOperationRevision.set(slot, sourceRevision);
    cmd.reply(`${TAG} ${JSON.stringify({
      action: "refresh",
      slot,
      result: summarize(result),
      sourceRevision,
      providerCalls,
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_a_invalidate", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    const view = modalViews.get(slot);
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
      sourceRevision,
      providerCallsBefore: before,
      providerCallsAfter: providerCalls,
      immediateProviderDelta: providerCalls - before,
      lastUpdateResult: view.lastUpdateResult(),
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_a_banner_busy", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    if (!banners.get(slot)?.isValid()) {
      cmd.reply(`${TAG} ${JSON.stringify({
        ok: false,
        code: "InvalidArgument",
        message: "run sm_ui_a_open first so the primary owned banner is live",
      })}`);
      return HookResult.Handled;
    }
    ownerCounters.attempts++;
    const result = hudkit.forSlot(slot).tryOwnBanner({
      text: "Unexpected competing A banner",
      holdSeconds: 0,
    });
    if (result.ok) {
      ownerCounters.unexpectedSecondAcquire++;
      result.value.dispose();
      ownerCounters.disposed++;
    } else if (result.error.code === "Busy") ownerCounters.busy++;
    else ownerCounters.failed++;
    cmd.reply(`${TAG} ${JSON.stringify({ action: "banner-busy", slot, result: summarize(result) })}`);
    return HookResult.Handled;
  });

  command("sm_ui_a_close", (cmd) => {
    const slot = resolveSlot(cmd);
    if (slot === null) return HookResult.Handled;
    closeSlot(slot);
    cmd.reply(`${TAG} ${JSON.stringify({ action: "close", ok: true, slot, owners: ownerCounters })}`);
    return HookResult.Handled;
  });

  command("sm_ui_a_release", (cmd) => {
    releaseAll();
    cmd.reply(`${TAG} ${JSON.stringify({
      action: "release",
      ok: true,
      liveBannerHandles: banners.size,
      owners: ownerCounters,
    })}`);
    return HookResult.Handled;
  });

  command("sm_ui_a_status", (cmd) => {
    const requested = cmd.argCount > 0 ? cmd.argInt(0, -1) : cmd.callerSlot;
    cmd.reply(`${TAG} ${JSON.stringify(status(requested >= 0 ? requested : null))}`);
    return HookResult.Handled;
  });
}

export function OnClientDisconnect(client: Client): void {
  closeSlot(client.slot);
  successfulSyncOperationRevision.delete(client.slot);
  lastProvidedRevision.delete(client.slot);
}

export function OnPluginEnd(): void {
  releaseAll();
}

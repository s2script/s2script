import { Bans, Clients, command, HookResult, Plugins, watchOptional } from "@s2script/sdk";
import type { BaseComm } from "@s2script/basecomm";
import type { BaseBans } from "@s2script/basebans";
import { Churn } from "../../../shared/churn";

declare function __s2_async_stats(): string;
interface Report { action: number; result: number; original: string; final: string }
interface Probe { probe(mode: string): Report; malformed(): number; ping(depth: number): void }
let numeric: Probe | null = null;
let text: Probe | null = null;
let saved: Probe | null = null;
let comm: BaseComm | null = null;
let bans: BaseBans | null = null;
let attached = 0, detached = 0, hits = 0, textHits = 0, isolationFailures = 0, recursionBlocked = 0;
let map = 0;
let captured: { map: number; slot: number; userId: number }[] = [];
const serviceEvents = { mute: 0, gag: 0, request: 0, recorded: 0, removed: 0 };
const TEST_ID = "18446744073709551615"; // Offline domain fixture, never authentication evidence.
function resources(): Record<string, number> {
  const value = JSON.parse(__s2_async_stats()).interop as Record<string, number>;
  if (!value || !["watches", "callbacks", "attachments", "disposers", "pending", "subscriptions", "methods", "ledger"].every(key => Number.isSafeInteger(value[key]))) throw Error("interop diagnostics unavailable");
  return value;
}
const churn = new Churn({
  loaded: () => Plugins.list().some(p => p.id === "@interop/numeric" && p.loaded),
  counts: () => ({ attached, detached, hits }), resources,
  load: () => Plugins.load("@interop/numeric"), unload: () => Plugins.unload("@interop/numeric"),
  probe: () => { if (!numeric) throw Error("attachment missing"); numeric.probe("cycle"); },
  stale: () => { try { saved?.probe("stale"); return false; } catch (error) { return String(error).includes("InterfaceUnavailable"); } },
});
function probe(): object {
  if (!numeric || !text) throw Error("both providers must be attached");
  const start = { hits, textHits, isolationFailures, recursionBlocked };
  const a = numeric.probe("normal"), b = text.probe("normal");
  const normal = { numeric: hits - start.hits, text: textHits - start.textHits };
  const stop = { numeric: numeric.probe("stop").action, text: text.probe("stop").action };
  const beforeMalformed = hits + textHits;
  const malformed = numeric.malformed() + text.malformed();
  const malformedDelivered = hits + textHits - beforeMalformed;
  numeric.ping(0);
  const recursion = recursionBlocked - start.recursionBlocked;
  const recovery = numeric.probe("recovery");
  return { a, b, normal, stop, malformed, malformedDelivered, recursion, recovery: recovery.action,
    isolationFailures: isolationFailures - start.isolationFailures, resources: resources() };
}
function services(): object {
  if (!comm || !bans) throw Error("basecomm and basebans must be attached");
  if (Clients.all().some(c => c.steamId === TEST_ID) || Bans.get(TEST_ID) || comm.isMuted(TEST_ID) || comm.isGagged(TEST_ID)) throw Error("offline fixture identity is occupied");
  const before = { ...serviceEvents };
  const result: Record<string, unknown> = {};
  try {
    result.mute = comm.setMuted(TEST_ID, true) && comm.isMuted(TEST_ID);
    result.gag = comm.setGagged(TEST_ID, true) && comm.isGagged(TEST_ID);
    result.invalidMute = comm.setMuted("0", true);
    result.invalidBan = bans.ban({ steamId: "0", minutes: 0, reason: "invalid", source: "plugin", actorSteamId: null });
    result.ban = bans.ban({ steamId: TEST_ID, minutes: 0, reason: "interop acceptance offline", source: "plugin", actorSteamId: null });
    result.unban = bans.unban({ steamId: TEST_ID });
  } finally {
    comm.setMuted(TEST_ID, false); comm.setGagged(TEST_ID, false);
    if (Bans.get(TEST_ID)) bans.unban({ steamId: TEST_ID });
  }
  result.events = Object.fromEntries(Object.keys(before).map(key => [key, serviceEvents[key as keyof typeof before] - before[key as keyof typeof before]]));
  result.cleaned = !Bans.get(TEST_ID) && !comm.isMuted(TEST_ID) && !comm.isGagged(TEST_ID);
  return result;
}
export function OnPluginStart(): void {
  watchOptional("@interop/numeric", (service, scope) => {
    numeric = service; saved = service; attached++;
    scope.own({ dispose() { numeric = null; detached++; } });
    scope.own(service.on("OnSignal", event => {
      hits++; if (event.value !== 1) isolationFailures++;
      if (event.mode.startsWith("recurse:")) {
        try { service.ping(Number(event.mode.slice(8)) + 1); }
        catch (error) { if (String(error).includes("InterfaceRecursionLimit")) recursionBlocked++; else throw error; }
      }
    }));
    scope.own(service.on("OnRequest", () => HookResult.Handled));
    scope.own(service.on("OnFormat", event => ({ result: HookResult.Changed, patch: { value: event.value + 10 } })));
  });
  watchOptional("@interop/text", (service, scope) => {
    text = service; scope.own({ dispose() { text = null; } });
    scope.own(service.on("OnSignal", event => { textHits++; if (event.text !== "seed") isolationFailures++; }));
    scope.own(service.on("OnRequest", () => HookResult.Handled));
    scope.own(service.on("OnFormat", event => ({ result: HookResult.Changed, patch: { text: event.text + "!" } })));
  });
  watchOptional("@s2script/basecomm", (service, scope) => {
    comm = service; scope.own({ dispose() { comm = null; } });
    scope.own(service.on("OnClientMuteChanged", event => { if (event.steamId === TEST_ID) serviceEvents.mute++; }));
    scope.own(service.on("OnClientGagChanged", event => { if (event.steamId === TEST_ID) serviceEvents.gag++; }));
  });
  watchOptional("@s2script/basebans", (service, scope) => {
    bans = service; scope.own({ dispose() { bans = null; } });
    scope.own(service.on("OnBanRequested", event => { if (event.steamId === TEST_ID) serviceEvents.request++; return HookResult.Continue; }));
    scope.own(service.on("OnBanRecorded", event => { if (event.request.steamId === TEST_ID) serviceEvents.recorded++; }));
    scope.own(service.on("OnBanRemoved", event => { if (event.steamId === TEST_ID) serviceEvents.removed++; }));
  });
  const commands: Record<string, () => object> = {
    s2_interop_probe: probe, s2_interop_services: services,
    s2_interop_service_status: () => ({ id: TEST_ID, ban: Bans.get(TEST_ID), muted: comm?.isMuted(TEST_ID) ?? null, gagged: comm?.isGagged(TEST_ID) ?? null, events: { ...serviceEvents } }),
    s2_interop_churn: () => {
      if (Plugins.list().some(p => p.id === "@interop/named" && p.loaded)) throw Error("unload @interop/named before churn");
      churn.start(); return churn.status;
    },
    s2_interop_status: () => ({ ...churn.status, attached, detached, hits, map, resources: resources() }),
    s2_interop_restore: () => ({ numeric: Plugins.load("@interop/numeric") }),
    s2_interop_restore_named: () => {
      if (!numeric) throw Error("wait for numeric attachment before restoring named");
      return { named: Plugins.reload("@interop/named") };
    },
    s2_interop_map_capture: () => { captured = Clients.all().map(c => ({ map, slot: c.slot, userId: c.userId })); return { map, captured: captured.length }; },
    s2_interop_map_check: () => ({ map, captured: captured.length, blocked: captured.filter(c => c.map !== map || Clients.fromSlot(c.slot)?.userId !== c.userId).length }),
  };
  for (const [name, fn] of Object.entries(commands)) command.server(name, cmd => {
    try {
      const json = JSON.stringify(fn());
      cmd.reply(json.length <= 1800 ? json : '{"error":"summary too large"}');
    } catch (error) { cmd.reply(JSON.stringify({ error: String(error).slice(0, 200) })); }
  });
}
export function OnGameFrame(): void { churn.tick(); }
export function OnMapStart(): void { map++; churn.mapChanged(); }

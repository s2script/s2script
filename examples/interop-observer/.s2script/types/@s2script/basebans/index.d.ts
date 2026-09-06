/**
 * Trusted BaseBans operations (protocol 2). Command/menu permission and immunity
 * checks belong to their wrappers. source and actorSteamId are caller-supplied
 * context, never authorization proof.
 */
import type { Hook, Notification } from "@s2script/sdk/interfaces";
import type { HookResultValue } from "@s2script/sdk/events";

export interface BanRequest {
  /** Canonical nonzero decimal u64 SteamID. Offline identities are accepted. */
  steamId: string;
  /** Nonnegative safe integer; 0 is permanent. Expiry arithmetic must remain safe. */
  minutes: number;
  reason: string;
  source: "command" | "menu" | "plugin";
  /** Canonical nonzero decimal u64, or null for server console/no actor. */
  actorSteamId: string | null;
}
export interface BanResult {
  /** Immediate cache readback matches the requested reason and expiry. This is
   * not disk acknowledgement or proof of a fresh write when an identical record
   * already existed. A persistence failure can still return true. */
  recorded: boolean;
  result: HookResultValue;
}
export interface BanRecord { request: BanRequest; until: number }
export interface UnbanRequest { steamId: string }

/** Validate, dispatch the advisory hook, record/read back, notify, then kick the
 * same currently connected identity snapshotted before callbacks (if still live).
 * Invalid domain requests return {recorded:false,result:Continue} before effects.
 * Handled/Stop suppress recording and kicking; Changed alone applies no patch. */
export declare function ban(request: BanRequest): BanResult;
/** Remove from cache; return true and notify only when a cache key existed.
 * Invalid identities and absent keys return false. No disk acknowledgement. */
export declare function unban(request: UnbanRequest): boolean;
export interface BaseBans {
  ban(request: BanRequest): BanResult;
  unban(request: UnbanRequest): boolean;
}
export interface Contract {
  methods: BaseBans;
  forwards: {
    /** Advisory, fail-open: throwing/invalid listeners contribute Continue.
     * This hook is not a fail-closed authorization boundary. */
    OnBanRequested: Hook<BanRequest>;
    /** Cache visibility immediately after add, before kick; not disk durability.
     * Reconnect enforcement does not emit this event. */
    OnBanRecorded: Notification<BanRecord>;
    OnBanRemoved: Notification<UnbanRequest>;
  };
}

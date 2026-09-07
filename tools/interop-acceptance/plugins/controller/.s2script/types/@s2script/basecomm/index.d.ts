/**
 * @s2script/basecomm — trusted plugin access to BaseComm mute and gag policy.
 *
 * Protocol 2. Methods read or update policy keyed by canonical, nonzero decimal
 * SteamID64 strings. Setters accept offline identities and return true when the
 * request is valid and policy equals the requested state, including an unchanged
 * request. Notifications report transitions only. Mute policy does not guarantee
 * audio delivery when the host voice descriptor is degraded.
 *
 * Command and menu permission/immunity checks remain in BaseComm's UI wrappers.
 * A plugin calling these methods is trusted and bypasses those checks.
 */
import type { Notification } from "@s2script/sdk/interfaces";

export interface CommunicationStateEvent {
  /** Canonical nonzero decimal SteamID64. */
  steamId: string;
  /** The new BaseComm policy state. */
  state: boolean;
}

/** Whether mute policy is enabled for this SteamID. Invalid identities return false. */
export declare function isMuted(steamId: string): boolean;
/** Whether gag policy is enabled for this SteamID. Invalid identities return false. */
export declare function isGagged(steamId: string): boolean;
/** Set mute policy. True means the valid request is now represented, not that audio delivery was proven. */
export declare function setMuted(steamId: string, state: boolean): boolean;
/** Set gag policy. True means the valid request is now represented. */
export declare function setGagged(steamId: string, state: boolean): boolean;

/** Method surface retained for type imports. */
export interface BaseComm {
  isMuted(steamId: string): boolean;
  isGagged(steamId: string): boolean;
  setMuted(steamId: string, state: boolean): boolean;
  setGagged(steamId: string, state: boolean): boolean;
}

export interface Contract {
  methods: BaseComm;
  forwards: {
    /** Fired after mute policy and any current live engine state change. */
    OnClientMuteChanged: Notification<CommunicationStateEvent>;
    /** Fired after gag policy changes. */
    OnClientGagChanged: Notification<CommunicationStateEvent>;
  };
}

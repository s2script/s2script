// GENERATED from gamedata/functions.jsonc — DO NOT EDIT.
import type { EntityRef } from "@s2script/sdk/entity";
import "@s2script/sdk/unsafe";
declare module "@s2script/sdk/unsafe" {
  interface CommitSuicidePreView {
    readonly self: EntityRef;
    readonly explode: boolean;
    force: boolean;
  }
  interface CommitSuicidePostView {
    readonly self: EntityRef;
    readonly explode: boolean;
    readonly force: boolean;
    readonly returnValue: void;
  }
  interface CommitSuicideAvailableBinding {
    readonly available: true;
    readonly status: FunctionStatus;
    call(self: EntityRef, arg0_explode: boolean, arg1_force: boolean): void;
    onPre(handler: (view: CommitSuicidePreView) => 0 | 1 | 2 | 3 | void): FunctionSubscription;
    onPre(options: { observeOnly: true }, handler: (view: Readonly<CommitSuicidePreView>) => void | undefined): FunctionSubscription;
    onPost(handler: (view: Readonly<CommitSuicidePostView>) => void): FunctionSubscription;
  }
  interface CommitSuicideUnavailableBinding {
    readonly available: false;
    readonly status: FunctionStatus;
  }
  interface StaleEntryProbePreView {
    readonly self: EntityRef;
  }
  interface StaleEntryProbePostView {
    readonly self: EntityRef;
    readonly returnValue: void;
  }
  interface StaleEntryProbeAvailableBinding {
    readonly available: true;
    readonly status: FunctionStatus;
    call(self: EntityRef): void;
  }
  interface StaleEntryProbeUnavailableBinding {
    readonly available: false;
    readonly status: FunctionStatus;
  }
  interface EngineFunctions {
    commitSuicide: CommitSuicideAvailableBinding | CommitSuicideUnavailableBinding;
    staleEntryProbe: StaleEntryProbeAvailableBinding | StaleEntryProbeUnavailableBinding;
  }
}

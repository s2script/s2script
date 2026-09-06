import { bindForwards, command, HookResult } from "@s2script/sdk";
let numericHits = 0;
let textHits = 0;
export function OnNumericSignal(event: { value: number; mode: string }): void {
  numericHits++; event.value = -999;
}
export function OnTextSignal(event: { text: string; mode: string }): void {
  textHits++; event.text = "mutated";
}
const numeric = bindForwards("@interop/numeric", {
  OnSignal: OnNumericSignal,
  OnRequest: event => event.mode === "stop" ? HookResult.Stop : HookResult.Changed,
  OnFormat: event => ({ result: HookResult.Changed, patch: { value: event.value + 1 } }),
});
const text = bindForwards("@interop/text", {
  OnSignal: OnTextSignal,
  OnRequest: event => event.mode === "stop" ? HookResult.Stop : HookResult.Changed,
  OnFormat: event => ({ result: HookResult.Changed, patch: { text: event.text + "!" } }),
});
export function OnPluginStart(): void {
  command.server("s2_interop_named", cmd => cmd.reply(JSON.stringify({ numericHits, textHits })));
  command.server("s2_interop_dispose", cmd => { numeric.dispose(); text.dispose(); cmd.reply("disposed"); });
}

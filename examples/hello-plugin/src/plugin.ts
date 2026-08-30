// hello-plugin — the smallest complete s2script plugin. Start here.
//
// It shows the four things every plugin does:
//   1. export function OnPluginStart            — runs once at load
//   2. register a command                      — command("sm_hello", …)
//   3. subscribe to a game event               — hook.event
//   4. survive a hot reload                    — OnPluginState / previous()
//
// Build it:   npx @s2script/sdk build examples/hello-plugin
// Then drop dist/*.s2sp into addons/s2script/plugins/ on a running server.
import { Player } from "@s2script/cs2";
import { hook, previous } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";

// State that survives a hot reload. Edit this file on a running server and the
// host hands `OnPluginState`'s return value to the next instance as `previous()`.
interface State { greeted: number; }

let greeted = 0;

export function OnPluginStart(): void {
  const prev = previous() as State | undefined;
  greeted = prev?.greeted ?? 0;

  console.log(`[hello] loaded (greeted so far: ${greeted})`);

  command("sm_hello", (cmd) => {
    cmd.reply(`hello! I have greeted ${greeted} spawns since first load.`);
  });

  hook.event("player_spawn", (ev) => {
    greeted += 1;
    const player = Player.fromSlot(ev.getPlayerSlot("userid"));
    console.log(`[hello] spawn #${greeted}: ${player?.playerName ?? "unknown"}`);
  });
}

export function OnPluginEnd(): void {
  console.log(`[hello] unloading after ${greeted} greetings`);
}

export function OnPluginState(): State {
  return { greeted };
}

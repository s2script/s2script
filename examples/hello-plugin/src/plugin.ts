// hello-plugin — the smallest complete s2script plugin. Start here.
//
// It shows the four things every plugin does:
//   1. define itself with plugin((ctx) => …)   — the factory runs once at load
//   2. register a command                      — ctx.commands.register
//   3. subscribe to a game event               — ctx.events.on
//   4. survive a hot reload                    — return { state, onUnload }
//
// Build it:   npx @s2script/sdk build examples/hello-plugin
// Then drop dist/*.s2sp into addons/s2script/plugins/ on a running server.
import { plugin } from "@s2script/sdk/plugin";
import { Player } from "@s2script/cs2";

// State that survives a hot reload. Edit this file on a running server and the
// host hands `state()`'s return value to the next instance as `ctx.previous`.
interface State { greeted: number; }

export default plugin((ctx) => {
  // ctx.previous is undefined on a first load, and the previous instance's
  // state() return on a reload.
  const prev = ctx.previous as State | undefined;
  let greeted = prev?.greeted ?? 0;

  console.log(`[hello] loaded (greeted so far: ${greeted})`);

  // A command any client can run, from chat or console.
  ctx.commands.register("hello", (cmd) => {
    cmd.reply(`hello! I have greeted ${greeted} spawns since first load.`);
  });

  // A game event. The GameEvent is only valid synchronously — read what you
  // need inside the handler, never stash it.
  ctx.events.on("player_spawn", (ev) => {
    greeted += 1;
    const player = Player.fromSlot(ev.getPlayerSlot("userid"));
    console.log(`[hello] spawn #${greeted}: ${player?.playerName ?? "unknown"}`);
  });

  return {
    // Best-effort cleanup. The ledger is the real teardown authority — you do
    // not have to unregister what you registered through ctx.
    onUnload() {
      console.log(`[hello] unloading after ${greeted} greetings`);
    },
    // Handed to the next instance as ctx.previous. Serialized as JSON
    // (EntityRef-aware), so no BigInt — carry 64-bit values as strings.
    state(): State {
      return { greeted };
    },
  };
});

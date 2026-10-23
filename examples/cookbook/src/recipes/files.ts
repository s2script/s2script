import { Files } from "@s2script/sdk/files";
import { command, HookResult, Server } from "@s2script/sdk";
import { Pawn } from "@s2script/cs2";

/**
 * Files runs every read/write on the async runtime's worker threads: the game thread copies the
 * payload and later gets the result on a frame drain. This records the caller's movement for N
 * ticks into one preallocated Float64Array (Pawn.readKinematics: one native call per tick, no
 * per-field allocation), writes it gzip-compressed, then reads it back — and reports how many
 * ticks advanced while the IO was in flight.
 *
 *   sm_files_record [ticks]  — default 128 ticks (2 s at 64 tick)
 */
export const name = "files";
export const describe = "record movement to a gzip file off-thread and read it back (sm_files_record [ticks])";

const SLOTS = Pawn.KINEMATICS_SLOTS;
let recording: { slot: number; pawn: Pawn; buf: Float64Array; n: number; max: number } | null = null;

export function OnGameFrame(): void {
  const r = recording;
  if (!r) return;
  if (r.n >= r.max || !r.pawn.readKinematics(r.buf, r.n * SLOTS)) {
    recording = null;
    void save(r.slot, r.buf.subarray(0, r.n * SLOTS), r.n);
    return;
  }
  r.n += 1;
}

async function save(slot: number, frames: Float64Array, n: number): Promise<void> {
  const bytes = new Uint8Array(frames.buffer, frames.byteOffset, frames.byteLength);
  const before = Server.tickCount;
  await Files.write(`recordings/slot${slot}.bin`, bytes, { gzip: true });
  const back = await Files.read(`recordings/slot${slot}.bin`, { gzip: true });
  const ticks = Server.tickCount - before;
  console.log(`[cookbook] files: ${n} ticks, ${bytes.length} B raw -> read back ${back ? back.length : 0} B; ` +
    `${ticks} ticks advanced during the IO (tick = ${Server.tickInterval}s)`);
}

export function OnPluginStart(): void {
  command("sm_files_record", (cmd) => {
    const slot = cmd.callerSlot;
    const pawn = slot >= 0 ? Pawn.forSlot(slot) : null;
    if (!pawn) { cmd.reply("in-game only"); return HookResult.Handled; }
    const max = Math.max(1, Math.min(64 * 60, Number(cmd.arg(1)) || 128));
    recording = { slot, pawn, buf: new Float64Array(max * SLOTS), n: 0, max };
    cmd.reply(`recording ${max} ticks…`);
    return HookResult.Handled;
  });
}

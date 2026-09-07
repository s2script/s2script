import {
  after,
  Client,
  Clients,
  command,
  config,
  createEntity,
  Database,
  delay,
  fetch,
  HookResult,
  Net,
  nextFrame,
  SDKHook,
  SDKHookType,
  SDKUnhook,
  threadSleep,
  Timer,
  type EntityRef,
} from "@s2script/sdk";

declare function __s2_async_stats(): string;

const PEER = "s2script-mixed-slow-peer";
const HTTP_PORT = 18080;
const TCP_PORT = 18081;
const TIMER_EXPECTED = 12;
const HTTP_EXPECTED = 2;
const TCP_EXPECTED = 2;
const HOOK_EXPECTED = 16;
const SLOT_PENDING_CAP = 64;
const SLOT_STABILITY_MS = 250;
// Source console replies truncate near 2 KiB. Keep framed stats lines comfortably below that cap.
const STATS_CHUNK_CHARS = 1024;
const STATS_MAX_CHARS = 65536;

interface CycleState {
  cycle: number;
  state: "idle" | "running" | "done";
  timers: number;
  http: number;
  tcp: number;
  hooks: number;
  db: "pending" | "pass" | "fail";
  errors: string[];
}

interface PressureState {
  state: "idle" | "running" | "done";
  expected: number;
  settled: number;
  success: number;
  rejected: number;
  other: number;
}

let cycle: CycleState = {
  cycle: -1,
  state: "idle",
  timers: 0,
  http: 0,
  tcp: 0,
  hooks: 0,
  db: "pending",
  errors: [],
};
let configGeneration = 0;
let slotAttempts = 0;
let slotReuses = 0;
let slotFailures = 0;
let statsSnapshot = 0;
const staleBySlot = new Map<number, Client>();
let slotCheckTimer: Timer | null = null;
let pressure: PressureState = { state: "idle", expected: 0, settled: 0, success: 0, rejected: 0, other: 0 };

function errorText(error: unknown): string {
  return error instanceof Error ? `${error.name}:${error.message}` : String(error);
}

function isQueueFull(error: unknown): boolean {
  return error instanceof Error && (error.name === "AsyncQueueFull" || error.message.includes("AsyncQueueFull"));
}

function settlePressure(kind: "success" | "rejected" | "other"): void {
  pressure[kind] += 1;
  pressure.settled += 1;
  if (pressure.settled !== pressure.expected) return;
  pressure.state = "done";
  console.log(`[mixed-live] PRESSURE_DONE expected=${pressure.expected} success=${pressure.success} ` +
    `rejected=${pressure.rejected} other=${pressure.other}`);
}

function runPressure(expected: number): void {
  pressure = { state: "running", expected, settled: 0, success: 0, rejected: 0, other: 0 };
  for (let i = 0; i < expected; i += 1) {
    try {
      threadSleep(750).then(
        () => settlePressure("success"),
        (error: unknown) => settlePressure(isQueueFull(error) ? "rejected" : "other"),
      );
    } catch (error) {
      settlePressure(isQueueFull(error) ? "rejected" : "other");
    }
  }
}

function pressureLine(): string {
  return `[mixed-live] PRESSURE state=${pressure.state} expected=${pressure.expected} ` +
    `settled=${pressure.settled} success=${pressure.success} rejected=${pressure.rejected} other=${pressure.other}`;
}

function ascii(text: string): Uint8Array {
  const value = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i += 1) value[i] = text.charCodeAt(i) & 0xff;
  return value;
}

function framed(text: string): Uint8Array {
  const payload = ascii(text);
  const value = new Uint8Array(payload.length + 4);
  value[0] = (payload.length >>> 24) & 0xff;
  value[1] = (payload.length >>> 16) & 0xff;
  value[2] = (payload.length >>> 8) & 0xff;
  value[3] = payload.length & 0xff;
  value.set(payload, 4);
  return value;
}

function timerWork(): Promise<number> {
  const tasks: Promise<void>[] = [];
  for (let i = 0; i < 6; i += 1) {
    tasks.push(new Promise<void>((resolve) => {
      const timer = after(15 + i * 5, resolve);
      if (i % 3 === 0) {
        if (!timer.kill()) throw new Error(`timer ${i} could not be killed`);
        resolve();
      }
    }));
  }
  tasks.push(delay(20), delay(30), nextFrame(), nextFrame(), threadSleep(10), threadSleep(15));
  return Promise.all(tasks).then((values) => values.length);
}

async function httpWork(cycleNumber: number): Promise<number> {
  const requests = Array.from({ length: HTTP_EXPECTED }, (_unused, index) =>
    fetch(`http://${PEER}:${HTTP_PORT}/slow?bytes=4096&chunks=8&delay_ms=${20 + index}&cycle=${cycleNumber}`, {
      timeoutMs: 5000,
    }).then((response) => {
      if (response.status !== 200 || response.text().length !== 4096) {
        throw new Error(`bad HTTP response status=${response.status} bytes=${response.text().length}`);
      }
    })
  );
  await Promise.all(requests);
  return requests.length;
}

async function tcpOne(cycleNumber: number, index: number): Promise<void> {
  const payload = `cycle=${cycleNumber};stream=${index};` + "x".repeat(2048);
  const socket = await Net.connectTcp(PEER, TCP_PORT);
  await new Promise<void>((resolve, reject) => {
    let settled = false;
    let received = "";
    let timeout: Timer | null = null;
    const finish = (error?: string) => {
      if (settled) return;
      settled = true;
      timeout?.kill();
      socket.close();
      if (error) reject(new Error(error));
      else resolve();
    };
    socket.onData((bytes) => {
      for (const byte of bytes) received += String.fromCharCode(byte);
      if (received.includes(`ACK ${payload.length}\n`)) finish();
    });
    socket.onError((message) => finish(`TCP error: ${message}`));
    socket.onClose(() => finish("TCP closed before ACK"));
    timeout = after(5000, () => finish("TCP ACK timeout"));
    if (!socket.send(framed(payload))) finish("TCP send rejected");
  });
}

async function tcpWork(cycleNumber: number): Promise<number> {
  const streams = Array.from({ length: TCP_EXPECTED }, (_unused, index) => tcpOne(cycleNumber, index));
  await Promise.all(streams);
  return streams.length;
}

async function dbWork(cycleNumber: number): Promise<void> {
  const locker = await Database.open("mixed_live");
  const contender = await Database.open("mixed_live");
  let locked = false;
  try {
    await locker.execute("CREATE TABLE IF NOT EXISTS cycle_probe (id INTEGER PRIMARY KEY, cycle INTEGER NOT NULL, marker TEXT NOT NULL)");
    await locker.execute("BEGIN EXCLUSIVE");
    locked = true;
    let sawContention = false;
    try {
      await contender.execute("INSERT OR REPLACE INTO cycle_probe (id, cycle, marker) VALUES (1, ?, ?)", [cycleNumber, "blocked"]);
    } catch (error) {
      const message = errorText(error).toLowerCase();
      if (!message.includes("locked") && !message.includes("busy")) throw error;
      sawContention = true;
    }
    if (!sawContention) throw new Error("contending SQLite write unexpectedly succeeded");
    await locker.execute("ROLLBACK");
    locked = false;
    await contender.execute("INSERT OR REPLACE INTO cycle_probe (id, cycle, marker) VALUES (1, ?, ?)", [cycleNumber, "recovered"]);
    const rows = await contender.query("SELECT cycle, marker FROM cycle_probe WHERE id = 1");
    if (rows.length !== 1 || rows[0].cycle !== cycleNumber || rows[0].marker !== "recovered") {
      throw new Error(`SQLite recovery read mismatch rows=${JSON.stringify(rows)}`);
    }
  } finally {
    if (locked) {
      try { await locker.execute("ROLLBACK"); } catch (error) { console.error(`[mixed-live] rollback cleanup ${errorText(error)}`); }
    }
    await Promise.all([locker.close(), contender.close()]);
  }
}

function transmitProbe(_entity: EntityRef, _client: Client): void {
  // Registration/index teardown is the workload. Bots are not treated as evidence that this ran.
}

function hookWork(): number {
  const entities: EntityRef[] = [];
  try {
    for (let i = 0; i < HOOK_EXPECTED; i += 1) {
      const entity = createEntity("point_worldtext", { message: `mixed-live-${i}` });
      if (!entity) throw new Error(`create entity ${i} failed`);
      entities.push(entity);
      if (!SDKHook(entity, SDKHookType.SetTransmit, transmitProbe)) {
        throw new Error(`hook entity ${i} failed`);
      }
    }
    for (const entity of entities) {
      if (!SDKUnhook(entity, SDKHookType.SetTransmit, transmitProbe)) {
        throw new Error(`unhook entity ${entity.index} failed`);
      }
    }
    return entities.length;
  } finally {
    for (const entity of entities) entity.remove();
  }
}

async function runCycle(cycleNumber: number): Promise<void> {
  const state: CycleState = {
    cycle: cycleNumber,
    state: "running",
    timers: 0,
    http: 0,
    tcp: 0,
    hooks: 0,
    db: "pending",
    errors: [],
  };
  cycle = state;
  const components = [
    timerWork().then((count) => { state.timers = count; }).catch((error: unknown) => { state.errors.push(`timers:${errorText(error)}`); }),
    httpWork(cycleNumber).then((count) => { state.http = count; }).catch((error: unknown) => { state.errors.push(`http:${errorText(error)}`); }),
    tcpWork(cycleNumber).then((count) => { state.tcp = count; }).catch((error: unknown) => { state.errors.push(`tcp:${errorText(error)}`); }),
    dbWork(cycleNumber).then(() => { state.db = "pass"; }).catch((error: unknown) => {
      state.db = "fail";
      state.errors.push(`db:${errorText(error)}`);
    }),
    Promise.resolve().then(() => { state.hooks = hookWork(); }).catch((error: unknown) => {
      state.errors.push(`hooks:${errorText(error)}`);
    }),
  ];
  await Promise.all(components);
  state.state = "done";
  console.log(`[mixed-live] DONE cycle=${cycleNumber} pass=${state.errors.length === 0} errors=${JSON.stringify(state.errors)}`);
}

function sameConnection(client: Client, slot: number, userId: number, name: string): boolean {
  return client.isValid() && client.slot === slot && client.userId === userId && client.name === name;
}

function cleanupSlotProbes(): void {
  slotCheckTimer?.kill();
  slotCheckTimer = null;
  staleBySlot.clear();
}

function verifyReplacement(old: Client, replacement: Client): void {
  const slot = replacement.slot;
  const userId = replacement.userId;
  const name = replacement.name;
  const voice = replacement.voiceMuted;
  const oldValid = old.isValid();
  const oldUserId = old.userId;
  const oldName = old.name;
  const oldSteamId = old.steamId;
  const oldSignon = old.signonState;
  const oldBot = old.isBot;
  const oldIp = old.ip;
  const oldVoice = old.voiceMuted;
  let pass = !oldValid && oldUserId === -1 && oldName === "" && oldSteamId === "0" &&
    oldSignon === -1 && !oldBot && oldIp === "" && !oldVoice;
  const commandResult = old.fakeCommand("say mixed-live stale command MUST NOT run");
  old.voiceMuted = !voice;
  old.chat("mixed-live stale chat MUST NOT reach replacement");
  old.print("mixed-live stale print MUST NOT reach replacement");
  old.kick("mixed-live stale kick MUST NOT affect replacement");
  if (commandResult) pass = false;
  slotCheckTimer = after(100, () => {
    slotCheckTimer = null;
    const current = Clients.fromSlot(slot);
    pass = pass && current !== null && sameConnection(current, slot, userId, name) && current.voiceMuted === voice;
    if (pass) slotReuses += 1;
    else slotFailures += 1;
    console.log(`[mixed-live] SLOT_CHECK slot=${slot} oldValid=${oldValid} oldUserId=${oldUserId} ` +
      `oldName=${JSON.stringify(oldName)} oldSteamId=${oldSteamId} oldSignon=${oldSignon} ` +
      `oldBot=${oldBot} oldIp=${JSON.stringify(oldIp)} oldVoice=${oldVoice} commandResult=${commandResult} ` +
      `freshLookup=${current === null ? "missing" : "live"} freshValid=${replacement.isValid()} ` +
      `freshUserId=${current?.userId ?? -1} freshName=${JSON.stringify(current?.name ?? "")} ` +
      `freshVoice=${current?.voiceMuted ?? false} pass=${pass}`);
    cleanupSlotProbes();
    console.log(`[mixed-live] SLOT_REUSE slot=${slot} userId=${userId} pass=${pass} ` +
      `attempts=${slotAttempts} reuses=${slotReuses} failures=${slotFailures}`);
  });
}

function observeReplacement(old: Client, candidate: Client): void {
  const slot = candidate.slot;
  const userId = candidate.userId;
  const name = candidate.name;
  slotCheckTimer = after(SLOT_STABILITY_MS, () => {
    slotCheckTimer = null;
    const current = Clients.fromSlot(slot);
    if (!candidate.isValid() || current === null || !sameConnection(current, slot, userId, name)) {
      console.log(`[mixed-live] SLOT_RETRY slot=${slot} candidateUserId=${userId} ` +
        `candidateName=${JSON.stringify(name)} currentUserId=${current?.userId ?? -1} ` +
        `currentName=${JSON.stringify(current?.name ?? "")} reason=provisional-churn`);
      if (current !== null && current.isValid()) observeReplacement(old, current);
      return;
    }
    verifyReplacement(old, current);
  });
}

function statusLine(): string {
  const pass = cycle.state === "done" && cycle.errors.length === 0 && cycle.timers === TIMER_EXPECTED &&
    cycle.http === HTTP_EXPECTED && cycle.tcp === TCP_EXPECTED && cycle.db === "pass" &&
    cycle.hooks === HOOK_EXPECTED &&
    configGeneration === cycle.cycle && slotFailures === 0;
  return `[mixed-live] STATUS cycle=${cycle.cycle} state=${cycle.state} pass=${pass} ` +
    `timers=${cycle.timers}/${TIMER_EXPECTED} http=${cycle.http}/${HTTP_EXPECTED} ` +
    `tcp=${cycle.tcp}/${TCP_EXPECTED} hooks=${cycle.hooks}/${HOOK_EXPECTED} db=${cycle.db} config=${configGeneration} ` +
    `slotAttempts=${slotAttempts} slotReuses=${slotReuses} slotFailures=${slotFailures} ` +
    `slotPending=${staleBySlot.size}`;
}

export function OnPluginStart(): void {
  configGeneration = config.getInt("generation");
  config.onChange(() => {
    configGeneration = config.getInt("generation");
    console.log(`[mixed-live] CONFIG generation=${configGeneration}`);
  });

  command.server("sm_mixed_start", (cmd) => {
    const requested = cmd.argInt(0, -1);
    if (requested < 0 || cycle.state === "running") {
      cmd.reply(`[mixed-live] START rejected cycle=${requested} state=${cycle.state}`);
      return HookResult.Handled;
    }
    cmd.reply(`[mixed-live] START accepted cycle=${requested}`);
    void runCycle(requested);
    return HookResult.Handled;
  });

  command.server("sm_mixed_status", (cmd) => {
    cmd.reply(statusLine());
    return HookResult.Handled;
  });

  command.server("sm_mixed_stats", (cmd) => {
    // Capture one coherent point-in-time snapshot before emitting any part. A second native read
    // between replies could combine counters from different instants into invalid evidence.
    const payload = __s2_async_stats();
    statsSnapshot = statsSnapshot === Number.MAX_SAFE_INTEGER ? 1 : statsSnapshot + 1;
    if (payload.length > STATS_MAX_CHARS) {
      cmd.reply(`[mixed-live] STATS_ERROR snapshot=${statsSnapshot} reason=oversized ` +
        `bytes=${payload.length} cap=${STATS_MAX_CHARS}`);
      return HookResult.Handled;
    }
    const count = Math.max(1, Math.ceil(payload.length / STATS_CHUNK_CHARS));
    for (let index = 0; index < count; index += 1) {
      const data = payload.slice(index * STATS_CHUNK_CHARS, (index + 1) * STATS_CHUNK_CHARS);
      cmd.reply(`[mixed-live] STATS_PART snapshot=${statsSnapshot} part=${index + 1}/${count} ` +
        `bytes=${payload.length} data=${data}`);
    }
    return HookResult.Handled;
  });

  command.server("sm_mixed_pressure", (cmd) => {
    const expected = cmd.argInt(0, 256);
    if (pressure.state === "running" || expected < 1 || expected > 256) {
      cmd.reply(`[mixed-live] PRESSURE rejected state=${pressure.state} expected=${expected}`);
      return HookResult.Handled;
    }
    runPressure(expected);
    cmd.reply(`[mixed-live] PRESSURE accepted expected=${expected}`);
    return HookResult.Handled;
  });

  command.server("sm_mixed_pressure_status", (cmd) => {
    cmd.reply(pressureLine());
    return HookResult.Handled;
  });

  command.server("sm_mixed_slotreset", (cmd) => {
    cleanupSlotProbes();
    slotAttempts = slotReuses = slotFailures = 0;
    cmd.reply("[mixed-live] SLOT_RESET pending=0 checking=0");
    return HookResult.Handled;
  });

  command.server("sm_mixed_slotcleanup", (cmd) => {
    cleanupSlotProbes();
    cmd.reply("[mixed-live] SLOT_CLEANUP pending=0 checking=0");
    return HookResult.Handled;
  });

  command.server("sm_mixed_slotcycle", (cmd) => {
    const requested = cmd.argInt(0, -1);
    if (staleBySlot.size >= SLOT_PENDING_CAP) {
      slotFailures += 1;
      cmd.reply(`[mixed-live] SLOT failed cycle=${requested} reason=pending-cap cap=${SLOT_PENDING_CAP}`);
      return HookResult.Handled;
    }
    if (slotCheckTimer !== null) {
      cmd.reply(`[mixed-live] SLOT skipped cycle=${requested} reason=check-in-progress`);
      return HookResult.Handled;
    }
    const client = Clients.all().find((candidate) => candidate.isBot && candidate.signonState >= 5 &&
      !staleBySlot.has(candidate.slot));
    if (!client) {
      cmd.reply(`[mixed-live] SLOT skipped cycle=${requested} reason=no-eligible-bot`);
      return HookResult.Handled;
    }
    slotAttempts += 1;
    staleBySlot.set(client.slot, client);
    cmd.reply(`[mixed-live] SLOT armed cycle=${requested} slot=${client.slot} userId=${client.userId}`);
    client.kick("mixed-live slot generation churn");
    return HookResult.Handled;
  });
}

export function OnClientActive(client: Client): void {
  const old = staleBySlot.get(client.slot);
  if (!old || old.isValid() || slotCheckTimer !== null) return;
  observeReplacement(old, client);
}

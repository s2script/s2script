export interface ChurnIO {
  loaded(): boolean;
  counts(): { attached: number; detached: number; hits: number };
  resources(): Record<string, number>;
  load(): boolean;
  unload(): boolean;
  probe(): void;
  stale(): boolean;
}
export class Churn {
  readonly status = {
    state: 'idle' as 'idle' | 'running' | 'done' | 'failed', cycles: 0, deliveries: 0,
    staleBlocked: 0, error: '', baseline: {} as Record<string, number>, final: {} as Record<string, number>,
  };
  private phase: 'absent' | 'attached' = 'absent';
  private frames = 0;
  private settled = 0;
  private initial = true;
  private before = { attached: 0, detached: 0, hits: 0 };
  private activeResources: Record<string, number> | null = null;
  private io: ChurnIO;
  constructor(io: ChurnIO) { this.io = io; }
  start(): void {
    if (this.status.state === 'running') throw Error('churn already running');
    Object.assign(this.status, { state: 'running', cycles: 0, deliveries: 0, staleBlocked: 0, error: '', baseline: {}, final: {} });
    this.initial = true; this.activeResources = null; this.before = this.io.counts();
    this.phase = 'absent'; this.frames = 0; this.settled = 0;
    if (!this.io.loaded() || !this.io.unload()) this.fail('start requires a running provider');
  }
  private fail(error: string): void { this.status.state = 'failed'; this.status.error = error; }
  private equal(a: Record<string, number>, b: Record<string, number>): boolean {
    return Object.keys(a).length === Object.keys(b).length && Object.keys(a).every(k => a[k] === b[k]);
  }
  tick(): void {
    if (this.status.state !== 'running') return;
    try {
      if (++this.frames > 600) return this.fail('transition timeout after 600 frames');
      if (this.io.loaded() !== (this.phase === 'attached')) { this.settled = 0; return; }
      if (++this.settled < 3) return;
      const counts = this.io.counts();
      const resources = this.io.resources();
      if (this.phase === 'absent') {
        if (counts.detached !== this.before.detached + 1) return this.fail('detach count mismatch');
        if (this.initial) { this.status.baseline = resources; this.initial = false; }
        else {
          if (!this.io.stale()) return this.fail('stale provider method remained callable');
          this.status.staleBlocked++;
          if (!this.equal(resources, this.status.baseline)) return this.fail('absent resource baseline mismatch');
          this.status.cycles++;
        }
        this.status.final = resources;
        if (this.status.cycles === 1000) { this.status.state = 'done'; return; }
        this.before = counts;
        if (!this.io.load()) return this.fail('provider load refused');
        this.phase = 'attached';
      } else {
        if (counts.attached !== this.before.attached + 1) return this.fail('attachment generation count mismatch');
        if (this.activeResources && !this.equal(resources, this.activeResources)) return this.fail('active resource baseline mismatch');
        this.activeResources = resources;
        this.io.probe();
        if (this.io.counts().hits !== counts.hits + 1) return this.fail('delivery count mismatch');
        this.status.deliveries++;
        this.before = this.io.counts();
        if (!this.io.unload()) return this.fail('provider unload refused');
        this.phase = 'absent';
      }
      this.frames = 0; this.settled = 0;
    } catch (error) { this.fail(String(error).slice(0, 160)); }
  }
  mapChanged(): void { if (this.status.state === 'running') this.fail('map changed during churn'); }
}

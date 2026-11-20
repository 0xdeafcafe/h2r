// One cache for a viewer session. Seeking requests the target segment first.
// Fetch only the adjacent segment near an edge so interpolation has both poses.
import { chunkAt, recordingFrames, type ArchiveSegment, type ChunkIndex, type RecordingChunk, type TimelineSegment } from "../api/chunk";
import { FilmCache } from "../api/film-cache";
import type { Frame, WireEvent } from "../api/types";

export interface ReplayBlock { frames: Frame[]; events?: WireEvent[] }

export class ReplayWindow<T extends TimelineSegment> {
  private readonly history = new FilmCache<WireEvent>(4096, 4 << 20);
  private readonly loaded = new Map<number, ReplayBlock>();
  private readonly pending = new Map<number, {request: Promise<void>; controller: AbortController}>();
  private current = -1;
  private readonly pinned: number | undefined;

  constructor(readonly segments: T[], private readonly read: (c: T, signal: AbortSignal) => Promise<ReplayBlock>, loopStart?: number) {
    if (!segments.length) throw new Error("empty playback index");
    // Keep the loop's first segment ready when playback comes back to it.
    if (loopStart !== undefined) {
      const start = chunkAt(this.segments, loopStart) ?? this.segments[0]!;
      this.pinned = start.ordinal;
    }
  }

  ready(tick: number): boolean {
    const segment = chunkAt(this.segments, tick);
    const ordinal = segment?.ordinal ?? 0;
    return Math.abs(ordinal - this.current) <= 1 && this.loaded.has(ordinal);
  }

  get frames(): Frame[] {
    // Only contiguous data around the playhead can contribute to a pose.
    const out: Frame[] = [];
    for (let i = this.current - 1; i <= this.current + 1; i++) {
      const block = this.loaded.get(i);
      if (block) out.push(...block.frames);
    }
    return out;
  }

  get events(): WireEvent[] {
    const all = new Map<number, WireEvent>();
    for (const event of this.history.values()) all.set(event.position, event);
    // The active scene keeps its complete events even if one group exceeds
    // the optional history budget. Only neighbouring frames are retained.
    for (const block of this.loaded.values())
      for (const event of block.events ?? []) all.set(event.position, event);
    return [...all.values()];
  }

  private load(meta: T): Promise<void> {
    const i = meta.ordinal;
    const existing = this.pending.get(i);
    if (existing) return existing.request;
    const controller = new AbortController();
    const request = this.read(meta, controller.signal).then(block => {
      if (controller.signal.aborted) return;
      this.loaded.set(i, block);
      for (const event of block.events ?? []) {
        if (!this.history.get(event.position))
          this.history.set(event.position, event, 2 * JSON.stringify(event).length);
      }
    }).catch(error => { if (!controller.signal.aborted) throw error; }).finally(() => {
      if (this.pending.get(i)?.controller === controller) this.pending.delete(i);
    });
    this.pending.set(i, {request, controller});
    return request;
  }

  /** Null when the current window is ready. The first request resolves
   * without waiting for a neighbour, other segments or game events. */
  at(tick: number): Promise<void> | null {
    const meta = chunkAt(this.segments, tick) ?? this.segments[0]!;
    const changed = this.current !== meta.ordinal;
    this.current = meta.ordinal;
    if (changed) {
      for (const i of this.loaded.keys()) if (i !== this.pinned && Math.abs(i - this.current) > 1) this.loaded.delete(i);
      for (const [i, p] of this.pending) if (Math.abs(i - this.current) > 1) {
        p.controller.abort();
        this.pending.delete(i);
      }
    }
    if (!this.loaded.has(meta.ordinal)) return this.load(meta);
    if (changed) return Promise.resolve();
    const neighbor = tick >= meta.to_tick - 60 ? this.segments[this.current + 1]
      : tick < meta.from_tick + 2 ? this.segments[this.current - 1] : undefined;
    return neighbor && !this.loaded.has(neighbor.ordinal) ? this.load(neighbor) : null;
  }
}

export class ArchiveWindow extends ReplayWindow<ArchiveSegment> {
  constructor(index: ChunkIndex, fetch: (c: RecordingChunk, signal: AbortSignal) => Promise<Uint8Array>, loopStart?: number) {
    const segments = index.items.filter((m): m is ArchiveSegment => m.codec === "archive");
    if (segments.length !== index.items.length) throw new Error("mixed archive index");
    super(segments, async (c, s) => ({frames: await recordingFrames(await fetch(c, s), c)}), loopStart);
  }
}

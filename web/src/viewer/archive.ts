// One cache for a viewer session. Seeking requests the target segment first.
// Fetch only the adjacent segment near an edge so interpolation has both poses.
import { chunkAt, recordingFrames, type ArchiveSegment, type ChunkIndex, type RecordingChunk } from "../api/chunk";
import type { Frame } from "../api/types";

export class ArchiveWindow {
  readonly segments: ArchiveSegment[];
  private readonly loaded = new Map<number, Frame[]>();
  private readonly pending = new Map<number, {request: Promise<void>; controller: AbortController}>();
  private current = -1;
  private readonly pinned: number | undefined;

  constructor(index: ChunkIndex, private readonly fetch: (c: RecordingChunk, signal: AbortSignal) => Promise<Uint8Array>, loopStart?: number) {
    this.segments = index.items.filter((m): m is ArchiveSegment => m.codec === "archive");
    if (!this.segments.length || this.segments.length !== index.items.length) throw new Error("empty or mixed archive index");
    // Keep the loop's first segment ready when playback comes back to it.
    if (loopStart !== undefined) {
      const start = (chunkAt(this.segments, loopStart) ?? this.segments[0]!) as ArchiveSegment;
      this.pinned = start.ordinal;
    }
  }

  ready(tick: number): boolean {
    const segment = chunkAt(this.segments, tick) as ArchiveSegment | null;
    const ordinal = segment?.ordinal ?? 0;
    return Math.abs(ordinal - this.current) <= 1 && this.loaded.has(ordinal);
  }

  get frames(): Frame[] {
    // Only contiguous data around the playhead can contribute to a pose.
    const out: Frame[] = [];
    for (let i = this.current - 1; i <= this.current + 1; i++) {
      const frames = this.loaded.get(i);
      if (frames) out.push(...frames);
    }
    return out;
  }

  private load(meta: ArchiveSegment): Promise<void> {
    const i = meta.ordinal;
    const existing = this.pending.get(i);
    if (existing) return existing.request;
    const controller = new AbortController();
    const request = this.fetch(meta, controller.signal).then(async bytes => {
      if (controller.signal.aborted) return;
      const frames = await recordingFrames(bytes, meta);
      if (!controller.signal.aborted) this.loaded.set(i, frames);
    }).catch(error => { if (!controller.signal.aborted) throw error; }).finally(() => {
      if (this.pending.get(i)?.controller === controller) this.pending.delete(i);
    });
    this.pending.set(i, {request, controller});
    return request;
  }

  /** Null when the current window is ready. The first request resolves
   * without waiting for a neighbour, other segments or game events. */
  at(tick: number): Promise<void> | null {
    const meta = (chunkAt(this.segments, tick) ?? this.segments[0]!) as ArchiveSegment;
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

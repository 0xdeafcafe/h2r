// One cache for a viewer session. Seeking requests the target segment first.
// Fetch only the adjacent segment near an edge so interpolation has both poses.
import { chunkAt, recordingFrames, type ArchiveSegment, type ChunkIndex, type RecordingChunk } from "../api/chunk";
import type { Frame } from "../api/types";

export class ArchiveWindow {
  readonly segments: ArchiveSegment[];
  private readonly loaded = new Map<number, Frame[]>();
  private readonly pending = new Map<number, Promise<void>>();
  private current = -1;

  constructor(index: ChunkIndex, private readonly fetch: (c: RecordingChunk) => Promise<Uint8Array>) {
    this.segments = index.items.filter((m): m is ArchiveSegment => m.codec === "archive");
    if (!this.segments.length || this.segments.length !== index.items.length) throw new Error("empty or mixed archive index");
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
    if (existing) return existing;
    const request = this.fetch(meta).then(bytes => recordingFrames(bytes, meta)).then(frames => {
      if (Math.abs(i - this.current) <= 1) this.loaded.set(i, frames);
    }).finally(() => this.pending.delete(i));
    this.pending.set(i, request);
    return request;
  }

  /** Null when the current window is ready. The first request resolves
   * without waiting for a neighbour, other segments or game events. */
  at(tick: number): Promise<void> | null {
    const meta = (chunkAt(this.segments, tick) ?? this.segments[0]!) as ArchiveSegment;
    const changed = this.current !== meta.ordinal;
    this.current = meta.ordinal;
    if (changed) for (const i of this.loaded.keys()) {
      if (Math.abs(i - this.current) > 1) this.loaded.delete(i);
    }
    if (!this.loaded.has(meta.ordinal)) return this.load(meta);
    if (changed) return Promise.resolve();
    const neighbor = tick >= meta.to_tick - 60 ? this.segments[this.current + 1]
      : tick < meta.from_tick + 2 ? this.segments[this.current - 1] : undefined;
    return neighbor && !this.loaded.has(neighbor.ordinal) ? this.load(neighbor) : null;
  }
}

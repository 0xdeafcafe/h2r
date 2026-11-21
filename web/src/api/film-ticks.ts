// Project the public records onto the same frame and event types as live play.
import { ByteCursor } from "./pack-v8";
import { FilmCache } from "./film-cache";
import { decodeFilmLifecycle } from "./film-lifecycle";
import { FilmPoses } from "./film-poses";
import type { FilmSegment } from "./host-film";
import { applyFilmEffects, isFilmEffect } from "./film-fx";
import type { Frame, WireEvent } from "./types";

const utf8 = new TextDecoder("utf-8", {fatal: true});
type Body = Record<string, unknown>;
function object(v: unknown): v is Body { return v !== null && typeof v === "object" && !Array.isArray(v); }

interface FilmRecord { stamp: number; tag: number; data: Uint8Array; at: number }
function* records(bytes: Uint8Array, row: FilmSegment): Generator<FilmRecord> {
  const r = new ByteCursor(bytes);
  const first = r.uint(), count = r.uint();
  if (!first || !count || count > 128 || first !== row.first || first + count - 1 !== row.last)
    throw new Error("Host film time bounds disagree with index");
  let stamp = first;
  while (r.at < r.end) {
    stamp += r.uint();
    if (stamp > row.last || r.at >= r.end) throw new Error("Invalid host film timestamp");
    const tag = bytes[r.at++]!, size = r.uint();
    if (size > r.end - r.at) throw new Error("Truncated host film record");
    const at = r.at;
    r.at += size;
    if (tag === 2 || tag === 3 || tag === 5 || tag === 6) yield {stamp, tag, at, data: bytes.subarray(at, r.at)};
  }
}

function decodeWindow(bytes: Uint8Array, row: FilmSegment, poses: FilmPoses, game: string, shift: number, firstVisible: number, materialise: boolean, sampleGap: number) {
  const events: WireEvent[] = [], frames: Frame[] = [];
  const iterator = records(bytes, row);
  let next = iterator.next();
  let clock = -1;
  let clockRecords = 0;
  let baseline = false;
  let previousClockStamp = -1;
  for (let stamp = row.first; stamp <= row.last; stamp++) {
    const at = stamp + shift;
    const fx: {kind: string; data: Body; age: number}[] = [];
    let changed = false;
    const show = materialise && stamp >= firstVisible;
    const eventsAtTick: FilmRecord[] = [];
    while (!next.done && next.value.stamp === stamp) {
      const record = next.value;
      next = iterator.next();
      if (record.tag === 6) {
        if (stamp === previousClockStamp) throw new Error("Duplicate host film clock anchor");
        previousClockStamp = stamp;
        const cr = new ByteCursor(record.data);
        const guest = cr.end ? cr.uint() : null;
        if (cr.at !== cr.end) throw new Error("Invalid host film clock record");
        // The writer omits initial unknown clock anchors from the directory.
        if (guest !== null || clockRecords > 0) {
          const expected = row.clock[clockRecords++];
          if (!expected || expected[0] !== stamp - row.first || expected[1] !== guest)
            throw new Error("Host film guest clock disagrees with index");
        }
        continue;
      }
      if (record.tag !== 3) {
        if (stamp >= firstVisible) eventsAtTick.push(record);
        continue;
      }
      // Hold a resting pose until the sample preceding its first movement.
      if (show && !changed && frames.length && frames.at(-1)!.tick < at - sampleGap)
        frames.push(poses.frame(at - sampleGap));
      if (record.data[0] === 0 && stamp === row.first) baseline = true;
      poses.apply(at, record.data);
      changed = true;
    }
    if (row.clock[clock + 1]?.[0] === stamp - row.first) clock++;
    const anchor = row.clock[clock];
    const guestTick = anchor && anchor[1] !== null ? anchor[1] + stamp - row.first - anchor[0] : poses.matchTick(at) ?? at;
    for (const {data, tag, at: byte} of eventsAtTick) {
      const event = (type: string, body: Body, tick = at) => events.push({
        type, data: {...body, tick}, at: "", stream: "game-" + game,
        version: 0, position: row.ordinal * (16 << 20) + byte,
      });
      if (tag === 5) {
        event("FilmChapter", {label: utf8.decode(data)});
      } else if (tag === 2 && (data[0] === 0 || data[0] === 2)) {
        event("FilmLifecycle", {closing: data[0] === 2, changes: decodeFilmLifecycle(data.subarray(1))});
      } else if (tag === 2 && data[0] === 1) {
        if (data.length > 1 + (128 << 10)) throw new Error("Oversized host film event");
        const e: unknown = JSON.parse(utf8.decode(data.subarray(1)));
        if (!object(e) || typeof e.kind !== "string" || !object(e.data)) throw new Error("Invalid host film event");
        const d = e.data;
        const age = typeof e.tick === "number" && Number.isSafeInteger(e.tick) && e.tick <= guestTick
          ? Math.min(at - (firstVisible + shift), guestTick - e.tick) : 0;
        if (isFilmEffect(e.kind)) {
          fx.push({kind: e.kind, data: d, age});
          changed = true;
        } else if (["PlayerKilled", "ActorKilled", "ScoreChanged", "GameEnded"].includes(e.kind)) {
          event(e.kind, d, at - age);
        }
      }
    }
    if (show && (changed || !frames.length || stamp === row.last)) {
      const frame = poses.frame(at);
      if (fx.length) applyFilmEffects(frame, fx, poses);
      frames.push(frame);
    }
  }
  if (clockRecords !== row.clock.length || (row.scene === row.first && !baseline))
    throw new Error("Missing host film clock or scene checkpoint");
  return {frames, events};
}

/** A seek may require up to 16 short time chunks to seed the scene. Forward
 * playback hands the completed dictionary on to the next chunk. An aborted
 * request owns its own partial dictionary and cannot mutate its successor. */
export class Movie {
  readonly segments: FilmSegment[];
  private readonly first: number;
  private readonly shift: number;
  private readonly spans: {from: number; to: number; guest: number | null}[] = [];
  private readonly raw = new FilmCache<Uint8Array>(16, 8 << 20);
  private readonly lookup = new Map<number, number>();
  private cursor?: {next: number; scene: number; poses: FilmPoses};

  constructor(private readonly chunks: FilmSegment[], start: number | null, gameTick: number,
    private readonly game: string, private readonly fetch: (c: FilmSegment, s: AbortSignal) => Promise<Uint8Array>, readonly ticksPerSecond = 30) {
    this.first = start ?? chunks[0]?.first ?? 0;
    this.shift = gameTick - this.first;
    chunks.forEach((c, i) => this.lookup.set(c.first, i));
    this.segments = chunks.filter(c => c.last >= this.first).map((c, ordinal) => {
      const from_tick = Math.max(this.first, c.first) + this.shift;
      const anchors: [number, number | null][] = c.clock[0]?.[0] === 0 ? c.clock : [[0, null], ...c.clock];
      for (let i = 0; i < anchors.length; i++) {
        const [offset, guest] = anchors[i]!;
        const from = Math.max(from_tick, c.first + offset + this.shift);
        const to = c.first + (anchors[i + 1]?.[0] ?? c.last - c.first + 1) - 1 + this.shift;
        if (from <= to) this.spans.push({from, to, guest: guest === null ? null : guest + from - (c.first + offset + this.shift)});
      }
      return {...c, ordinal, from_tick, to_tick: c.last + this.shift, rawFrom: BigInt(from_tick)};
    });
  }

  /** Legacy clips and the live buffer name a match-clock tick. The first
   * chronological occurrence wins when a clock repeats. */
  playTick(guest: number): number {
    let best: {distance: number; tick: number} | undefined;
    for (const span of this.spans) {
      if (span.guest === null) continue;
      const start = span.guest;
      const end = start + span.to - span.from;
      if (guest >= start && guest <= end) return span.from + guest - start;
      const edge = guest < start ? span.from : span.to;
      const distance = Math.min(Math.abs(guest - start), Math.abs(guest - end));
      if (!best || distance < best.distance) best = {distance, tick: edge};
    }
    // Preserve points outside the film so playbackRange can reject or
    // intersect the clip. A point in a clock gap seeks the nearest edge.
    for (const span of this.spans) if (span.guest === null && guest >= span.from && guest <= span.to) return guest;
    const first = this.spans[0], last = this.spans.at(-1);
    if (first && guest < (first.guest ?? first.from)) return first.from + guest - (first.guest ?? first.from);
    if (last && guest > (last.guest ?? last.from) + last.to - last.from) return last.from + guest - (last.guest ?? last.from);
    return best?.tick ?? guest;
  }

  clipTick(tick: number): number {
    let lo = 0, hi = this.spans.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (this.spans[mid]!.from <= tick) lo = mid + 1;
      else hi = mid;
    }
    const span = this.spans[Math.max(0, lo - 1)];
    return span?.guest != null ? span.guest + tick - span.from : tick;
  }

  async read(row: FilmSegment, signal: AbortSignal) {
    const end = this.lookup.get(row.first)!, base = this.lookup.get(row.scene);
    if (base === undefined || base > end) throw new Error("Missing host film scene checkpoint");
    let start = base;
    let poses = new FilmPoses(this.game);
    if (this.cursor?.scene === row.scene && this.cursor.next >= base && this.cursor.next <= end) {
      start = this.cursor.next;
      poses = this.cursor.poses;
      this.cursor = undefined;
    }
    let frames: Frame[] = [];
    const events: WireEvent[] = [];
    for (let i = start; i <= end; i++) {
      const c = this.chunks[i]!;
      const cached = this.raw.get(i);
      const bytes = cached ?? await this.fetch(c, signal);
      signal.throwIfAborted();
      const block = decodeWindow(bytes, c, poses, this.game, this.shift, this.first, i === end, Math.ceil(this.ticksPerSecond / 15));
      if (!cached) this.raw.set(i, bytes, bytes.byteLength);
      frames = block.frames;
      for (const event of block.events) events.push(event);
    }
    this.cursor = {next: end + 1, scene: row.scene, poses};
    return {frames, events};
  }
}

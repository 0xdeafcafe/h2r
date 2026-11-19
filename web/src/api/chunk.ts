// Recorded frame chunks (docs/live/live-api.md, "Frame chunks"; the
// writer is crates/live-proto/src/chunk.rs). A game's frames are stored
// ten seconds of game time at a time: each chunk a packed stream (a
// keyframe, then deltas) compressed with raw deflate, behind a small
// header. The game's index (`GET /v1/games/{id}/chunks`) lists them, so a
// seek is the index plus one chunk.
//
// The chunk carries packed records as they are: a new packed frame version
// needs no change here, only in the frame decoder (pack.ts).

import { decodeArchive } from "./archive";
import type { Frame } from "./types";
import { FrameDecoder, PackError } from "./pack";

/** A chunk's content type (`GET /v1/games/{id}/chunks/{tick}`). */
export const CHUNK_TYPE = "application/x-h2-chunk";

/** The most bytes a chunk inflates to (the server's `MAX_RAW`). */
export const MAX_RAW = 16 << 20;

/** A chunk's entry in its game's index: the `FrameChunk` event's data. */
export interface ChunkMeta {
  from_tick: number;
  to_tick: number;
  frames: number;
  bytes: number;
  sha256: string;
  codec: "stored" | "deflate";
  raw_bytes: number;
  /** The packed frame versions inside. */
  versions: number[];
  players?: number[];
  /** Ticks of frames that carry a `cut`. */
  cuts?: number[];
  hits?: number;
  shots?: number;
}

export interface ArchiveSegment {
  codec: "archive";
  game: string;
  index: string;
  ordinal: number;
  from_tick: number;
  to_tick: number;
  rawFrom: bigint;
  frames: number;
  bytes: number;
  sha256: string;
  keyframes: number[];
}

export type RecordingChunk = ChunkMeta | ArchiveSegment;

/** `GET /v1/games/{id}/chunks`. */
export interface ChunkIndex {
  game_id: string;
  chunk_ticks: number;
  ended: boolean;
  items: RecordingChunk[];
  archive_sha256?: string;
}

export interface ChunkHeader {
  /** 0 stored, 1 raw deflate. */
  codec: number;
  rawBytes: number;
  frames: number;
  fromTick: number;
  toTick: number;
  /** The coded container. */
  body: Uint8Array;
}

const MAGIC = [0x48, 0x32, 0x43, 0x4b]; // "H2CK"
const FORMAT = 1;

/** The header, without inflating the body. */
export function chunkHeader(bytes: Uint8Array): ChunkHeader {
  if (bytes.length < 6 || MAGIC.some((b, i) => bytes[i] !== b)) throw new PackError("not a frame chunk");
  if (bytes[4] !== FORMAT) throw new PackError(`frame chunk format ${bytes[4]} is not supported`);
  const codec = bytes[5]!;
  if (codec !== 0 && codec !== 1) throw new PackError(`frame chunk codec ${codec} is not supported`);
  let at = 6;
  const leb = (): number => {
    let n = 0;
    for (let shift = 0; shift < 56; shift += 7) {
      if (at >= bytes.length) throw new PackError("frame chunk ends early");
      const b = bytes[at++]!;
      n += (b & 0x7f) * 2 ** shift;
      if ((b & 0x80) === 0) return n;
    }
    throw new PackError("frame chunk length too long");
  };
  const rawBytes = leb(), frames = leb(), fromTick = leb(), toTick = leb();
  if (rawBytes > MAX_RAW) throw new PackError("frame chunk too large");
  return { codec, rawBytes, frames, fromTick, toTick, body: bytes.subarray(at) };
}

/** Inflates raw deflate: the browser's own decoder. */
export type Inflate = (body: Uint8Array) => Promise<Uint8Array>;

export async function inflateRaw(body: Uint8Array): Promise<Uint8Array> {
  const ds = new DecompressionStream("deflate-raw");
  const w = ds.writable.getWriter();
  // Not awaited: the reader below drains it (awaiting would stall on a
  // body bigger than the stream's buffer).
  const input = body.buffer instanceof ArrayBuffer
    ? new Uint8Array(body.buffer, body.byteOffset, body.byteLength)
    : body.slice();
  w.write(input).catch(() => {});
  w.close().catch(() => {});
  return new Uint8Array(await new Response(ds.readable).arrayBuffer());
}

/** The chunk's container of packed records. */
export async function chunkContainer(bytes: Uint8Array, inflate: Inflate = inflateRaw): Promise<Uint8Array> {
  const h = chunkHeader(bytes);
  let raw: Uint8Array;
  try {
    raw = h.codec === 0 ? h.body : await inflate(h.body);
  } catch (e) {
    throw new PackError(`frame chunk does not inflate: ${(e as Error)?.message ?? e}`);
  }
  if (raw.length !== h.rawBytes) throw new PackError("frame chunk length does not match");
  return raw;
}

/** Every frame in a chunk, in order. */
export async function decodeChunk(bytes: Uint8Array, inflate: Inflate = inflateRaw): Promise<Frame[]> {
  const frames: Frame[] = [];
  new FrameDecoder().forEachRecord(await chunkContainer(bytes, inflate), frame => frames.push(frame));
  return frames;
}

/** The chunk holding `tick` (the last that starts at or before it), or
 *  null before the first: what a seek fetches. `index` is oldest first. */
export function chunkAt(index: readonly RecordingChunk[], tick: number): RecordingChunk | null {
  let lo = 0, hi = index.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (index[mid]!.from_tick <= tick) lo = mid + 1;
    else hi = mid;
  }
  return lo > 0 ? index[lo - 1]! : null;
}

/** The chunks with a frame in `from..=to` (either end open). */
export function chunksIn(index: readonly RecordingChunk[], from?: number, to?: number): RecordingChunk[] {
  return index.filter((c) => (from === undefined || c.to_tick >= from) && (to === undefined || c.from_tick <= to));
}

/** Reads the index body; null when it is not one. */
export function chunkIndex(body: unknown): ChunkIndex | null {
  const o = body as Partial<ChunkIndex> | null;
  if (!o || typeof o !== "object" || !Array.isArray(o.items)) return null;
  if (typeof o.archive_sha256 === "string") {
    const index = o.archive_sha256;
    if (!/^[a-f0-9]{64}$/.test(index) || typeof o.game_id !== "string" || !o.ended) throw new PackError("invalid archive index");
    const game = o.game_id;
    let previousRaw: bigint | undefined, last = 0;
    const tick = (v: unknown) => {
      if (typeof v === "number" && Number.isSafeInteger(v) && v >= 0) return BigInt(v);
      if (typeof v === "string" && /^[0-9]{1,20}$/.test(v)) {
        const n = BigInt(v);
        if (n <= 0xffffffffffffffffn) return n;
      }
      throw new PackError("invalid archive tick");
    };
    const items = o.items.map((s, ordinal): ArchiveSegment => {
      const lo = tick(s.from_tick), hi = tick(s.to_tick);
      if (hi < lo || hi - lo >= 900n || !Number.isInteger(s.frames) || s.frames < 1 || s.frames > 900 ||
          !/^[a-f0-9]{64}$/.test(s.sha256) || !("keyframes" in s) || !Array.isArray(s.keyframes) ||
          !s.keyframes.every((i: number) => Number.isInteger(i) && i >= 0 && i < s.frames)) throw new PackError("invalid archive segment index");
      const from = previousRaw === undefined ? (lo <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(lo) : 0)
        : last + (lo <= previousRaw ? 2 : Number(lo - previousRaw));
      last = from + Number(hi - lo);
      if (!Number.isSafeInteger(last)) throw new PackError("archive playback duration exceeds clock range");
      previousRaw = hi;
      return {codec:"archive", game, index, ordinal, from_tick:from, to_tick:last, rawFrom:lo,
        bytes:s.bytes, frames:s.frames, sha256:s.sha256, keyframes:s.keyframes};
    });
    return {game_id:game, chunk_ticks:900, ended:true, archive_sha256:index, items};
  }
  const items = o.items.filter((c): c is ChunkMeta => typeof c?.from_tick === "number" && typeof c?.to_tick === "number");
  items.sort((a, b) => a.from_tick - b.from_tick);
  return { game_id: String(o.game_id ?? ""), chunk_ticks: Number(o.chunk_ticks ?? 300), ended: o.ended === true, items };
}

/** Fetches and decodes `chunks`, at most `parallel` at a time, and returns
 *  their frames in order. `fetchChunk` gets a chunk's bytes. */
export async function recordingFrames(bytes: Uint8Array, meta: RecordingChunk, inflate: Inflate = inflateRaw): Promise<Frame[]> {
  if (meta.codec !== "archive") return decodeChunk(bytes, inflate);
  const frames = decodeArchive(bytes);
  if (frames.length !== meta.frames || BigInt(frames[0]!.tick) !== meta.rawFrom) throw new PackError("archive frame count or first tick differs");
  let last = -1;
  const output = frames.map((f): Frame => {
    const tick = meta.from_tick + Number(BigInt(f.tick) - meta.rawFrom);
    if (f.game_id !== meta.game || !Number.isSafeInteger(tick) || tick <= last || tick > meta.to_tick)
      throw new PackError("archive game or tick differs from index");
    last = tick;
    // Replay sampling only uses cut presence. Keep exact source integers in decodeArchive.
    f.tick = tick;
    if (typeof f.cut === "bigint") f.cut = Number(f.cut);
    return f as Frame;
  });
  if (last !== meta.to_tick) throw new PackError("archive last tick differs from index");
  return output;
}

export async function chunkFrames(
  chunks: readonly RecordingChunk[],
  fetchChunk: (c: RecordingChunk) => Promise<Uint8Array>,
  parallel = 6,
  inflate: Inflate = inflateRaw,
): Promise<Frame[]> {
  const out: Frame[][] = new Array(chunks.length);
  let next = 0;
  const worker = async () => {
    while (next < chunks.length) {
      const i = next++;
      const meta = chunks[i]!;
      out[i] = await recordingFrames(await fetchChunk(meta), meta, inflate);
    }
  };
  await Promise.all(Array.from({ length: Math.min(parallel, chunks.length) }, worker));
  return out.flat();
}

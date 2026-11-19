// Recorded frame chunks (docs/live/live-api.md, "Frame chunks"; the
// writer is crates/live-proto/src/chunk.rs). A game's frames are stored
// ten seconds of game time at a time: each chunk a packed stream (a
// keyframe, then deltas) compressed with raw deflate, behind a small
// header. The game's index (`GET /v1/games/{id}/chunks`) lists them, so a
// seek is the index plus one chunk.
//
// The chunk carries packed records as they are: a new packed frame version
// needs no change here, only in the frame decoder (pack.ts).

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

/** `GET /v1/games/{id}/chunks`. */
export interface ChunkIndex {
  game_id: string;
  chunk_ticks: number;
  ended: boolean;
  items: ChunkMeta[];
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
  w.write(body.slice()).catch(() => {});
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
export function chunkAt(index: readonly ChunkMeta[], tick: number): ChunkMeta | null {
  let lo = 0, hi = index.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (index[mid]!.from_tick <= tick) lo = mid + 1;
    else hi = mid;
  }
  return lo > 0 ? index[lo - 1]! : null;
}

/** The chunks with a frame in `from..=to` (either end open). */
export function chunksIn(index: readonly ChunkMeta[], from?: number, to?: number): ChunkMeta[] {
  return index.filter((c) => (from === undefined || c.to_tick >= from) && (to === undefined || c.from_tick <= to));
}

/** Reads the index body; null when it is not one. */
export function chunkIndex(body: unknown): ChunkIndex | null {
  const o = body as Partial<ChunkIndex> | null;
  if (!o || typeof o !== "object" || !Array.isArray(o.items)) return null;
  const items = o.items.filter((c): c is ChunkMeta => typeof c?.from_tick === "number" && typeof c?.to_tick === "number");
  items.sort((a, b) => a.from_tick - b.from_tick);
  return { game_id: String(o.game_id ?? ""), chunk_ticks: Number(o.chunk_ticks ?? 300), ended: o.ended === true, items };
}

/** Fetches and decodes `chunks`, at most `parallel` at a time, and returns
 *  their frames in order. `fetchChunk` gets a chunk's bytes. */
export async function chunkFrames(
  chunks: readonly ChunkMeta[],
  fetchChunk: (c: ChunkMeta) => Promise<Uint8Array>,
  parallel = 6,
  inflate: Inflate = inflateRaw,
): Promise<Frame[]> {
  const out: Frame[][] = new Array(chunks.length);
  let next = 0;
  const worker = async () => {
    while (next < chunks.length) {
      const i = next++;
      out[i] = await decodeChunk(await fetchChunk(chunks[i]!), inflate);
    }
  };
  await Promise.all(Array.from({ length: Math.min(parallel, chunks.length) }, worker));
  return out.flat();
}

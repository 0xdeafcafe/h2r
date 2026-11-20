// Public v3 film ranges. Read the menu, directory and one time window without
// downloading the engine snapshot or replaying the match.
import type { TimelineSegment } from "./chunk";

const FRONT_SIZE = 32;
const FRONT_CAPACITY = 128 << 10;
const FOOTER_SIZE = 40;
const MAX_RAW = 8 << 20;
const MAX_PACKED = MAX_RAW + (64 << 10);
const UTF8 = new TextDecoder("utf-8", {fatal: true});
const modes = ["custom", "matchmaking", "system_link", "coop_campaign", "campaign", "unknown"] as const;

export interface HostFilmSummary {
  level: string;
  game_type: string;
  mode: typeof modes[number];
  variant_name: string;
  duration_ticks: number;
  dropped_samples?: Record<string, number>;
  players: {seat: number; name: string; xuid?: string | null; team: number; score: number; medals: Record<string, number>}[];
}

export interface FilmSeat {
  seat: number;
  name: string;
  team: number;
  kind: "local" | "remote" | "phantom" | "bot";
}

export interface FilmFront {
  id: string;
  image_stamp: string;
  runtime_stamp: string;
  started_ms: number;
  map_sha256: string;
  map_origin: string;
  roster: FilmSeat[];
  ticks_per_second?: number | null;
  match_start_tick: number | null;
  summary: HostFilmSummary;
}

export interface FilmSegment extends TimelineSegment {
  first: number;
  last: number;
  scene: number;
  clock: [number, number | null][];
  offset: number;
  size: number;
  raw_size: number;
}

type Read = (range: string, signal?: AbortSignal) => Promise<Response>;
export type FilmInflate = (bytes: Uint8Array, rawSize: number) => Promise<Uint8Array>;
let decoder: Promise<import("three/addons/libs/zstddec.module.js").ZSTDDecoder> | undefined;

async function expand(bytes: Uint8Array, size: number): Promise<Uint8Array> {
  decoder ??= import("three/addons/libs/zstddec.module.js").then(async ({ZSTDDecoder}) => {
    const d = new ZSTDDecoder();
    await d.init();
    return d;
  });
  return (await decoder).decode(bytes, size);
}

function bad(why: string): never { throw new Error("Invalid host film: " + why); }
function integer(v: unknown): v is number { return typeof v === "number" && Number.isSafeInteger(v); }
function uint(v: unknown): v is number { return integer(v) && v >= 0; }
function object(v: unknown): v is Record<string, unknown> { return v !== null && typeof v === "object" && !Array.isArray(v); }
function magic(b: Uint8Array, s: string): boolean {
  return b.length >= s.length && [...s].every((c, i) => b[i] === c.charCodeAt(0));
}
function view(b: Uint8Array): DataView { return new DataView(b.buffer, b.byteOffset, b.byteLength); }

export function decodeFilmFront(bytes: Uint8Array): FilmFront {
  if (!bytes.length || bytes.length > FRONT_CAPACITY) bad("front size");
  const f: unknown = JSON.parse(UTF8.decode(bytes));
  if (!object(f) || typeof f.id !== "string" || !(f.match_start_tick === null || uint(f.match_start_tick))) bad("front header");
  if (typeof f.image_stamp !== "string" || typeof f.runtime_stamp !== "string" || !uint(f.started_ms) ||
    typeof f.map_sha256 !== "string" || typeof f.map_origin !== "string" || !Array.isArray(f.roster) ||
    !f.roster.every(p => object(p) && uint(p.seat) && p.seat < 16 && typeof p.name === "string" &&
      integer(p.team) && (p.kind === "local" || p.kind === "remote" || p.kind === "phantom" || p.kind === "bot"))) bad("header roster or identity");
  if (f.ticks_per_second != null && (!uint(f.ticks_per_second) || !f.ticks_per_second || f.ticks_per_second > 65535)) bad("tick rate");
  const s = f.summary;
  if (!object(s) || typeof s.level !== "string" || typeof s.game_type !== "string" ||
    typeof s.variant_name !== "string" || !modes.some(m => m === s.mode) ||
    !uint(s.duration_ticks) || !Array.isArray(s.players)) bad("summary");
  if (s.dropped_samples !== undefined && (!object(s.dropped_samples) || !Object.values(s.dropped_samples).every(uint)))
    bad("dropped sample counts");
  for (const p of s.players) {
    if (!object(p) || !uint(p.seat) || p.seat > 15 || typeof p.name !== "string" ||
      !(p.xuid == null || typeof p.xuid === "string") || !integer(p.team) || !integer(p.score) ||
      !object(p.medals) || !Object.values(p.medals).every(uint)) bad("summary player");
  }
  return f as unknown as FilmFront;
}

interface Slice { bytes: Uint8Array; total: number }

/** Reading through a bounded buffer also rejects a chunked HTTP response
 * that exceeds its advertised range before it allocates a whole film. */
async function slice(response: Response, start: number | null, length: number, total?: number): Promise<Slice> {
  const m = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.headers.get("content-range") ?? "");
  const [a,b,n] = [Number(m?.[1]), Number(m?.[2]), Number(m?.[3])];
  if (response.status !== 206 || !m || !uint(a) || !uint(b) || !uint(n) ||
    a !== (start ?? n - length) || b !== a + length - 1 || b >= n || (total !== undefined && n !== total)) {
    await response.body?.cancel();
    bad("HTTP range");
  }
  const reader = response.body?.getReader();
  if (!reader) bad("empty HTTP response");
  const out = new Uint8Array(length);
  let at = 0;
  try {
    for (;;) {
      const {value, done} = await reader.read();
      if (done) break;
      if (value.length > length - at) { await reader.cancel(); bad("long HTTP response"); }
      out.set(value, at);
      at += value.length;
    }
  } finally { reader.releaseLock(); }
  if (at !== length) bad("short HTTP response");
  return {bytes: out, total: n};
}

export function decodeFilmIndex(bytes: Uint8Array, indexOffset: number): FilmSegment[] {
  const v: unknown = JSON.parse(UTF8.decode(bytes));
  if (!object(v) || v.version !== 1 || !Array.isArray(v.chunks) || !Array.isArray(v.keyframes) || v.chunks.length + v.keyframes.length > 1_000_000) bad("time index");
  let priorKeyTick = -1, priorKeyOffset = FRONT_SIZE + FRONT_CAPACITY - 1;
  for (const k of v.keyframes) {
    if (!object(k) || !uint(k.tick) || k.tick <= priorKeyTick || !uint(k.offset) ||
      k.offset <= priorKeyOffset || k.offset >= indexOffset) bad("private keyframe reference");
    priorKeyTick = k.tick;
    priorKeyOffset = k.offset;
  }
  let end = FRONT_SIZE + FRONT_CAPACITY;
  let previous = 0;
  let base = 0, baseOrdinal = 0;
  return v.chunks.map((entry: unknown, ordinal): FilmSegment => {
    if (!object(entry)) bad("time index row");
    const {first, last, offset, size, raw_size} = entry;
    if (!uint(first) || !uint(last) || first !== previous + 1 || last < first || last - first >= 128 ||
      !uint(size) || !size || size > MAX_PACKED || !uint(raw_size) || !raw_size || raw_size > MAX_RAW ||
      !uint(offset) || offset < end || offset + size + 8 > indexOffset) bad("time index bounds");
    const scene = entry.scene;
    if (scene === first) { base = first; baseOrdinal = ordinal; }
    if (!uint(scene) || !base || scene !== base || ordinal - baseOrdinal >= 16 || last - scene >= 2048) bad("scene checkpoint");
    const clock = entry.clock ?? [];
    let previousOffset = -1;
    if (!Array.isArray(clock) || !clock.every(pair => {
      if (!Array.isArray(pair) || pair.length !== 2) return false;
      const [i, t] = pair;
      if (!uint(i) || i <= previousOffset || i > last - first || !(t === null || (uint(t) && Number.isSafeInteger(t + last - first - i)))) return false;
      previousOffset = i;
      return true;
    })) bad("guest clock");
    end = offset + 8 + size;
    previous = last;
    return {first, last, scene, clock, offset, size, raw_size, ordinal, from_tick: first, to_tick: last, rawFrom: BigInt(first)};
  });
}

async function unpack(bytes: Uint8Array, tag: string, rawSize: number, inflate: FilmInflate) {
  if (bytes.length < 8 || !magic(bytes, tag) || view(bytes).getUint32(4, true) !== bytes.length - 8) bad(tag + " envelope");
  const raw = await inflate(bytes.subarray(8), rawSize);
  if (raw.length !== rawSize) bad(tag + " inflated size");
  return raw;
}

export async function openHostFilm(read: Read, signal?: AbortSignal, inflate: FilmInflate = expand) {
  const probe = await read("bytes=0-31", signal);
  // On servers with the participant-only download route even a public
  // game can answer 401/403 to the probe. Its telemetry has its own ACL.
  if ([200, 401, 403, 404, 405, 416].includes(probe.status)) { await probe.body?.cancel(); return null; }
  if (!probe.ok) {
    await probe.body?.cancel();
    throw new Error(`Host film request failed (HTTP ${probe.status})`);
  }
  const prefix = await slice(probe, 0, FRONT_SIZE);
  if (!magic(prefix.bytes, "H2BLAM3!")) bad("front magic");
  const head = view(prefix.bytes);
  if (head.getUint32(8, true) !== 3 || head.getUint32(12, true) !== FRONT_CAPACITY ||
    head.getUint32(20, true) !== 0 || head.getBigUint64(24, true) !== 0n) bad("version or flags");
  const size = head.getUint32(16, true);
  if (!size || size > FRONT_CAPACITY || prefix.total < FRONT_SIZE + FRONT_CAPACITY + FOOTER_SIZE) bad("front size");
  const range = async (a: number, length: number, s?: AbortSignal) =>
    (await slice(await read(`bytes=${a}-${a + length - 1}`, s), a, length, prefix.total)).bytes;
  const front = decodeFilmFront(await range(FRONT_SIZE, size, signal));
  // Directory access is lazy: callers listing a film need just its prefix and JSON.
  const index = async (s?: AbortSignal) => {
    const {bytes} = await slice(await read("bytes=-40", s), null, FOOTER_SIZE, prefix.total);
    if (!magic(bytes, "H2RANGE3")) bad("footer magic");
    const f = view(bytes);
    const offset = Number(f.getBigUint64(8, true)), next = Number(f.getBigUint64(16, true));
    const rawSize = f.getUint32(32, true), size = next - offset;
    if (!uint(offset) || !uint(next) || offset < FRONT_SIZE + FRONT_CAPACITY || size <= 8 || size > 8 + (16 << 20) ||
      next + 8 >= prefix.total - FOOTER_SIZE || f.getBigUint64(24, true) !== BigInt(prefix.total) ||
      !rawSize || rawSize > (8 << 20) || f.getUint32(36, true) !== 0) bad("footer bounds");
    return decodeFilmIndex(await unpack(await range(offset, size, s), "CIDX", rawSize, inflate), offset);
  };
  const chunk = async (c: FilmSegment, s?: AbortSignal) =>
    unpack(await range(c.offset, c.size + 8, s), "TICK", c.raw_size, inflate);
  return {front, index, chunk};
}

export type HostFilm = NonNullable<Awaited<ReturnType<typeof openHostFilm>>>;

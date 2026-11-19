// H2AR v1, the post-HTTP-decompression bytes of a finished replay segment.
// The Rust writer and its exact integer predictors live in live-proto/archive.
import { PackError, unoct } from "./pack";
import type { Frame } from "./types";

type Int = number | bigint;
type Row = { [field: string]: Value };
type Value = Int | boolean | string | Row | Value[];
export type ExactArchiveFrame = Omit<Frame, "tick" | "cut"> & { tick: Int; cut?: Int };
const TAU = 2 * Math.PI;
const DIR_MAX = 16382;
const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const MAX_BODY = 16 << 20;

function bad(): never { throw new PackError("invalid replay archive"); }

class Reader {
  at = 0;
  nodes = 2_000_000;
  strings = MAX_BODY;
  constructor(readonly bytes: Uint8Array) {}
  byte(): number { return this.bytes[this.at++] ?? bad(); }
  take(n: number): Uint8Array {
    if (n < 0 || n > this.bytes.length - this.at) bad();
    this.at += n;
    return this.bytes.subarray(this.at - n, this.at);
  }
  var(): Int {
    let low = 0;
    for (let shift = 0; shift < 49; shift += 7) {
      const b = this.byte();
      low += (b & 127) * 2 ** shift;
      if (b < 128) return low;
    }
    let wide = BigInt(low);
    for (let shift = 49; shift <= 63; shift += 7) {
      const b = this.byte();
      if (shift === 63 && b > 1) bad();
      wide |= BigInt(b & 127) << BigInt(shift);
      if (b < 128) return wide;
    }
    return bad();
  }
  count(max: number): number {
    const n = this.var();
    return typeof n === "number" && n <= max ? n : bad();
  }
  signed(): Int {
    const z = this.var();
    if (typeof z === "number") return z % 2 === 0 ? z / 2 : -(z + 1) / 2;
    return (z >> 1n) ^ -(z & 1n);
  }
  string(): {value: string; bytes: number} {
    const bytes = this.count(MAX_BODY);
    return {value: UTF8.decode(this.take(bytes)), bytes};
  }
  charge(n: number) { this.strings -= n; if (this.strings < 0) bad(); }
}

/** Rust/JS use the same binary64 operations for the fitted curve. */
function sin(x: number): number {
  let a = x - TAU * Math.floor(x / TAU + 0.5);
  if (a > Math.PI / 2) a = Math.PI - a;
  if (a < -Math.PI / 2) a = -Math.PI - a;
  const z = -a * a;
  let p = 1;
  for (let k = 9; k >= 1; k--) p = 1 + z * p / ((2 * k) * (2 * k + 1));
  return a * p;
}

function add(a: Int, b: Int): Int {
  if (typeof a === "number" && typeof b === "number") {
    const sum = a + b;
    if (Number.isSafeInteger(sum)) return sum;
  }
  return BigInt.asIntN(64, BigInt(a) + BigInt(b));
}

function numbers(r: Reader, n: number): Int[] {
  const mode = r.byte();
  const out = new Array<Int>(n);
  if (mode === 0) return out.fill(r.signed());
  if (mode === 5) {
    const freq = Number(r.signed()), a = Number(r.signed()), b = Number(r.signed()), c = Number(r.signed());
    if (freq < 0 || freq > 13_500_000_000 || Math.max(Math.abs(a), Math.abs(b), Math.abs(c)) > 2 ** 44) bad();
    for (let i = 0; i < n; i++) {
      const w = i * (freq / 4294967296);
      const pred = Math.floor((a + b * sin(w + Math.PI / 2) + c * sin(w)) / 128 + 0.5);
      out[i] = add(pred, r.signed());
    }
  } else {
    if (mode < 1 || mode > 4) bad();
    const prev: Int[] = [0, 0, 0];
    for (let i = 0; i < n; i++) {
      let v = r.signed();
      for (let j = mode - 2; j >= 0; j--) prev[j] = v = add(prev[j]!, v);
      out[i] = v;
    }
  }
  return out;
}

function node(r: Reader, n: number, depth: number): Value[] {
  if (!n || depth > 10 || (r.nodes -= n) < 0) bad();
  const tag = r.byte();
  if (tag === 0) {
    const rows = Array.from({ length: n }, (): Row => Object.create(null) as Row);
    const keys = r.count(128), seen = new Set<string>();
    for (let j = 0; j < keys; j++) {
      const {value:key, bytes:keyBytes} = r.string();
      if (seen.has(key)) bad();
      seen.add(key);
      const presence = r.byte();
      if (presence > 1) bad();
      const mask = presence ? r.take(Math.ceil(n / 8)) : undefined;
      let ct = n;
      if (mask) {
        ct = 0;
        for (let i = 0; i < n; i++) ct += (mask[i >> 3]! >> (i % 8)) & 1;
      }
      r.charge(ct * keyBytes);
      const col = node(r, ct, depth + 1);
      let pos = 0;
      for (let i = 0; i < n; i++) if (!mask || mask[i >> 3]! & (1 << (i % 8))) rows[i]![key] = col[pos++]!;
    }
    return rows;
  }
  if (tag === 1) {
    const lens = numbers(r, n).map(Number);
    let width = 0, total = 0;
    for (const v of lens) {
      if (v < 0 || v > 4096) bad();
      total += v;
      width = Math.max(width, v);
    }
    if (total > r.nodes) bad();
    const rows = lens.map((): Value[] => []);
    for (let j = 0; j < width; j++) {
      let ct = 0;
      for (const l of lens) if (l > j) ct++;
      const col = node(r, ct, depth + 1);
      let pos = 0;
      for (let i = 0; i < n; i++) if (lens[i]! > j) rows[i]!.push(col[pos++]!);
    }
    return rows;
  }
  if (tag === 2) {
    const d = r.count(n);
    if (!d) bad();
    const dict = Array.from({ length: d }, () => r.string());
    if (d === 1) {
      const s = dict[0]!;
      r.charge(n * s.bytes);
      return new Array<string>(n).fill(s.value);
    }
    return numbers(r, n).map(v => {
      const s = dict[Number(v)] ?? bad();
      r.charge(s.bytes);
      return s.value;
    });
  }
  if (tag === 3) return numbers(r, n).map(q => q === 0 ? false : q === 1 ? true : bad());
  if (tag === 4) {
    const signed = r.byte();
    if (signed > 1) bad();
    return numbers(r, n).map(q => {
      if (typeof q === "number" && (q >= 0 || signed)) return q;
      const wide = signed ? BigInt(q) : BigInt.asUintN(64, BigInt(q));
      const approx = Number(wide);
      return Number.isSafeInteger(approx) ? approx : wide;
    });
  }
  if (tag === 5) {
    const scale = r.count(65536);
    const raw = new DataView(new ArrayBuffer(8));
    return numbers(r, n).map(q => {
      if (scale) return Number(q) / scale;
      raw.setBigInt64(0, BigInt(q));
      const x = raw.getFloat64(0);
      return Number.isFinite(x) ? x : bad();
    });
  }
  return bad();
}

function oct(x: number, y: number, z: number): [number, number] {
  const l = Math.abs(x) + Math.abs(y) + Math.abs(z);
  if (!Number.isFinite(l) || l < 1e-12) bad();
  x /= l; y /= l;
  if (z < 0) {
    const t = (1 - Math.abs(y)) * (x >= 0 ? 1 : -1);
    y = (1 - Math.abs(x)) * (y >= 0 ? 1 : -1);
    x = t;
  }
  const q = (v: number) => Math.round((Math.min(1, Math.max(-1, v)) + 1) * 0.5 * DIR_MAX);
  return [q(x), q(y)];
}

function row(v: Value | undefined): Row {
  return v !== null && typeof v === "object" && !Array.isArray(v) ? v : bad();
}

function restore(f: Row): ExactArchiveFrame {
  if (typeof f.game_id !== "string" || !Array.isArray(f.players) ||
      !(typeof f.tick === "bigint" || typeof f.tick === "number")) bad();
  for (const [name, list] of Object.entries(f)) {
    if (!Array.isArray(list)) continue;
    let ref: number[] | undefined, heading = 0;
    for (let i = 0; i < list.length; i++) {
      const e = row(list[i]);
      if (e.$y !== undefined) {
        const q = e.$y;
        if (typeof q !== "number" || !Number.isInteger(q) || q < 0 || q >= 16384) bad();
        const a = q * TAU / 16384;
        e.yaw = a > Math.PI ? a - TAU : a;
        delete e.$y;
      }
      for (const k of ["aim", "forward", "up", "f", "u", "r"]) {
        const tag = "$" + k, xy = e[tag];
        if (xy === undefined) continue;
        if (!Array.isArray(xy) || xy.length !== 2 || typeof xy[0] !== "number" || typeof xy[1] !== "number") bad();
        let x = xy[0], y = xy[1];
        if (k === "r") {
          if (!ref || typeof e.yaw !== "number") bad();
          const d = e.yaw - heading, c = sin(d + Math.PI / 2), s = sin(d);
          const base = oct(ref[0]! * c - ref[1]! * s, ref[0]! * s + ref[1]! * c, ref[2]!);
          x += base[0]; y += base[1];
        }
        if (!Number.isInteger(x) || !Number.isInteger(y) || x < 0 || x > DIR_MAX || y < 0 || y > DIR_MAX) bad();
        const v = unoct(x, y, DIR_MAX);
        e[k === "r" ? "aim" : k] = v;
        delete e[tag];
        if (!i && (name === "players" || name === "actors") && k === "aim" && typeof e.yaw === "number") {
          ref = v; heading = e.yaw;
        }
      }
    }
  }
  if (Array.isArray(f.hits)) for (const h of f.hits) {
    const hit = row(h);
    if (hit.$p !== undefined) {
      if (typeof hit.$p !== "number") bad();
      hit.pos = row(f.players[hit.$p]).pos ?? bad();
      delete hit.$p;
    }
  }
  return f as unknown as ExactArchiveFrame;
}

/** Decode fetch().arrayBuffer() after HTTP Content-Encoding: br. Unsafe
 *  integral ticks remain bigint until mapped onto the playback clock. */
export function decodeArchive(bytes: Uint8Array): ExactArchiveFrame[] {
  const r = new Reader(bytes);
  if (bytes.length > MAX_BODY || ![72, 50, 65, 82, 1].every(b => r.byte() === b)) bad();
  const frames = node(r, r.count(900), 0).map(v => restore(row(v)));
  if (r.at !== bytes.length) bad();
  return frames;
}

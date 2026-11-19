// v8 keeps a causal predictor per list, row ordinal and field. Keys seed the
// predictor and dictionary from the embedded v7 frame, leaving entropy priors fresh.
import { K, LISTS, HIT, PackError, dir, dyaw, idText, materialize,
  type Direction, type Entity, type Field, type Val, type Vec3 } from "./pack";
import { oct, rotatedOct } from "./pack-math";
import { RangeModels, RangeReader } from "./pack-range";
import type { Frame } from "./types";

const WIDTH = 128, LIST_WIDTH = 24 * WIDTH, META = 8 * LIST_WIDTH;
const MAX_DIR = 16382;
const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const PLAYER_HOT = [2, 3, 6, 8, 13, 14, 16, 20].reduce((a, b) => a | (1 << b), 0);
const VEHICLE_HOT = [2, 3, 4, 7, 8].reduce((a, b) => a | (1 << b), 0);

function bad(): never { throw new PackError("invalid v8 packed frame"); }
function bounded(n: number, max: number) { return n >= 0 && n <= max && Number.isSafeInteger(n) ? n : bad(); }
function text(bytes: Uint8Array): string {
  try { return UTF8.decode(bytes); } catch { return bad(); }
}

export class ByteCursor {
  constructor(readonly bytes: Uint8Array, public at = 0, readonly end = bytes.length) {}
  uint(): number {
    let value = 0;
    for (let shift = 0; shift <= 49 && this.at < this.end; shift += 7) {
      const b = this.bytes[this.at++]!;
      if (b === 0 && shift > 0) bad();
      value += (b & 127) * 2 ** shift;
      if (b < 128) return bounded(value, Number.MAX_SAFE_INTEGER);
    }
    return bad();
  }
}

function blob(r: RangeReader, ctx: number, max: number): Uint8Array {
  const bytes = new Uint8Array(bounded(r.uint(ctx), max));
  for (let i = 0; i < bytes.length; i++) bytes[i] = r.literal();
  return bytes;
}

function literal(kind: K, bytes: Uint8Array): Val {
  if (kind === K.Str) return text(bytes);
  if (kind === K.Id) return { text: text(bytes) };
  if (kind === K.Actions) {
    if (bytes.length > 8 || bytes.some(n => n >= 6)) bad();
    return Array.from(bytes);
  }
  const r = new ByteCursor(bytes), ages = [];
  while (r.at < r.end) {
    if (ages.length === 8) bad();
    ages.push(bounded(r.uint(), 0xffffffff));
  }
  return ages;
}

// Each field owns three previous components, three slopes, presence and
// relative-aim mode. Storage follows the high water mark of each list.
class Rows {
  data = new Float64Array(0);
  entities: Entity[] = [];
  resize(n: number, fields: number): void {
    const used = n * fields * 8;
    if (used > this.data.length) {
      const grown = new Float64Array(Math.max(used, this.data.length * 2));
      grown.set(this.data);
      this.data = grown;
    } else if (n < this.entities.length) this.data.fill(0, used, this.entities.length * fields * 8);
  }
}

export class V8Stream {
  private readonly models = new RangeModels(META + 256);
  private readonly rows = Array.from({ length: 8 }, () => new Rows());
  private readonly dictionary: { kind: K; value: Val }[] = [];
  private lists: Entity[][] = [];
  private readonly q = [0, 0, 0];
  private readonly travel = [0, 0, 0];
  private game: string;
  private bsp?: number;
  private cut?: number;
  private cinematic: boolean;

  constructor(private frame: Frame, lists: Entity[][], hits: Entity[], public seq: number) {
    this.game = frame.game_id;
    this.bsp = frame.bsp ?? undefined;
    this.cut = frame.cut ?? undefined;
    this.cinematic = !!frame.cinematic;
    const ids = new Map<string, number>();
    for (let j = 0; j < 8; j++) {
      this.list(j, 0, undefined, j === 7 ? hits : lists[j], ids);
    }
    this.lists = this.rows.map(h => h.entities);
  }

  private dictionaryValue(kind: K, ctx: number, history: Float64Array, off: number,
                          r?: RangeReader, seed?: Val, ids?: Map<string, number>): Val {
    let id: number;
    if (r) id = bounded(history[off]! + r.signed(ctx), this.dictionary.length);
    else {
      const key = kind + ":" + (kind === K.Id ? idText(seed!) : String(seed));
      id = ids!.get(key) ?? this.dictionary.length;
      ids!.set(key, id);
    }
    if (id === this.dictionary.length) {
      if (id >= 16384) bad();
      const value = r ? literal(kind, blob(r, META + 64, 255)) : seed!;
      this.dictionary.push({ kind, value });
    }
    const entry = this.dictionary[id]!;
    if (entry.kind !== kind) bad();
    history[off] = id;
    history[off + 6] = 1;
    return entry.value;
  }

  private list(j: number, dt: number, r?: RangeReader, source?: Entity[], ids?: Map<string, number>): void {
    const rows = this.rows[j]!, before = rows.entities;
    const base = j * LIST_WIDTH, lengths = base + 23 * WIDTH;
    if (r && (j === 3 || j === 4 || j === 6) && r.bit(lengths + 40)) return;
    const fields = j === 7 ? HIT : LISTS[j]![0];
    const max = j === 7 ? 64 : LISTS[j]![1];
    const n = r ? bounded(before.length + r.signed(lengths), max) : source!.length;
    rows.resize(n, fields.length);
    const h = rows.data, hot = j === 0 ? PLAYER_HOT : j === 1 ? VEHICLE_HOT : 0;
    const unit = j === 0 || j === 2, aim = j === 0 ? 6 : 4;
    const velocity = unit ? 13 : j === 1 ? 8 : j === 5 ? 3 : -1;
    let next = before.length === n ? before : before.slice(0, n);
    let firstAim: Vec3 | undefined, heading = 0;
    for (let i = 0; i < n; i++) {
      const old = before[i], seed = source?.[i];
      let row = old ?? new Array<Val>(fields.length);
      const held = hot !== 0 && r?.bit(lengths + 41);
      if (held && !old) bad();
      const start = i * fields.length * 8, v = start + velocity * 8;
      const moving = velocity >= 0 && h[v + 6] === 1 && dt <= 30;
      if (moving) for (let k = 0; k < 3; k++) {
        const d = (2 * h[v + k]! + (unit ? h[v + k + 3]! : 0)) * 4 * dt;
        this.travel[k] = Math.trunc((d + Math.sign(d) * 15) / 30);
      }
      for (let f = 0; f < fields.length; f++) {
        if (held && !(hot & (1 << f))) continue;
        const off = start + f * 8, field = fields[f]!, ctx = base + f * WIDTH;
        const s = seed?.[f], was = h[off + 6]!;
        let value: Val;
        if (field.opt && !(r ? r.bit(ctx + was) : s != null)) {
          if (was) h.fill(0, off, off + 8);
          value = null;
        } else if (field.kind === K.Str || field.kind === K.Id || field.kind === K.Actions || field.kind === K.Agos) {
          value = this.dictionaryValue(field.kind, ctx + 8, h, off, r, s, ids);
        } else if (field.kind === K.Bool) {
          value = r ? r.bit(ctx + 2 + Number(h[off] !== 0)) : !!s;
          h[off] = Number(value);
          h[off + 6] = 1;
        } else {
          const shared = firstAim && f === aim && field.kind === K.Dir
            ? rotatedOct(firstAim, dyaw(row[3] as number) - heading) : undefined;
          value = this.numeric(field, ctx, h, off, old?.[f], r, s, shared, moving, j === 7);
        }
        if (row[f] !== value) {
          if (row === old) row = row.slice();
          row[f] = value;
        }
      }
      if (next[i] !== row) {
        if (next === before) next = before.slice();
        next[i] = row;
      }
      if (i === 0 && unit && row[aim] != null) {
        firstAim = dir(row[aim]!);
        heading = dyaw(row[3] as number);
      }
    }
    rows.entities = next;
  }

  private numeric(field: Field, ctx: number, h: Float64Array, off: number, old: Val | undefined,
                  r: RangeReader | undefined, seed: Val | undefined, shift: number[] | undefined,
                  moving: boolean, hit: boolean): Val {
    const k = field.kind, vector = k === K.Pos || k === K.Vel, direction = k === K.Dir;
    const scale = k === K.Pos ? 1024 : 128;
    if (hit && k === K.Pos && this.rows[0]!.entities.length) {
      const players = this.rows[0]!.entities;
      if (r) {
        if (r.bit(ctx + 4)) return players[bounded(r.uint(ctx + 8), players.length - 1)]![2]!;
      } else {
        const p = seed as Vec3;
        for (const e of players) {
          const v = e[2] as Vec3;
          if (p[0] === v[0] && p[1] === v[1] && p[2] === v[2]) return v;
        }
      }
    }
    const q = this.q;
    if (!r && direction) [q[0], q[1]] = oct(...dir(seed!));
    const shared = !!shift && (r ? r.bit(ctx + 5 + h[off + 7]!)
      : Math.abs(q[0]! - shift[0]!) <= 16 && Math.abs(q[1]! - shift[1]!) <= 16);
    if (Number(shared) !== h[off + 7]) {
      h.fill(0, off, off + 8);
      h[off + 7] = Number(shared);
    }
    const was = h[off + 6]!, count = vector ? 3 : direction ? 2 : 1;
    for (let i = 0; i < count; i++) {
      const previous = h[off + i]!, correction = shared ? shift![i]! : 0;
      let n: number;
      if (r) {
        let prediction = previous;
        if (k === K.Pos && moving) prediction += this.travel[i]!;
        else if (vector || direction || k === K.Angle || k === K.AnimPhase) prediction += h[off + i + 3]!;
        n = prediction + r.signed(ctx + 8 + i * 32);
        if (k === K.Angle) n = n & 16383;
      } else n = vector ? (seed as Vec3)[i]! * scale : direction ? q[i]! - correction : seed as number;
      h[off + i + 3] = was ? n - previous : 0;
      h[off + i] = n;
      n += correction;
      if (vector) { if (Math.abs(n) > 1e9 * scale) bad(); q[i] = n / scale; }
      else q[i] = bounded(n, direction ? MAX_DIR : k === K.Bits ? 2 ** field.arg! - 1 :
        k === K.AnimPhase ? 32767 * 16 : 2 ** 49);
    }
    h[off + 6] = 1;
    if (vector) {
      const v = old as Vec3 | undefined;
      return v && v[0] === q[0] && v[1] === q[1] && v[2] === q[2] ? v : [q[0]!, q[1]!, q[2]!];
    }
    if (direction) {
      const d = old as Direction | undefined;
      return d && d.x === q[0] && d.y === q[1] ? d : { x: q[0]!, y: q[1]!, max: MAX_DIR };
    }
    return q[0]!;
  }

  delta(bytes: Uint8Array, start: number, end: number, dt: number, seq: number): Frame {
    const r = new RangeReader(bytes, this.models, start, end);
    if (r.bit(META)) {
      const c = new ByteCursor(blob(r, META + 1, 279));
      const n = bounded(c.uint(), 255);
      if (n > c.end - c.at) bad();
      this.game = text(c.bytes.subarray(c.at, c.at + n));
      c.at += n;
      const bsp = bounded(c.uint(), 2 ** 32), cut = c.uint();
      this.bsp = bsp === 0 ? undefined : bsp - 1;
      this.cut = cut === 0 ? undefined : cut - 1;
      if (c.end !== c.at + 1 || c.bytes[c.at]! > 1) bad();
      this.cinematic = c.bytes[c.at] === 1;
    }
    for (let j = 0; j < 8; j++) this.list(j, dt, r);
    const lists = this.rows.map(h => h.entities);
    const frame = materialize(this.game, bounded(this.frame.tick + dt, Number.MAX_SAFE_INTEGER),
      lists, lists[7]!, this.lists, this.frame);
    if (this.bsp !== undefined) frame.bsp = this.bsp;
    if (this.cut !== undefined) frame.cut = this.cut;
    if (this.cinematic) frame.cinematic = true;
    if (r.bit(META + 37)) {
      const n = 1 + bounded(r.uint(META + 96), 254);
      const updates = [];
      let next = 0;
      for (let j = 0; j < n; j++) {
        const i = bounded(next + r.uint(META + 128), 32767);
        updates.push({ i, p: r.literal() });
        next = i + 1;
      }
      frame.machine_positions = updates;
    }
    r.finish();
    this.lists = lists;
    this.frame = frame;
    this.seq = seq;
    return frame;
  }
}

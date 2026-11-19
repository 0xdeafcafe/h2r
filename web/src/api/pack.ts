// Packed telemetry frames: the decoder for the bit-packed format that
// crates/live-proto/src/pack.rs writes (docs/live/live-telemetry.md, "Packed
// frames"). The live stream (`/v1/stream?frames=packed`) and replays
// (`/v1/games/{id}/frames?format=packed`) send containers of these: each
// record a LEB128 length then the frame's bits, least significant first.
// The golden vectors in test/fixtures/pack, written by the Rust encoder,
// prove the two agree.

import type { Frame, FrameActor, FrameHit, FrameItem, FrameMachine, FrameObject, FramePlayer, FrameProjectile, FrameVehicle } from "./types";

/** Version 7 refines linear and angular precision, reuses edit masks and
 *  predicts unchanged motion. Version 6 carries the cinematic bit on every
 *  frame, adds vehicles' velocity and the projectile and item lists, and
 *  codes a moving entity's position in a delta as the error of a
 *  prediction from its velocity (`predict`). Version 5 was version 4 with
 *  the cinematic bit, written only for cinematic frames; version 4 added
 *  units' animation; version 3 units' velocity, fire ticks, vitality, seat
 *  and action ticks, hits' ticks and the frame's cut; versions 1 (vehicles
 *  without a parent and aim) to 5 still read. */
export const PACK_VERSION = 7;
/** Version 4's frames (ordinary frames before version 6). */
export const V4 = 4;
/** A cinematic frame's version before version 6: version 4 with the
 *  cinematic bit after the cut. */
export const CINEMATIC_VERSION = 5;
/** Version 6: the Exp-Golomb order of a predicted position's error, the
 *  longest gap predicted over, and the largest velocity component (1/64
 *  unit a second) predicted from. */
const PREDICT_K = 3;
const PREDICT_TICKS = 30;
const PREDICT_VEL = 2 ** 23;
const DIR_MAX_V4 = 4094;
const DIR_MAX = 16382;
const SPEED_SCALE = 16;
const AMOUNT_SCALE = 256;

const BODY_MAX = 2 ** 6 - 1;
const SHIELD_SCALE = 32;
const ANIM_SCALE = 16;
const DEVICE_BITS = 10;
const DEVICE_MAX = 2 ** DEVICE_BITS - 1;
const MAX_STRING = 255;
const MAX_STRINGS = 4096;
const MAX_NEW_STRINGS = 1024;
const MAX_ACTIONS = 8;
const TAU = 2 * Math.PI;

const STANCES = ["stand", "crouch", "airborne", "vehicle", "dead"];
const ACTIONS = ["grenade", "melee", "reload", "jump", "switch", "pickup"];
const GRENADES = ["frag", "plasma"];
/** Object kinds in code order (live-proto `ObjectKind::ALL`). */
export const OBJECT_KINDS = [
  "bipd", "vehi", "weap", "eqip", "scen", "mach", "ctrl", "lifi", "ssce", "bloc",
  "crea", "netgame",
] as const;
export type ObjectKind = (typeof OBJECT_KINDS)[number];

export class PackError extends Error {
  constructor(message: string, readonly gap = false) {
    super(message);
  }
}

export class BitReader {
  private pos = 0;
  private end: number;
  precise = false;
  span = 0;
  constructor(private readonly bytes: Uint8Array) { this.end = bytes.length * 8; }

  record(start: number, end: number) {
    this.pos = start * 8;
    this.end = end * 8;
  }

  bits(n: number): number {
    const pos = this.pos, b = this.bytes;
    if (pos + n > this.end) throw new PackError("packed frame ends early");
    // Up to 24 bits lie in the four bytes from the first one: one word, shifted.
    if (n <= 24) {
      const i = pos >> 3;
      const w = (b[i]! | ((b[i + 1] ?? 0) << 8) | ((b[i + 2] ?? 0) << 16) | ((b[i + 3] ?? 0) << 24)) >>> 0;
      this.pos = pos + n;
      return (w >>> (pos & 7)) & ((1 << n) - 1);
    }
    let v = 0;
    let got = 0;
    while (got < n) {
      const off = this.pos & 7;
      const take = Math.min(8 - off, n - got);
      const byte = (this.bytes[this.pos >> 3]! >> off) & ((1 << take) - 1);
      v += byte * 2 ** got;
      got += take;
      this.pos += take;
    }
    return v;
  }

  bit(): boolean {
    const pos = this.pos;
    if (pos >= this.end) throw new PackError("packed frame ends early");
    this.pos = pos + 1;
    return ((this.bytes[pos >> 3]! >> (pos & 7)) & 1) === 1;
  }

  /** Exp-Golomb of order `k`. */
  eg(k: number): number {
    let z = 0;
    while (!this.bit()) {
      z += 1;
      if (z > 40) throw new PackError("number code too long");
    }
    if (z + k > 52) throw new PackError("number too large");
    const width = z + k;
    const suffix = this.bits(width);
    return width < 30 ? (1 << width) - (1 << k) + suffix : 2 ** width - 2 ** k + suffix;
  }

  seg(k: number): number {
    const v = this.eg(k);
    return v % 2 === 0 ? v / 2 : -(v + 1) / 2;
  }
}

function sign(c: number): number {
  return c >= 0 ? 1 : -1;
}

export function unoct(qx: number, qy: number, max = DIR_MAX_V4): [number, number, number] {
  let x = qx / max * 2 - 1;
  let y = qy / max * 2 - 1;
  const z = 1 - Math.abs(x) - Math.abs(y);
  if (z < 0) {
    const ax = Math.abs(x), ay = Math.abs(y);
    x = (1 - ay) * sign(x);
    y = (1 - ax) * sign(y);
  }
  const len = Math.sqrt(x * x + y * y + z * z);
  return [x / len, y / len, z / len];
}

function dyaw(q: number): number {
  const a = q * TAU / 16384;
  return a > Math.PI ? a - TAU : a;
}

// Field values as the wire holds them.
type Id = { datum: number } | { player: number } | { text: string };
type Vec3 = [number, number, number];
type Direction = { x: number; y: number; max: number; unit?: Vec3 };
type Val = null | boolean | number | Vec3 | Direction | string | Id | readonly number[];

const enum K { Bool, Pos, Angle, Dir, Str, Id, Uint, Bits, Actions, Vel, Agos, AnimPhase }
interface Field { kind: K; arg?: number; opt?: boolean; key?: boolean }

const f = (kind: K, arg?: number): Field => ({ kind, arg });
const o = (kind: K, arg?: number): Field => ({ kind, arg, opt: true });
const key = (kind: K, arg?: number): Field => ({ kind, arg, key: true });

/** Version 3's unit fields, after version 2's 13: vel, fire, body, shield, seat, actions_ago. */
const UNIT_V3: Field[] = [o(K.Vel), o(K.Uint, 1), o(K.Bits, 6), o(K.Uint, 3), o(K.Uint, 0), o(K.Agos)];
/** Version 4's: the animation's name and frame. */
const UNIT_V4: Field[] = [o(K.Str), o(K.AnimPhase)];
const PLAYER_V2: Field[] = [key(K.Uint, 2), f(K.Bool), f(K.Pos), f(K.Angle), o(K.Id), o(K.Str), o(K.Dir), o(K.Str), o(K.Uint, 1), o(K.Bits, 3), o(K.Uint, 6), o(K.Actions), o(K.Bits, 1)];
const PLAYER_V3 = [...PLAYER_V2, ...UNIT_V3];
const PLAYER = [...PLAYER_V3, ...UNIT_V4];
/** Version 2 adds a vehicle's parent (a turret's vehicle) and aim; version 6 its velocity. */
const VEHICLE_V2: Field[] = [key(K.Id), f(K.Str), f(K.Pos), f(K.Dir), f(K.Dir), f(K.Bool), o(K.Id), o(K.Dir)];
const VEHICLE = [...VEHICLE_V2, o(K.Vel)];
const VEHICLE_V1 = VEHICLE_V2.slice(0, 6);
const ACTOR_V2: Field[] = [key(K.Id), f(K.Str), f(K.Pos), f(K.Angle), o(K.Dir), o(K.Str), o(K.Uint, 1), o(K.Str), o(K.Id), o(K.Bits, 3), o(K.Uint, 6), o(K.Actions), o(K.Bits, 1)];
const ACTOR_V3 = [...ACTOR_V2, ...UNIT_V3];
const ACTOR = [...ACTOR_V3, ...UNIT_V4];
const HIT_V2: Field[] = [o(K.Id), o(K.Id), o(K.Str), f(K.Pos), o(K.Uint, 6), o(K.Str)];
/** Version 3 adds the hit's ticks ago. */
const HIT = [...HIT_V2, o(K.Uint, 1)];
const MACHINE: Field[] = [o(K.Str), key(K.Id), f(K.Str), f(K.Pos), f(K.Dir), f(K.Dir), o(K.Bits, DEVICE_BITS)];
const OBJECT: Field[] = [key(K.Bits, 4), key(K.Uint, 6), f(K.Bool), o(K.Pos), o(K.Dir), o(K.Dir)];
/** Version 6: id, kind, pos, vel, parent. */
const PROJECTILE: Field[] = [key(K.Id), f(K.Str), f(K.Pos), o(K.Vel), o(K.Id)];
/** Version 6: id, kind, pos, forward, up, equipment. */
const ITEM: Field[] = [key(K.Id), f(K.Str), f(K.Pos), f(K.Dir), f(K.Dir), f(K.Bool)];

/** The delta-coded lists in wire order; hits go between actors and machines, always whole. */
const LISTS: [Field[], number][] = [[PLAYER, 16], [VEHICLE, 64], [ACTOR, 128], [MACHINE, 128], [OBJECT, 2048], [PROJECTILE, 64], [ITEM, 128]];
const LISTS_V4: [Field[], number][] = [[PLAYER, 16], [VEHICLE_V2, 64], [ACTOR, 128], [MACHINE, 128], [OBJECT, 2048]];
const LISTS_V3: [Field[], number][] = [[PLAYER_V3, 16], [VEHICLE_V2, 64], [ACTOR_V3, 128], [MACHINE, 128], [OBJECT, 2048]];
const LISTS_V2: [Field[], number][] = [[PLAYER_V2, 16], [VEHICLE_V2, 64], [ACTOR_V2, 128], [MACHINE, 128], [OBJECT, 2048]];
const LISTS_V1: [Field[], number][] = [[PLAYER_V2, 16], [VEHICLE_V1, 64], [ACTOR_V2, 128], [MACHINE, 128], [OBJECT, 2048]];
/** What a version this decoder reads has: its lists, its hits, whether the header carries a cut and the cinematic bit, and whether deltas predict positions (version 6). */
interface Schema { lists: [Field[], number][]; hit: Field[]; cut: boolean; cinematic: boolean; predict: boolean }
const SCHEMAS: Record<number, Schema> = {
  1: { lists: LISTS_V1, hit: HIT_V2, cut: false, cinematic: false, predict: false },
  2: { lists: LISTS_V2, hit: HIT_V2, cut: false, cinematic: false, predict: false },
  3: { lists: LISTS_V3, hit: HIT, cut: true, cinematic: false, predict: false },
  [V4]: { lists: LISTS_V4, hit: HIT, cut: true, cinematic: false, predict: false },
  [CINEMATIC_VERSION]: { lists: LISTS_V4, hit: HIT, cut: true, cinematic: true, predict: false },
  6: { lists: LISTS, hit: HIT, cut: true, cinematic: true, predict: true },
  [PACK_VERSION]: { lists: LISTS, hit: HIT, cut: true, cinematic: true, predict: true },
};


/** A mask has one bit per non-key field. Adjacent bits can be read in a
 *  single word even when their group starts after a key, as on machines. */
type DeltaLayout = { mask: { first: number; width: number }[]; motion: [number, number] | null };
const layouts = new Map<Field[], DeltaLayout>();
for (const schema of Object.values(SCHEMAS)) for (const [fields] of schema.lists) {
  if (layouts.has(fields)) continue;
  const mask: DeltaLayout["mask"] = [];
  for (let i = 0; i < fields.length;) {
    if (fields[i]!.key) { i++; continue; }
    const first = i++;
    while (i < fields.length && !fields[i]!.key) i++;
    mask.push({ first, width: i - first });
  }
  const pos = fields.findIndex(f => f.kind === K.Pos && !f.opt);
  const vel = fields.findIndex(f => f.kind === K.Vel);
  layouts.set(fields, { mask, motion: pos >= 0 && vel >= 0 ? [pos, vel] : null });
}

/** v6 predicts from the mean of both velocities, rounded half up on its
 *  1/512 grid. The original codec caps quantised velocity at 2^23. */
function legacyVelocity(v: Val | undefined): Vec3 | null {
  if (!Array.isArray(v)) return null;
  const limit = PREDICT_VEL / 64;
  return Math.abs(v[0]!) <= limit && Math.abs(v[1]!) <= limit && Math.abs(v[2]!) <= limit ? v as Vec3 : null;
}

function legacyPosition(p: Vec3, a: Vec3, newVel: Val | undefined, dt: number, residual: Vec3): Vec3 {
  const b = legacyVelocity(newVel) ?? a;
  const at = (i: number) => p[i]! + (Math.floor((256 * (a[i]! + b[i]!) * dt + 15) / 30) + residual[i]!) / 512;
  return [at(0), at(1), at(2)];
}

type Entity = Val[];

const UTF8 = new TextDecoder("utf-8", { fatal: true });

function readStrLiteral(r: BitReader): string {
  const len = r.eg(3);
  if (len > MAX_STRING) throw new PackError("string too long");
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i++) bytes[i] = r.bits(8);
  try {
    return UTF8.decode(bytes);
  } catch {
    throw new PackError("string is not UTF-8");
  }
}

/** Binary scales keep these sums exact. Immutable frames can share
 *  vectors directly whenever only a different field changes. */
function vector(r: BitReader, previous: Val | undefined, velocity: boolean): Vec3 {
  const scale = velocity ? (r.precise ? 128 : 64) : (r.precise ? 1024 : 512);
  if (!Array.isArray(previous)) {
    const k = velocity ? 8 : 16;
    return [r.seg(k) / scale, r.seg(k) / scale, r.seg(k) / scale];
  }
  const k = r.precise ? 1 : (velocity ? 4 : 7);
  const component = (base: number) => base + (r.precise && !r.bit() ? 0 : r.seg(k)) / scale;
  return [component(previous[0]!), component(previous[1]!), component(previous[2]!)];
}

function angularCode(r: BitReader, previous: number | undefined): number {
  if (previous === undefined || r.bit()) return r.bits(14);
  const delta = r.seg(0);
  if (delta < -64 || delta > 63) throw new PackError("angle delta out of range");
  return previous + delta;
}

function readValue(r: BitReader, field: Field, prev: Val | undefined, strings: readonly string[]): Val {
  const str = (i: number) => {
    const s = strings[i];
    if (s === undefined) throw new PackError("string index out of range");
    return s;
  };
  switch (field.kind) {
    case K.Bool: return r.bit();
    case K.Pos: return vector(r, prev, false);
    case K.Vel: return vector(r, prev, true);
    case K.Angle: return r.precise
      ? angularCode(r, typeof prev === "number" ? prev : undefined) & 0x3fff
      : r.bits(12) * 4;
    case K.Dir: {
      const before = prev as Direction | null | undefined;
      const x = r.precise ? angularCode(r, before?.x) : r.bits(12);
      const y = r.precise ? angularCode(r, before?.y) : r.bits(12);
      const max = r.precise ? DIR_MAX : DIR_MAX_V4;
      if (x < 0 || y < 0 || x > max || y > max) throw new PackError("direction out of range");
      return { x, y, max };
    }
    case K.Str: return str(r.eg(3));
    case K.Id: {
      switch (r.bits(2)) {
        case 0: return { datum: r.bits(32) };
        case 1: {
          const p = r.eg(2);
          if (p >= 2 ** 31) throw new PackError("player index out of range");
          return { player: p };
        }
        case 2: return { text: str(r.eg(3)) };
        default: throw new PackError("unknown id form");
      }
    }
    case K.AnimPhase:
      return r.precise && typeof prev === "number" && r.span >= 1 && r.span <= PREDICT_TICKS && r.bit()
        ? prev + ANIM_SCALE * r.span : r.eg(4);
    case K.Uint: return r.eg(field.arg!);
    case K.Bits: {
      const v = r.bits(field.arg!);
      if (field.arg === DEVICE_BITS && v > DEVICE_MAX) throw new PackError("device position out of range");
      return v;
    }
    case K.Actions: {
      const n = r.eg(0);
      if (n > MAX_ACTIONS) throw new PackError("too many actions");
      const out: number[] = [];
      for (let i = 0; i < n; i++) out.push(r.bits(3));
      return out;
    }
    case K.Agos: {
      const n = r.eg(0);
      if (n > MAX_ACTIONS) throw new PackError("too many action ticks");
      const out: number[] = [];
      for (let i = 0; i < n; i++) out.push(r.eg(1));
      return out;
    }
  }
}

function readField(r: BitReader, field: Field, prev: Val | undefined, strings: readonly string[]): Val {
  if (field.opt && !r.bit()) return null;
  return readValue(r, field, prev, strings);
}

function readEntity(r: BitReader, schema: Field[], strings: readonly string[]): Entity {
  const e: Entity = new Array(schema.length);
  for (let i = 0; i < schema.length; i++) e[i] = readField(r, schema[i]!, undefined, strings);
  return e;
}

function readCount(r: BitReader, max: number): number {
  const n = r.eg(2);
  if (n > max) throw new PackError("list too long");
  return n;
}

function readFull(r: BitReader, schema: Field[], max: number, strings: readonly string[]): Entity[] {
  const n = readCount(r, max);
  const out: Entity[] = [];
  for (let i = 0; i < n; i++) out.push(readEntity(r, schema, strings));
  return out;
}

/** Each 5-bit group contains four field toggles and one continuation bit.
 *  Expand the wire order to schema bits once for each unit type. */
function unitMask(order: number[]): Uint32Array {
  const table = new Uint32Array(5 * 16);
  for (let i = 0; i < order.length; i++) {
    for (let n = 0; n < 16; n++) {
      if (n & (1 << (i % 4))) table[(i >> 2) * 16 + n]! |= 1 << order[i]!;
    }
  }
  return table;
}

const PLAYER_MASK = unitMask([14, 16, 3, 8, 13, 2, 6, 11, 20, 9, 10, 7, 1, 4, 5, 12, 15, 17, 18, 19]);
const ACTOR_MASK = unitMask([14, 16, 3, 6, 13, 2, 4, 11, 20, 9, 10, 5, 1, 7, 8, 12, 15, 17, 18, 19]);

function readUnitMask(r: BitReader, table: Uint32Array, previous: number): number {
  for (let offset = 0; offset < table.length; offset += 16) {
    const group = r.bits(5);
    previous ^= table[offset + (group & 15)]!;
    if (group < 16) return previous;
  }
  throw new PackError("unit mask too long");
}

function sameUnit(a: Entity, b: Entity): boolean {
  const key = a[0], other = b[0];
  if (typeof key === "number") return key === other;
  return !!key && typeof key === "object" && "datum" in key &&
    !!other && typeof other === "object" && "datum" in other && key.datum === other.datum;
}

/** Immutable entity arrays double as history. A run can share the previous
 *  entity until the first field actually moves. Edits always own their copy. */
function advance(before: Entity, older: Entity | undefined, motion: [number, number],
                 aimField: number, dt: number, edit: boolean): Entity {
  let e = edit ? before.slice() : before;
  const [pos, vel] = motion;
  const v = before[vel] as Vec3 | null;
  let end = v;
  if (older && sameUnit(before, older)) {
    const yaw = before[3] as number;
    const nextYaw = (2 * yaw - (older[3] as number)) & 16383;
    if (nextYaw !== yaw) {
      if (e === before) e = before.slice();
      e[3] = nextYaw;
    }
    const a = before[aimField] as Direction | null, b = older[aimField] as Direction | null;
    if (a && b && (a.x !== b.x || a.y !== b.y)) {
      const x = Math.min(DIR_MAX, Math.max(0, 2 * a.x - b.x));
      const y = Math.min(DIR_MAX, Math.max(0, 2 * a.y - b.y));
      if (x !== a.x || y !== a.y) {
        if (e === before) e = before.slice();
        e[aimField] = { x, y, max: DIR_MAX };
      }
    }
    const old = older[vel] as Vec3 | null;
    if (v && old && (v[0] !== old[0] || v[1] !== old[1] || v[2] !== old[2])) {
      if (e === before) e = before.slice();
      e[vel] = end = [2 * v[0] - old[0], 2 * v[1] - old[1], 2 * v[2] - old[2]];
    }
  }
  if (v && end) {
    const step = (velocitySum: number) => {
      const q = velocitySum * 512 * dt / 30;
      return Math.sign(q) * Math.round(Math.abs(q)) / 1024;
    };
    const x = step(v[0] + end[0]), y = step(v[1] + end[1]), z = step(v[2] + end[2]);
    if (x || y || z) {
      if (e === before) e = before.slice();
      const p = before[pos] as Vec3;
      e[pos] = [p[0] + x, p[1] + y, p[2] + z];
    }
  }
  return e;
}

function readDelta(r: BitReader, schema: Field[], max: number, prev: Entity[], strings: readonly string[], dt: number, predicting: boolean, older?: Entity[]): Entity[] {
  const n = readCount(r, max);
  if (!n && !prev.length) return prev;
  const layout = layouts.get(schema)!;
  const motion = predicting && dt <= PREDICT_TICKS ? layout.motion : null;
  const advancing = r.precise && dt > 0 ? motion : null;
  const table = r.precise ? (schema === PLAYER ? PLAYER_MASK : schema === ACTOR ? ACTOR_MASK : null) : null;
  const aimField = schema === PLAYER ? 6 : 4;
  const out: Entity[] = [];
  let cursor = 0, mask = 0;
  while (out.length < n) {
    if (!r.bit()) {
      const run = r.eg(2) + 1;
      if (out.length + run > n || cursor + run > prev.length) throw new PackError("run past the list");
      if (!advancing && !out.length && run === n && n === prev.length) return prev;
      for (let i = 0; i < run; i++) {
        const e = prev[cursor + i]!;
        out.push(advancing ? advance(e, older?.[cursor + i], advancing, aimField, dt, false) : e);
      }
      cursor += run;
      continue;
    }
    if (r.bit()) {
      const j = cursor + r.seg(0);
      const before = prev[j];
      if (j < 0 || !before) throw new PackError("entity refers past the list");
      // The changed fields' mask (keys never change), then their values.
      if (table) mask = readUnitMask(r, table, mask);
      else if (!r.precise || r.bit()) {
        mask = 0;
        for (const group of layout.mask) mask |= r.bits(group.width) << group.first;
      }
      const e = advancing ? advance(before, older?.[j], advancing, aimField, dt, true) : before.slice();
      // A predicted position: its error now, the prediction once the new
      // velocity (a later field) is read.
      const base = !r.precise && motion ? legacyVelocity(before[motion[1]]) : null;
      let error: [number, number, number] | null = null;
      for (let fields = mask; fields; fields &= fields - 1) {
        const i = 31 - Math.clz32(fields & -fields);
        if (base && motion && i === motion[0]) error = [r.seg(PREDICT_K), r.seg(PREDICT_K), r.seg(PREDICT_K)];
        else e[i] = readField(r, schema[i]!, e[i], strings);
      }
      if (base && motion && error) {
        e[motion[0]] = legacyPosition(before[motion[0]] as Vec3, base, e[motion[1]], dt, error);
      }
      out.push(e);
      cursor = j + 1;
    } else {
      out.push(readEntity(r, schema, strings));
    }
  }
  return out;
}

// Entity values to frame objects, keys left out when absent as serde does.

const hex8 = (n: number) => n.toString(16).padStart(8, "0");
function idText(v: Val): string {
  const id = v as Id;
  if ("datum" in id) return hex8(id.datum);
  if ("player" in id) return "p" + id.player;
  return id.text;
}
const pos = (v: Val) => v as Vec3;
function dir(v: Val): Vec3 {
  const d = v as Direction;
  return d.unit ??= unoct(d.x, d.y, d.max);
}
function enumOf(table: readonly string[], v: number): string {
  const s = table[v];
  if (s === undefined) throw new PackError("enum value out of range");
  return s;
}
function actionsOf(v: Val): string[] {
  return (v as number[]).map((a) => enumOf(ACTIONS, a));
}

/** Version 3's and 4's unit fields (13 on); older versions have none. */
function unitV3(u: FramePlayer | FrameActor, e: Entity): void {
  if (e[13] != null) u.vel = e[13] as Vec3;
  if (e[14] != null) u.fire = e[14] as number;
  if (e[15] != null) u.body = (e[15] as number) / BODY_MAX;
  if (e[16] != null) u.shield = (e[16] as number) / SHIELD_SCALE;
  if (e[17] != null) u.seat = e[17] as number;
  if (e[18] != null) u.actions_ago = (e[18] as number[]).slice();
  // Version 4: both or neither.
  if ((e[19] == null) !== (e[20] == null)) throw new PackError("anim needs its name and frame");
  if (e[19] != null) u.anim = { name: e[19] as string, frame: (e[20] as number) / ANIM_SCALE };
}

function playerOf(e: Entity): FramePlayer {
  const p: FramePlayer = { index: e[0] as number, alive: e[1] as boolean, pos: pos(e[2]!), yaw: dyaw(e[3] as number) };
  if (e[4] !== null) p.vehicle = idText(e[4]!);
  if (e[5] !== null) p.species = e[5] as string;
  if (e[6] !== null) p.aim = dir(e[6]!);
  if (e[7] !== null) p.weapon = e[7] as string;
  if (e[8] !== null) p.shots = e[8] as number;
  if (e[9] !== null) p.stance = enumOf(STANCES, e[9] as number);
  if (e[10] !== null) p.speed = (e[10] as number) / SPEED_SCALE;
  if (e[11] !== null) p.actions = actionsOf(e[11]!);
  if (e[12] !== null) p.grenade = enumOf(GRENADES, e[12] as number);
  unitV3(p, e);
  return p;
}

function vehicleOf(e: Entity): FrameVehicle {
  const v: FrameVehicle = { id: idText(e[0]!), kind: e[1] as string, pos: pos(e[2]!), forward: dir(e[3]!), up: dir(e[4]!), occupied: e[5] as boolean };
  // Version 1 vehicles have neither.
  if (e[6] != null) v.parent = idText(e[6]);
  if (e[7] != null) v.aim = dir(e[7]);
  // Version 6.
  if (e[8] != null) v.vel = e[8] as Vec3;
  return v;
}

function projectileOf(e: Entity): FrameProjectile {
  const p: FrameProjectile = { id: idText(e[0]!), kind: e[1] as string, pos: pos(e[2]!) };
  if (e[3] !== null) p.vel = e[3] as Vec3;
  if (e[4] !== null) p.parent = idText(e[4]!);
  return p;
}

function itemOf(e: Entity): FrameItem {
  const x: FrameItem = { id: idText(e[0]!), kind: e[1] as string, pos: pos(e[2]!), forward: dir(e[3]!), up: dir(e[4]!) };
  if (e[5] === true) x.equipment = true;
  return x;
}

function actorOf(e: Entity): FrameActor {
  const a: FrameActor = { id: idText(e[0]!), species: e[1] as string, pos: pos(e[2]!), yaw: dyaw(e[3] as number) };
  if (e[4] !== null) a.aim = dir(e[4]!);
  if (e[5] !== null) a.weapon = e[5] as string;
  if (e[6] !== null) a.shots = e[6] as number;
  if (e[7] !== null) a.team = e[7] as string;
  if (e[8] !== null) a.vehicle = idText(e[8]!);
  if (e[9] !== null) a.stance = enumOf(STANCES, e[9] as number);
  if (e[10] !== null) a.speed = (e[10] as number) / SPEED_SCALE;
  if (e[11] !== null) a.actions = actionsOf(e[11]!);
  if (e[12] !== null) a.grenade = enumOf(GRENADES, e[12] as number);
  unitV3(a, e);
  return a;
}

function hitOf(e: Entity): FrameHit {
  const h: FrameHit = { pos: pos(e[3]!) } as FrameHit;
  if (e[0] !== null) h.from = idText(e[0]!);
  if (e[1] !== null) h.to = idText(e[1]!);
  if (e[2] !== null) h.weapon = e[2] as string;
  if (e[4] !== null) h.amount = (e[4] as number) / AMOUNT_SCALE;
  if (e[5] !== null) h.kind = e[5] as string;
  if (e[6] != null) h.ago = e[6] as number;
  return h;
}

function machineOf(e: Entity): FrameMachine {
  const m: FrameMachine = { id: idText(e[1]!), kind: e[2] as string, pos: pos(e[3]!), forward: dir(e[4]!), up: dir(e[5]!) };
  if (e[0] !== null) m.node = e[0] as string;
  if (e[6] !== null) m.position = (e[6] as number) / DEVICE_MAX;
  return m;
}

function objectOf(e: Entity): FrameObject {
  const k = OBJECT_KINDS[e[0] as number];
  if (!k) throw new PackError("unknown object kind");
  const x: FrameObject = { k, i: e[1] as number };
  if (e[3] !== null) x.p = pos(e[3]!);
  if (e[4] !== null) x.f = dir(e[4]!);
  if (e[5] !== null) x.u = dir(e[5]!);
  if (e[2] === true) x.gone = true;
  return x;
}

/** Each entity's frame object, kept on the entity: a delta keeps the
 *  entities that did not change, so their objects are made once (and go
 *  with the entity). Frames are not changed after decoding, so sharing
 *  them is safe. */
const MADE = Symbol("frame object");
type Made<T> = Entity & { [MADE]?: T };
function cached<T>(convert: (e: Entity) => T): (e: Entity) => T {
  return (e) => {
    const m = e as Made<T>;
    let v = m[MADE];
    if (v === undefined) {
      v = convert(e);
      m[MADE] = v;
    }
    return v;
  };
}
const playerFrom = cached(playerOf);
const vehicleFrom = cached(vehicleOf);
const actorFrom = cached(actorOf);
const machineFrom = cached(machineOf);
const objectFrom = cached(objectOf);
const projectileFrom = cached(projectileOf);
const itemFrom = cached(itemOf);

interface StreamState {
  seq: number;
  /** The format version its frames were in. */
  version: number;
  gameId: string;
  tick: number;
  bsp: number | null;
  strings: string[];
  lists: Entity[][];
  previousPlayers?: Entity[];
  previousActors?: Entity[];
  span?: number;
  frame?: Frame;
}

export interface Decoded { sid: number; seq: number; keyframe: boolean; frame: Frame }

/** Follows any number of packed streams (by `sid`). A delta without its
 *  base (a dropped frame, or joining mid-stream) throws a gap error and is
 *  skipped until the stream's next keyframe. */
export class FrameDecoder {
  private readonly streams = new Map<number, StreamState>();

  forget(sid: number): void {
    this.streams.delete(sid);
  }

  decode(bytes: Uint8Array): Decoded {
    return this.read(new BitReader(bytes));
  }

  /** Decode directly over a container, reusing the bit cursor for all
   *  records. Each read is bounded by its own length prefix. */
  forEachRecord(bytes: Uint8Array, visit: (frame: Decoded) => void, skipGaps = false): void {
    const r = new BitReader(bytes);
    walkRecords(bytes, (start, end) => {
      r.record(start, end);
      let frame: Decoded;
      try { frame = this.read(r); }
      catch (e) {
        if (skipGaps && e instanceof PackError && e.gap) return;
        throw e;
      }
      visit(frame);
    });
  }

  private read(r: BitReader): Decoded {
    const version = r.bits(4);
    r.precise = version === PACK_VERSION;
    const schema = SCHEMAS[version];
    if (!schema) throw new PackError(`packed frame version ${version} is not supported`);
    const keyframe = r.bit();
    const sid = r.eg(0);
    const seq = r.eg(8);
    let prev: StreamState;
    if (!keyframe) {
      const s = this.streams.get(sid);
      // A delta is against a frame of its own version.
      if (!s || s.seq + 1 !== seq || s.version !== version) throw new PackError(`delta frame ${seq} of stream ${sid} needs a keyframe first`, true);
      prev = s;
    } else {
      prev = { seq: 0, version, gameId: "", tick: 0, bsp: null, strings: [], lists: [] };
    }
    const state: StreamState = { seq, version, gameId: prev.gameId, tick: prev.tick, bsp: prev.bsp, strings: prev.strings, lists: [] };
    if (keyframe) {
      state.gameId = readStrLiteral(r);
      state.tick = r.eg(20);
      state.bsp = r.bit() ? r.eg(1) : null;
    } else {
      state.tick = prev.tick + r.eg(2);
      if (r.bit()) state.bsp = r.bit() ? r.eg(1) : null;
    }
    const cut = schema.cut && r.bit() ? r.eg(16) : null;
    const cinematic = schema.cinematic && r.bit();
    const fresh = r.eg(1);
    if (fresh > MAX_NEW_STRINGS || state.strings.length + fresh > MAX_STRINGS) throw new PackError("too many strings");
    // The table is shared with the frame before until it grows.
    if (fresh > 0) state.strings = state.strings.slice();
    for (let i = 0; i < fresh; i++) state.strings.push(readStrLiteral(r));
    let hits: Entity[] = [];
    const dt = state.tick - prev.tick;
    r.span = dt;
    if (r.precise && !keyframe) {
      state.span = dt;
      state.previousPlayers = prev.lists[0];
      state.previousActors = prev.lists[2];
    }
    for (let n = 0; n < schema.lists.length; n++) {
      const [fields, max] = schema.lists[n]!;
      if (n === 3) hits = readFull(r, schema.hit, 64, state.strings);
      const older = dt === prev.span ? (n === 0 ? prev.previousPlayers : n === 2 ? prev.previousActors : undefined) : undefined;
      state.lists[n] = keyframe ? readFull(r, fields, max, state.strings) : readDelta(r, fields, max, prev.lists[n] ?? [], state.strings, dt, schema.predict, older);
    }
    const prior = prev.frame;
    const lists = state.lists;
    const frame: Frame = { game_id: state.gameId, tick: state.tick, players:
      prior && lists[0] === prev.lists[0] ? prior.players : lists[0]!.map(playerFrom) };
    if (lists[1]?.length) frame.vehicles = lists[1] === prev.lists[1] ? prior!.vehicles : lists[1].map(vehicleFrom);
    if (lists[2]?.length) frame.actors = lists[2] === prev.lists[2] ? prior!.actors : lists[2].map(actorFrom);
    if (lists[3]?.length) frame.machines = lists[3] === prev.lists[3] ? prior!.machines : lists[3].map(machineFrom);
    if (lists[4]?.length) frame.objects = lists[4] === prev.lists[4] ? prior!.objects : lists[4].map(objectFrom);
    if (lists[5]?.length) frame.projectiles = lists[5] === prev.lists[5] ? prior!.projectiles : lists[5].map(projectileFrom);
    if (lists[6]?.length) frame.items = lists[6] === prev.lists[6] ? prior!.items : lists[6].map(itemFrom);
    if (hits.length) frame.hits = hits.map(hitOf);
    state.frame = frame;
    if (state.bsp !== null) frame.bsp = state.bsp;
    if (cut !== null) frame.cut = cut;
    if (cinematic) frame.cinematic = true;
    this.streams.set(sid, state);
    return { sid, seq, keyframe, frame };
  }
}

/** The records of a container (LEB128 length, then the bytes). */
function walkRecords(bytes: Uint8Array, visit: (start: number, end: number) => void): void {
  let at = 0;
  while (at < bytes.length) {
    let n = 0, shift = 0;
    for (;;) {
      if (at >= bytes.length) throw new PackError("packed frame ends early");
      const b = bytes[at++]!;
      n += (b & 0x7f) * 2 ** shift;
      if ((b & 0x80) === 0) break;
      shift += 7;
      if (shift > 28) throw new PackError("record length too long");
    }
    if (at + n > bytes.length) throw new PackError("packed frame ends early");
    visit(at, at + n);
    at += n;
  }
}

export function records(bytes: Uint8Array): Uint8Array[] {
  const out: Uint8Array[] = [];
  walkRecords(bytes, (start, end) => out.push(bytes.subarray(start, end)));
  return out;
}

/** Decodes a container of frames packed on their own (recorded frames). */
export function decodeKeyframes(bytes: Uint8Array): Frame[] {
  const out: Frame[] = [];
  new FrameDecoder().forEachRecord(bytes, (d) => {
    if (!d.keyframe) throw new PackError("not a keyframe");
    out.push(d.frame);
  });
  return out;
}

/** Base64 (standard or URL alphabet, padded or not) to bytes. */
export function fromBase64(s: string): Uint8Array {
  const std = s.replace(/-/g, "+").replace(/_/g, "/");
  const padded = std + "=".repeat((4 - (std.length % 4)) % 4);
  const bin = atob(padded);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** One frame packed on its own (a recorded frame's `data.packed`). */
export function decodeKeyframe(bytes: Uint8Array): Frame {
  const d = new FrameDecoder().decode(bytes);
  if (!d.keyframe) throw new PackError("not a keyframe");
  return d.frame;
}

/** A recorded `GameFrame` event's frame: packed (`data.packed`) or, from
 *  older servers, the JSON frame itself. Null when it does not decode. */
export function recordedFrame(data: unknown): Frame | null {
  const d = data as { packed?: unknown; tick?: unknown } | null;
  if (!d || typeof d !== "object") return null;
  if (typeof d.packed === "string") {
    try {
      return decodeKeyframe(fromBase64(d.packed));
    } catch {
      return null;
    }
  }
  return typeof d.tick === "number" && Array.isArray((d as { players?: unknown }).players) ? (d as unknown as Frame) : null;
}

// Packed telemetry frames: the decoder for the bit-packed format that
// crates/live-proto/src/pack.rs writes (docs/live/live-telemetry.md, "Packed
// frames"). The live stream (`/v1/stream?frames=packed`) and replays
// (`/v1/games/{id}/frames?format=packed`) send containers of these: each
// record a LEB128 length then the frame's bits, least significant first.
// The golden vectors in test/fixtures/pack, written by the Rust encoder,
// prove the two agree.

import type { Frame, FrameActor, FrameHit, FrameItem, FrameMachine, FrameObject, FramePlayer, FrameProjectile, FrameVehicle } from "./types";

/** The version written now. Version 6 carries the cinematic bit on every
 *  frame, adds vehicles' velocity and the projectile and item lists, and
 *  codes a moving entity's position in a delta as the error of a
 *  prediction from its velocity (`predict`). Version 5 was version 4 with
 *  the cinematic bit, written only for cinematic frames; version 4 added
 *  units' animation; version 3 units' velocity, fire ticks, vitality, seat
 *  and action ticks, hits' ticks and the frame's cut; versions 1 (vehicles
 *  without a parent and aim) to 5 still read. */
export const PACK_VERSION = 6;
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
const POS_SCALE = 512;
const ANGLE_BITS = 12;
const DIR_BITS = 12;
const DIR_MAX = 2 ** DIR_BITS - 2;
const SPEED_SCALE = 16;
const AMOUNT_SCALE = 256;
const VEL_SCALE = 64;
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
  constructor(private readonly bytes: Uint8Array) {}

  bits(n: number): number {
    const pos = this.pos, b = this.bytes;
    if (pos + n > b.length * 8) throw new PackError("packed frame ends early");
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
    return this.bits(1) === 1;
  }

  /** Exp-Golomb of order `k`. */
  eg(k: number): number {
    let z = 0;
    while (!this.bit()) {
      z += 1;
      if (z > 40) throw new PackError("number code too long");
    }
    if (z + k > 52) throw new PackError("number too large");
    return 2 ** (z + k) + this.bits(z + k) - 2 ** k;
  }

  seg(k: number): number {
    const v = this.eg(k);
    return v % 2 === 0 ? v / 2 : -(v + 1) / 2;
  }
}

function sign(c: number): number {
  return c >= 0 ? 1 : -1;
}

export function unoct(qx: number, qy: number): [number, number, number] {
  let x = qx / DIR_MAX * 2 - 1;
  let y = qy / DIR_MAX * 2 - 1;
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
  const a = q * TAU / 2 ** ANGLE_BITS;
  return a > Math.PI ? a - TAU : a;
}

// Field values as the wire holds them.
type Id = { datum: number } | { player: number } | { text: string };
type Val = null | boolean | number | readonly [number, number, number] | { dir: [number, number] } | { vel: readonly [number, number, number] } | string | Id | readonly number[];

const enum K { Bool, Pos, Angle, Dir, Str, Id, Uint, Bits, Actions, Vel, Agos }
interface Field { kind: K; arg?: number; opt?: boolean; key?: boolean }

const f = (kind: K, arg?: number): Field => ({ kind, arg });
const o = (kind: K, arg?: number): Field => ({ kind, arg, opt: true });
const key = (kind: K, arg?: number): Field => ({ kind, arg, key: true });

/** Version 3's unit fields, after version 2's 13: vel, fire, body, shield, seat, actions_ago. */
const UNIT_V3: Field[] = [o(K.Vel), o(K.Uint, 1), o(K.Bits, 6), o(K.Uint, 3), o(K.Uint, 0), o(K.Agos)];
/** Version 4's: the animation's name and frame. */
const UNIT_V4: Field[] = [o(K.Str), o(K.Uint, 4)];
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
  [PACK_VERSION]: { lists: LISTS, hit: HIT, cut: true, cinematic: true, predict: true },
};
const LIST_COUNT = LISTS.length;

/** Where an entity's (non-optional) position and its velocity sit, when it has both: what version 6 predicts from. */
function motionFields(schema: Field[]): [number, number] | null {
  const pos = schema.findIndex((x) => x.kind === K.Pos && !x.opt);
  const vel = schema.findIndex((x) => x.kind === K.Vel);
  return pos >= 0 && vel >= 0 ? [pos, vel] : null;
}
const MOTION = new Map<Field[], [number, number] | null>();
function motionOf(schema: Field[]): [number, number] | null {
  let m = MOTION.get(schema);
  if (m === undefined) {
    m = motionFields(schema);
    MOTION.set(schema, m);
  }
  return m;
}

/** The previous velocity a version 6 delta predicts from, `dt` ticks on: only one in range, over at most PREDICT_TICKS. */
function predictBase(prevVel: Val | undefined, dt: number): readonly [number, number, number] | null {
  if (dt > PREDICT_TICKS || prevVel === null || typeof prevVel !== "object" || !("vel" in prevVel)) return null;
  const a = prevVel.vel;
  return Math.abs(a[0]) <= PREDICT_VEL && Math.abs(a[1]) <= PREDICT_VEL && Math.abs(a[2]) <= PREDICT_VEL ? a : null;
}

/** Version 6's predicted position (live-proto `pack::predict`): the
 *  previous one moved by the mean of the previous and new velocity (the
 *  previous alone without a new one in range) over `dt` ticks, in 1/512
 *  unit, rounded half up: p + floor((4 (a + b) dt + 15) / 30). */
function predict(p: readonly number[], a: readonly [number, number, number], newVel: Val | undefined, dt: number): [number, number, number] {
  let b = a;
  if (newVel !== null && typeof newVel === "object" && "vel" in newVel) {
    const v = newVel.vel;
    if (Math.abs(v[0]) <= PREDICT_VEL && Math.abs(v[1]) <= PREDICT_VEL && Math.abs(v[2]) <= PREDICT_VEL) b = v;
  }
  const at = (i: number) => p[i]! + Math.floor((4 * (a[i]! + b[i]!) * dt + 15) / 30);
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

function readValue(r: BitReader, field: Field, prev: Val | undefined, strings: readonly string[]): Val {
  const str = (i: number) => {
    const s = strings[i];
    if (s === undefined) throw new PackError("string index out of range");
    return s;
  };
  switch (field.kind) {
    case K.Bool: return r.bit();
    case K.Pos: {
      if (Array.isArray(prev) && prev.length === 3) {
        const p = prev as unknown as readonly [number, number, number];
        return [p[0] + r.seg(7), p[1] + r.seg(7), p[2] + r.seg(7)] as const;
      }
      return [r.seg(16), r.seg(16), r.seg(16)] as const;
    }
    case K.Angle: return r.bits(ANGLE_BITS);
    case K.Dir: {
      const x = r.bits(DIR_BITS), y = r.bits(DIR_BITS);
      if (x > DIR_MAX || y > DIR_MAX) throw new PackError("direction out of range");
      return { dir: [x, y] };
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
    case K.Vel: {
      // Differences against the value before in a delta.
      if (prev !== null && typeof prev === "object" && "vel" in prev) {
        const p = prev.vel;
        return { vel: [p[0] + r.seg(4), p[1] + r.seg(4), p[2] + r.seg(4)] as const };
      }
      return { vel: [r.seg(8), r.seg(8), r.seg(8)] as const };
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

function readDelta(r: BitReader, schema: Field[], max: number, prev: readonly Entity[], strings: readonly string[], dt: number, predicting: boolean): Entity[] {
  const n = readCount(r, max);
  const motion = predicting ? motionOf(schema) : null;
  const out: Entity[] = [];
  let cursor = 0;
  while (out.length < n) {
    if (!r.bit()) {
      const run = r.eg(2) + 1;
      if (out.length + run > n || cursor + run > prev.length) throw new PackError("run past the list");
      for (let i = 0; i < run; i++) out.push(prev[cursor + i]!);
      cursor += run;
      continue;
    }
    if (r.bit()) {
      const j = cursor + r.seg(0);
      const before = prev[j];
      if (j < 0 || !before) throw new PackError("entity refers past the list");
      // The changed fields' mask (keys never change), then their values.
      let mask = 0;
      for (let i = 0; i < schema.length; i++) if (!schema[i]!.key && r.bit()) mask |= 1 << i;
      const e = before.slice();
      // A predicted position: its error now, the prediction once the new
      // velocity (a later field) is read.
      const base = motion ? predictBase(before[motion[1]], dt) : null;
      let error: [number, number, number] | null = null;
      for (let i = 0; i < schema.length; i++) {
        if (!(mask & (1 << i))) continue;
        if (base && motion && i === motion[0]) error = [r.seg(PREDICT_K), r.seg(PREDICT_K), r.seg(PREDICT_K)];
        else e[i] = readField(r, schema[i]!, before[i], strings);
      }
      if (base && motion && error) {
        const guess = predict(before[motion[0]] as unknown as readonly number[], base, e[motion[1]], dt);
        e[motion[0]] = [guess[0] + error[0], guess[1] + error[1], guess[2] + error[2]] as const;
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
const pos = (v: Val) => (v as readonly number[]).map((c) => c / POS_SCALE) as [number, number, number];
const dir = (v: Val) => { const d = (v as { dir: [number, number] }).dir; return unoct(d[0], d[1]); };
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
  if (e[13] != null) u.vel = (e[13] as { vel: readonly number[] }).vel.map((c) => c / VEL_SCALE) as [number, number, number];
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
  if (e[8] != null) v.vel = velOf(e[8]);
  return v;
}

const velOf = (v: Val) => (v as { vel: readonly number[] }).vel.map((c) => c / VEL_SCALE) as [number, number, number];

function projectileOf(e: Entity): FrameProjectile {
  const p: FrameProjectile = { id: idText(e[0]!), kind: e[1] as string, pos: pos(e[2]!) };
  if (e[3] !== null) p.vel = velOf(e[3]!);
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
    const r = new BitReader(bytes);
    const version = r.bits(4);
    const schema = SCHEMAS[version];
    if (!schema) throw new PackError(`packed frame version ${version} is not supported`);
    const keyframe = r.bit();
    const sid = r.eg(0);
    const seq = r.eg(8);
    const empty: StreamState = { seq: 0, version, gameId: "", tick: 0, bsp: null, strings: [], lists: Array.from({ length: LIST_COUNT }, () => []) };
    let prev = empty;
    if (!keyframe) {
      const s = this.streams.get(sid);
      // A delta is against a frame of its own version.
      if (!s || s.seq + 1 !== seq || s.version !== version) throw new PackError(`delta frame ${seq} of stream ${sid} needs a keyframe first`, true);
      prev = s;
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
    schema.lists.forEach(([fields, max], n) => {
      if (n === 3) hits = readFull(r, schema.hit, 64, state.strings);
      state.lists[n] = keyframe ? readFull(r, fields, max, state.strings) : readDelta(r, fields, max, prev.lists[n] ?? [], state.strings, dt, schema.predict);
    });
    for (let n = schema.lists.length; n < LIST_COUNT; n++) state.lists[n] = [];
    const frame: Frame = { game_id: state.gameId, tick: state.tick, players: state.lists[0]!.map(playerFrom) };
    const vehicles = state.lists[1]!.map(vehicleFrom);
    const actors = state.lists[2]!.map(actorFrom);
    const hitList = hits.map(hitOf);
    const machines = state.lists[3]!.map(machineFrom);
    const objects = state.lists[4]!.map(objectFrom);
    const projectiles = state.lists[5]!.map(projectileFrom);
    const items = state.lists[6]!.map(itemFrom);
    if (vehicles.length) frame.vehicles = vehicles;
    if (actors.length) frame.actors = actors;
    if (hitList.length) frame.hits = hitList;
    if (state.bsp !== null) frame.bsp = state.bsp;
    if (machines.length) frame.machines = machines;
    if (objects.length) frame.objects = objects;
    if (cut !== null) frame.cut = cut;
    if (cinematic) frame.cinematic = true;
    if (projectiles.length) frame.projectiles = projectiles;
    if (items.length) frame.items = items;
    this.streams.set(sid, state);
    return { sid, seq, keyframe, frame };
  }
}

/** The records of a container (LEB128 length, then the bytes). */
export function records(bytes: Uint8Array): Uint8Array[] {
  const out: Uint8Array[] = [];
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
    out.push(bytes.subarray(at, at + n));
    at += n;
  }
  return out;
}

/** Decodes a container of frames packed on their own (recorded frames). */
export function decodeKeyframes(bytes: Uint8Array): Frame[] {
  return records(bytes).map(decodeKeyframe);
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

// Packed telemetry frames: the decoder for the bit-packed format that
// crates/live-proto/src/pack.rs writes (docs/live/live-telemetry.md, "Packed
// frames"). The live stream (`/v1/stream?frames=packed`) and replays
// (`/v1/games/{id}/frames?format=packed`) send containers of these: each
// record a LEB128 length then the frame's bits, least significant first.
// The golden vectors in test/fixtures/pack, written by the Rust encoder,
// prove the two agree.

import type { Frame, FrameActor, FrameHit, FrameMachine, FrameObject, FramePlayer, FrameVehicle } from "./types";

/** The version written now; version 1 (vehicles without a parent and aim) still reads. */
export const PACK_VERSION = 2;
const POS_SCALE = 512;
const ANGLE_BITS = 12;
const DIR_BITS = 12;
const DIR_MAX = 2 ** DIR_BITS - 2;
const SPEED_SCALE = 16;
const AMOUNT_SCALE = 256;
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
    if (this.pos + n > this.bytes.length * 8) throw new PackError("packed frame ends early");
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
type Val = null | boolean | number | readonly [number, number, number] | { dir: [number, number] } | string | Id | readonly number[];

const enum K { Bool, Pos, Angle, Dir, Str, Id, Uint, Bits, Actions }
interface Field { kind: K; arg?: number; opt?: boolean; key?: boolean }

const f = (kind: K, arg?: number): Field => ({ kind, arg });
const o = (kind: K, arg?: number): Field => ({ kind, arg, opt: true });
const key = (kind: K, arg?: number): Field => ({ kind, arg, key: true });

const PLAYER: Field[] = [key(K.Uint, 2), f(K.Bool), f(K.Pos), f(K.Angle), o(K.Id), o(K.Str), o(K.Dir), o(K.Str), o(K.Uint, 1), o(K.Bits, 3), o(K.Uint, 6), o(K.Actions), o(K.Bits, 1)];
/** Version 2 adds a vehicle's parent (a turret's vehicle) and aim. */
const VEHICLE: Field[] = [key(K.Id), f(K.Str), f(K.Pos), f(K.Dir), f(K.Dir), f(K.Bool), o(K.Id), o(K.Dir)];
const VEHICLE_V1 = VEHICLE.slice(0, 6);
const ACTOR: Field[] = [key(K.Id), f(K.Str), f(K.Pos), f(K.Angle), o(K.Dir), o(K.Str), o(K.Uint, 1), o(K.Str), o(K.Id), o(K.Bits, 3), o(K.Uint, 6), o(K.Actions), o(K.Bits, 1)];
const HIT: Field[] = [o(K.Id), o(K.Id), o(K.Str), f(K.Pos), o(K.Uint, 6), o(K.Str)];
const MACHINE: Field[] = [o(K.Str), key(K.Id), f(K.Str), f(K.Pos), f(K.Dir), f(K.Dir), o(K.Bits, DEVICE_BITS)];
const OBJECT: Field[] = [key(K.Bits, 4), key(K.Uint, 6), f(K.Bool), o(K.Pos), o(K.Dir), o(K.Dir)];

/** The delta-coded lists in wire order; hits go between actors and machines, always whole. */
const LISTS: [Field[], number][] = [[PLAYER, 16], [VEHICLE, 64], [ACTOR, 128], [MACHINE, 128], [OBJECT, 2048]];
const LISTS_V1: [Field[], number][] = [[PLAYER, 16], [VEHICLE_V1, 64], [ACTOR, 128], [MACHINE, 128], [OBJECT, 2048]];
const listsOf = (version: number) => (version === PACK_VERSION ? LISTS : version === 1 ? LISTS_V1 : null);

type Entity = Val[];

function readStrLiteral(r: BitReader): string {
  const len = r.eg(3);
  if (len > MAX_STRING) throw new PackError("string too long");
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i++) bytes[i] = r.bits(8);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
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
  }
}

function readField(r: BitReader, field: Field, prev: Val | undefined, strings: readonly string[]): Val {
  if (field.opt && !r.bit()) return null;
  return readValue(r, field, prev, strings);
}

function readEntity(r: BitReader, schema: Field[], strings: readonly string[]): Entity {
  return schema.map((field) => readField(r, field, undefined, strings));
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

function readDelta(r: BitReader, schema: Field[], max: number, prev: readonly Entity[], strings: readonly string[]): Entity[] {
  const n = readCount(r, max);
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
      const mask = schema.map((field) => (field.key ? false : r.bit()));
      const e = before.slice();
      schema.forEach((field, i) => {
        if (mask[i]) e[i] = readField(r, field, before[i], strings);
      });
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
  return p;
}

function vehicleOf(e: Entity): FrameVehicle {
  const v: FrameVehicle = { id: idText(e[0]!), kind: e[1] as string, pos: pos(e[2]!), forward: dir(e[3]!), up: dir(e[4]!), occupied: e[5] as boolean };
  // Version 1 vehicles have neither.
  if (e[6] != null) v.parent = idText(e[6]);
  if (e[7] != null) v.aim = dir(e[7]);
  return v;
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
  return a;
}

function hitOf(e: Entity): FrameHit {
  const h: FrameHit = { pos: pos(e[3]!) } as FrameHit;
  if (e[0] !== null) h.from = idText(e[0]!);
  if (e[1] !== null) h.to = idText(e[1]!);
  if (e[2] !== null) h.weapon = e[2] as string;
  if (e[4] !== null) h.amount = (e[4] as number) / AMOUNT_SCALE;
  if (e[5] !== null) h.kind = e[5] as string;
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
    const lists = listsOf(version);
    if (!lists) throw new PackError(`packed frame version ${version} is not supported`);
    const keyframe = r.bit();
    const sid = r.eg(0);
    const seq = r.eg(8);
    const empty: StreamState = { seq: 0, version, gameId: "", tick: 0, bsp: null, strings: [], lists: [[], [], [], [], []] };
    let prev = empty;
    if (!keyframe) {
      const s = this.streams.get(sid);
      // A delta is against a frame of its own version.
      if (!s || s.seq + 1 !== seq || s.version !== version) throw new PackError(`delta frame ${seq} of stream ${sid} needs a keyframe first`, true);
      prev = s;
    }
    const state: StreamState = { seq, version, gameId: prev.gameId, tick: prev.tick, bsp: prev.bsp, strings: prev.strings.slice(), lists: [] };
    if (keyframe) {
      state.gameId = readStrLiteral(r);
      state.tick = r.eg(20);
      state.bsp = r.bit() ? r.eg(1) : null;
    } else {
      state.tick = prev.tick + r.eg(2);
      if (r.bit()) state.bsp = r.bit() ? r.eg(1) : null;
    }
    const fresh = r.eg(1);
    if (fresh > MAX_NEW_STRINGS || state.strings.length + fresh > MAX_STRINGS) throw new PackError("too many strings");
    for (let i = 0; i < fresh; i++) state.strings.push(readStrLiteral(r));
    let hits: Entity[] = [];
    lists.forEach(([schema, max], n) => {
      if (n === 3) hits = readFull(r, HIT, 64, state.strings);
      state.lists[n] = keyframe ? readFull(r, schema, max, state.strings) : readDelta(r, schema, max, prev.lists[n]!, state.strings);
    });
    const frame: Frame = { game_id: state.gameId, tick: state.tick, players: state.lists[0]!.map(playerOf) };
    const vehicles = state.lists[1]!.map(vehicleOf);
    const actors = state.lists[2]!.map(actorOf);
    const hitList = hits.map(hitOf);
    const machines = state.lists[3]!.map(machineOf);
    const objects = state.lists[4]!.map(objectOf);
    if (vehicles.length) frame.vehicles = vehicles;
    if (actors.length) frame.actors = actors;
    if (hitList.length) frame.hits = hitList;
    if (state.bsp !== null) frame.bsp = state.bsp;
    if (machines.length) frame.machines = machines;
    if (objects.length) frame.objects = objects;
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

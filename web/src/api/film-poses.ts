// Pose predictors reset at the indexed scene baseline and span later time chunks.
// Decode full salted datums as numbers: the datum-delta opcode exceeds u32.
import { FrameDecoder } from "./pack";
import { ByteCursor } from "./pack-v8";
import type { FilmProp, FilmVector, FilmWorld, Frame, FrameActor, FrameObject, FramePlayer } from "./types";

const utf8 = new TextDecoder("utf-8", { fatal: true });
const placed = new WeakMap<ObjectState, FrameObject>();
const loose = new WeakMap<ObjectState, FilmProp>();
const classes = ["bipd", "vehi", "weap", "eqip", "garb", "proj", "scen", "mach", "ctrl", "lifi", "ssce", "bloc", "crea"];
const angle = 2 * Math.PI / 65536;
const units = 304.8;
const id = (datum: number) => datum.toString(16).padStart(8, "0");
const defaults = Array.from({ length: 24 }, (_, i) => i === 11 ? 16384 : 0);
const invalid = (): never => { throw new Error("invalid film pose"); };
function byte(r: ByteCursor) { return r.at < r.end ? r.bytes[r.at++]! : invalid(); }
function bytes(r: ByteCursor, n: number) {
  if (n > r.end - r.at) invalid();
  const data = r.bytes.subarray(r.at, r.at + n);
  r.at += n;
  return data;
}
function signed(r: ByteCursor): number {
  const n = r.uint();
  return n % 2 ? -(n + 1) / 2 : n / 2;
}
function between(n: number, lo: number, hi: number) {
  if (!Number.isSafeInteger(n) || n < lo || n > hi) invalid();
  return n;
}
function validate(q: number[]) {
  between(q[0]!, 0, 12);
  for (const i of [1, 4, 16, 22]) between(q[i]!, 0, 2 ** 32);
  for (const i of [2, 12]) between(q[i]!, 0, 65536);
  for (const i of [3, 20]) between(q[i]!, 0, 256);
  for (const i of [5, 6, 7, 19]) between(q[i]!, -2147483648, 2147483647);
  for (const i of [9, 11, 15]) between(q[i]!, -16384, 16384);
  for (const i of [17, 18]) between(q[i]!, -32768, 32767);
  for (const i of [13, 21]) between(q[i]!, 0, 1);
  if ((!q[13] && (q[14] || q[15])) || (!q[16] && (q[17] || q[18] || q[19])) || ![0, 1, 3, 9].includes(q[23]!)) invalid();
}
function direction(yaw: number, pitch: number): FilmVector {
  const h = Math.cos(pitch * angle);
  return [h * Math.cos(yaw * angle), h * Math.sin(yaw * angle), Math.sin(pitch * angle)];
}
interface Transform { pos: FilmVector; forward: FilmVector; up: FilmVector }
interface ObjectState extends Transform {
  q: number[]; name: string; datum: number; tag: string; kind: string; parent: number | null; aim: FilmVector | null;
}
function object(datum: number, q: number[], name: string): ObjectState {
  const tag = classes[q[0]!] ?? "unknown";
  const kind = name || ({ 1: "flag", 3: "ball", 9: "assault_bomb" } as Record<number, string>)[q[23]!] || "unknown";
  return { datum, q, name, kind, tag, parent: q[4] ? q[4] - 1 : null,
    pos: [q[5]! / units, q[6]! / units, q[7]! / units], forward: direction(q[8]!, q[9]!),
    up: direction(q[10]!, q[11]!), aim: q[13] ? direction(q[14]!, q[15]!) : null };
}

function transformed(child: Transform, parent: Transform): Transform {
  const f = parent.forward, u = parent.up;
  const left = [u[1] * f[2] - u[2] * f[1], u[2] * f[0] - u[0] * f[2], u[0] * f[1] - u[1] * f[0]];
  const rotate = (v: FilmVector): FilmVector => [0, 1, 2].map(i =>
    f[i]! * v[0] + left[i]! * v[1] + u[i]! * v[2]) as FilmVector;
  const p = rotate(child.pos);
  return { pos: [p[0] + parent.pos[0], p[1] + parent.pos[1], p[2] + parent.pos[2]],
    forward: rotate(child.forward), up: rotate(child.up) };
}

export class FilmPoses {
  private objects = new Map<number, ObjectState>();
  private hill: FilmWorld["hill"] = null;
  private decoder = new FrameDecoder();
  private extras: Frame | undefined;
  private started = false;
  private extrasTick: number | undefined;

  constructor(private readonly game: string) {}

  apply(tick: number, payload: Uint8Array) {
    if (payload.length > (8 << 20)) invalid();
    const r = new ByteCursor(payload), op = byte(r);
    if (op > 2) return;
    if (op !== 0 && !this.started) invalid();
    if (op === 2) {
      this.extras = this.decodeExtra(this.decoder, bytes(r, r.end - r.at), false);
      this.extrasTick = tick;
      return;
    }
    // Apply only after every lane, footer and optional packed key has
    // decoded. A truncated last edit must leave the previous scene intact.
    let total = op === 0 ? 0 : this.objects.size;
    let nextHill = op === 0 ? null : this.hill;
    const changes: [number, ObjectState | null][] = [];
    const count = r.uint();
    if (count > (op === 0 ? 2048 : 4096)) invalid();
    let datum = 0;
    for (let i = 0; i < count; i++) {
      const key = r.uint(), action = key % 4;
      const diff = Math.floor(key / 4);
      datum += diff;
      if ((i && !diff) || datum > 0xffffffff) invalid();
      const prev = op === 0 ? undefined : this.objects.get(datum);
      if (action > 2 || (action === 0) === !!prev || (op === 0 && action !== 0)) invalid();
      if (action === 2) { changes.push([datum, null]); total--; continue; }
      if (!prev) total++;
      const mask = r.uint();
      if (mask >= 2 ** 27 || mask & (3 << 24)) invalid();
      const q = (prev?.q ?? defaults).slice();
      for (let j = 0; j < 24; j++) {
        if (!(mask & (1 << j))) continue;
        const diff = signed(r);
        if (j === 8 || j === 10 || j === 14) q[j] = (q[j]! + between(diff, -32768, 32767)) & 65535;
        else q[j] = q[j]! + diff;
        if (!Number.isSafeInteger(q[j])) invalid();
      }
      validate(q);
      let name = prev?.name ?? "";
      if (mask & (1 << 26)) name = utf8.decode(bytes(r, between(r.uint(), 0, 256)));
      changes.push([datum, object(datum, q, name)]);
    }
    if (total > 2048) invalid();
    const hill = byte(r);
    if (hill === 1) nextHill = null;
    else if (hill === 2) {
      const index = byte(r), n = r.uint();
      if (n > 256) invalid();
      const corners: FilmVector[] = [];
      const coord = () => between(signed(r), -2147483648, 2147483647) / units;
      for (let i = 0; i < n; i++) corners.push([coord(), coord(), coord()]);
      nextHill = { index, corners };
    } else if (hill !== 0) invalid();
    const packed = op === 0 ? bytes(r, r.uint()) : null;
    if (r.at !== r.end) invalid();
    if (packed) {
      const decoder = new FrameDecoder();
      const extra = packed.length ? this.decodeExtra(decoder, packed, true) : undefined;
      this.objects.clear();
      this.decoder = decoder;
      this.extras = extra;
      this.extrasTick = undefined;
      this.started = true;
    }
    this.hill = nextHill;
    for (const [id, obj] of changes) {
      if (obj) this.objects.set(id, obj);
      else this.objects.delete(id);
    }
  }

  private decodeExtra(decoder: FrameDecoder, bytes: Uint8Array, key: boolean) {
    if (bytes[0] !== 24 && (key || bytes[0] !== 8)) invalid();
    const d = decoder.decode(bytes);
    if (key && (!d.keyframe || d.seq !== 0)) invalid();
    return d.frame;
  }

  /** Resolve a point recorded in a mounted object’s parent space. */
  objectPoint(parent: number, pos: FilmVector): FilmVector | null {
    const seen = new Set<number>();
    let cursor: number | null = parent;
    let place: Transform = {pos, forward: [1, 0, 0], up: [0, 0, 1]};
    while (cursor !== null) {
      const o: ObjectState | undefined = this.objects.get(cursor);
      if (!o || seen.has(cursor)) return null;
      seen.add(cursor);
      place = transformed(place, o);
      cursor = o.parent;
    }
    return place.pos;
  }

  /** Simulation tick corresponding to the recording clock since the last observation. */
  matchTick(at: number): number | null {
    return this.extrasTick !== undefined && this.extras ? this.extras.tick + at - this.extrasTick : null;
  }

  frame(tick: number): Frame {
    const extras = this.extras;
    const fresh = tick === this.extrasTick;
    // Fire and damage come from timestamped film events. A packed action
    // (reload, melee, jump, etc.) belongs only to its own observation.
    function actions(p?: FrameActor | FramePlayer) {
      return { fire: undefined, shots: undefined, actions: fresh ? p?.actions : undefined,
        actions_ago: fresh ? p?.actions_ago : undefined };
    }
    const world: FilmWorld = { props: [], hill: this.hill };
    const out: Frame = { game_id: this.game, tick, bsp: extras?.bsp, cinematic: extras?.cinematic, cut: fresh ? extras?.cut : undefined,
      players: [], actors: [], vehicles: [], machines: [], machine_positions: [], objects: [], items: [], projectiles: [], film_world: world };
    if (extras?.cinematic || !this.started) return out;

    const playerExtra = new Map(extras?.players.map(p => [p.index, p]));
    const actorExtra = new Map(extras?.actors?.map(a => [a.id, a]));
    const transforms = new Map<number, Transform>();
    const worldOf = (o: ObjectState): Transform => {
      if (o.parent === null) return o;
      const existing = transforms.get(o.datum);
      if (existing) return existing;
      const chain: ObjectState[] = [];
      const seen = new Set<number>();
      let part: ObjectState | undefined = o;
      let place: Transform | undefined;
      while (part) {
        place = transforms.get(part.datum);
        if (place) break;
        if (seen.has(part.datum)) invalid();
        seen.add(part.datum);
        chain.push(part);
        part = part.parent === null ? undefined : this.objects.get(part.parent);
      }
      while (chain.length) {
        const p = chain.pop()!;
        place = place ? transformed(p, place) : p;
        transforms.set(p.datum, place);
      }
      return place!;
    };
    const occupied = new Set<number>();
    const weapons = new Map<number, string>();
    for (const o of this.objects.values()) {
      if (o.parent === null) continue;
      if (o.tag === "bipd") occupied.add(o.parent);
      if (o.tag === "weap" && !weapons.has(o.parent)) weapons.set(o.parent, o.kind);
    }
    const placements = new Map<string, FrameObject>();
    for (const o of this.objects.values()) {
      const { q, kind, tag } = o;
      const objId = id(o.datum), parent = o.parent === null ? null : this.objects.get(o.parent);
      const placement = q[2]! - 1, spawn = q[3]! - 1;
      const at = worldOf(o);
      const prop = (): FilmProp => {
        if (at !== o) return { id: objId, class: tag, kind, ...pickPose(at) };
        let entry = loose.get(o);
        if (!entry) {
          entry = { id: objId, class: tag, kind, ...pickPose(at) };
          loose.set(o, entry);
        }
        return entry;
      };
      if (tag === "bipd") {
        const ride = parent?.tag === "vehi" ? id(parent.datum) : null;
        const common = { pos: at.pos, yaw: Math.atan2(at.forward[1], at.forward[0]), aim: o.aim, vehicle: ride,
          seat: q[12] ? q[12] - 1 : null, species: kind };
        if (q[22]) {
          const index = (q[22] - 1) & 65535, p = playerExtra.get(index);
          const player: FramePlayer = { ...p, ...common, ...actions(p), index, species: o.name || p?.species,
            anim: p?.anim && q[16] ? { ...p.anim, frame: q[19]! / 16 } : p?.anim, alive: p?.alive ?? !q[21], weapon: p?.weapon ?? weapons.get(o.datum) };
          out.players.push(player);
          playerExtra.delete(index);
        } else if (!q[21]) {
          const a = actorExtra.get(objId);
          const actor: FrameActor = { ...a, ...common, ...actions(a), species: o.name || a?.species || kind,
            anim: a?.anim && q[16] ? { ...a.anim, frame: q[19]! / 16 } : a?.anim, id: objId, weapon: a?.weapon ?? weapons.get(o.datum) };
          out.actors!.push(actor);
          actorExtra.delete(objId);
        }
      } else if (tag === "vehi") {
        out.vehicles!.push({ id: objId, kind, pos: o.pos, forward: o.forward, up: o.up,
          occupied: occupied.has(o.datum), parent: parent ? id(parent.datum) : null, aim: o.aim });
      } else if (tag === "mach") {
        const position = q[20] ? (q[20] - 1) / 255 : null;
        out.machines!.push({ id: objId, kind, ...pickPose(at), position, node: placement < 0 ? null : `${kind}_${placement}` });
        if (position !== null && placement >= 0) out.machine_positions!.push({ i: placement, p: q[20]! - 1 });
      } else if (tag === "proj") {
        out.projectiles!.push({ id: objId, kind, pos: at.pos, parent: parent ? id(parent.datum) : null });
        world.props.push(prop());
      } else if (spawn >= 0 || placement >= 0) {
        const k = spawn >= 0 ? "netgame" : tag, i = spawn >= 0 ? spawn : placement;
        const key = k + ":" + i;
        let entry = at === o ? placed.get(o) : undefined;
        if (!entry) {
          const source = { id: objId, class: tag, kind };
          entry = parent && (tag === "weap" || tag === "eqip") ? { k, i, source, gone: true }
            : { k, i, source, p: at.pos, f: at.forward, u: at.up };
          if (at === o) placed.set(o, entry);
        }
        const previous = placements.get(key);
        if (!previous || previous.gone) placements.set(key, entry);
        else if (!entry.gone) world.props.push(prop()); // dropped item and its respawn both exist
      } else if (!(parent && (tag === "weap" || tag === "eqip"))) {
        if (tag === "weap" || tag === "eqip") out.items!.push({ id: objId, kind, equipment: tag === "eqip", ...pickPose(at) });
        world.props.push(prop());
      }
    }
    out.objects = [...placements.values()];
    for (const p of playerExtra.values()) out.players.push({ ...p, ...actions(p) });
    for (const a of actorExtra.values()) out.actors!.push({ ...a, ...actions(a) });
    return out;
  }
}

function pickPose(at: Transform): Transform { return { pos: at.pos, forward: at.forward, up: at.up }; }

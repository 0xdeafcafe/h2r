// Frame buffering and interpolation. Frames are not reactive state: they
// arrive at up to 15 Hz per game and are read directly by the canvas loop.

import type { Frame, FrameHit, FrameMachine, FrameObject, FramePlayer, FrameVehicle } from "../api/types";

export const TICKS_PER_SECOND = 30;
const MS_PER_TICK = 1000 / TICKS_PER_SECOND;

export interface SampledPlayer {
  index: number;
  alive: boolean;
  x: number;
  y: number;
  z: number;
  yaw: number;
  vehicle: string | null;
  /** The biped's species id, when the frame says. */
  species?: string | null;
  /** Unit aim direction (game coordinates), when the frame says. */
  aim?: Aim | null;
  /** The held weapon's stem, when the frame says. */
  weapon?: string | null;
  /** Shots fired in the frame at or before the sampled tick (absent for
   *  none), and that frame's tick: together they name one burst, so a view
   *  can tell a new one from the one it has already drawn. */
  shots?: number;
  shotTick?: number;
  /** How they are moving, when the frame says. */
  stance?: Stance;
  /** Horizontal speed, game units a second, when the frame says. */
  speed?: number;
  /** One-shot actions started in the frame at or before the sampled tick,
   *  and that frame's tick (as `shots` and `shotTick` name a burst). */
  actions?: Action[];
  actionTick?: number;
  /** The grenade they hold or threw. */
  grenade?: string;
  /** Set for an AI-controlled biped (campaign), which is sampled, eased and
   *  drawn like a player: its id and side. Its `index` is made from the id
   *  and is negative, so it never meets a player's. */
  actor?: { id: string; team: string | null };
}

/** A vehicle with someone in it, at one moment. */
export interface SampledVehicle {
  id: string;
  kind: string;
  x: number; y: number; z: number;
  /** Unit vectors, game coordinates. */
  forward: Aim;
  up: Aim;
  /** False when nobody is in it (drawn dimmer). */
  occupied: boolean;
}

/** One piece of damage, as the views draw it. */
export interface SampledHit {
  /** Attacker and victim as the frame names them ("p<index>", or an actor's or vehicle's id); null when unknown. */
  from: string | null;
  to: string | null;
  weapon: string | null;
  /** Where the victim was. */
  x: number; y: number; z: number;
  amount: number | null;
  kind: string | null;
}

/** The damage in the frame at or before `tick`, with that frame's tick (so
 *  a view draws each frame's hits once), or null when it has none. */
export function sampleHits(frames: readonly Frame[], tick: number): { tick: number; hits: SampledHit[] } | null {
  const i = findFrame(frames, tick);
  const f = i < 0 ? undefined : frames[i];
  if (!f?.hits?.length) return null;
  let c = hitCache.get(f);
  if (!c) {
    c = [];
    for (const h of f.hits as FrameHit[]) {
      if (c.length >= MAX_HITS_PER_FRAME) break;
      if (!h || !isPos(h.pos)) continue;
      const amount = typeof h.amount === "number" && Number.isFinite(h.amount) && h.amount >= 0 ? h.amount : null;
      c.push({ from: cleanId(h.from), to: cleanId(h.to), weapon: cleanId(h.weapon), x: h.pos[0], y: h.pos[1], z: h.pos[2], amount, kind: cleanId(h.kind) });
    }
    hitCache.set(f, c);
  }
  return c.length ? { tick: f.tick, hits: c } : null;
}
const hitCache = new WeakMap<Frame, SampledHit[]>();

/** The structure BSP loaded at `tick` (a campaign level's zone), or null
 *  when the frames don't say. */
export function sampleBsp(frames: readonly Frame[], tick: number): number | null {
  const i = findFrame(frames, tick);
  const b = (i < 0 ? frames[0] : frames[i])?.bsp;
  return typeof b === "number" && Number.isInteger(b) && b >= 0 ? b : null;
}

/** At most this many actors are drawn from one frame, as the contract caps them. */
export const MAX_ACTORS = 128;

/** An actor's `index`: negative, and the same for the same id in every frame. */
export function actorIndex(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (Math.imul(h, 31) + id.charCodeAt(i)) | 0;
  return -1 - (h >>> 0);
}

export type Aim = [number, number, number];

export const STANCES = ["stand", "crouch", "airborne", "vehicle", "dead"] as const;
export type Stance = (typeof STANCES)[number];
export const ACTIONS = ["grenade", "melee", "reload", "jump", "switch", "pickup"] as const;
export type Action = (typeof ACTIONS)[number];
export const MAX_ACTIONS_PER_FRAME = 8;
export const MAX_HITS_PER_FRAME = 64;
export const MAX_VEHICLES = 64;

export function cleanStance(v: unknown): Stance | null {
  return typeof v === "string" && (STANCES as readonly string[]).includes(v) ? (v as Stance) : null;
}

/** A frame's actions: the known ones, in order, at most eight. */
export function cleanActions(v: unknown): Action[] {
  if (!Array.isArray(v)) return [];
  const out: Action[] = [];
  for (const a of v) {
    if (out.length >= MAX_ACTIONS_PER_FRAME) break;
    if (typeof a === "string" && (ACTIONS as readonly string[]).includes(a)) out.push(a as Action);
  }
  return out;
}

/** A speed: finite and not negative, else null. */
export function cleanSpeed(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null;
}

const ID = /^[a-z0-9_]{1,32}$/;
export const MAX_SHOTS_PER_FRAME = 64;

/** A species or weapon id as the contract spells them, else null. */
export function cleanId(v: unknown): string | null {
  return typeof v === "string" && ID.test(v) ? v : null;
}

/** An aim made unit length; null for anything that isn't three finite
 *  numbers with some length. */
export function cleanAim(v: unknown): Aim | null {
  if (!Array.isArray(v) || v.length !== 3) return null;
  const [x, y, z] = v as unknown[];
  if (typeof x !== "number" || typeof y !== "number" || typeof z !== "number") return null;
  const len = Math.hypot(x, y, z);
  if (!Number.isFinite(len) || len < 1e-6) return null;
  return [x / len, y / len, z / len];
}

/** A frame's shot count: a whole number from 0 to 64. */
export function cleanShots(v: unknown): number {
  if (typeof v !== "number" || !Number.isFinite(v) || v < 1) return 0;
  return Math.min(MAX_SHOTS_PER_FRAME, Math.floor(v));
}

/** Level aim along a yaw (0 = +x, counter-clockwise). */
export function yawAim(yaw: number): Aim {
  return [Math.cos(yaw), Math.sin(yaw), 0];
}

/** Where a player is looking: their aim, or straight ahead when the frame
 *  has none (an older sender). */
export function aimOf(p: { aim?: Aim | null; yaw: number }): Aim {
  return p.aim ?? yawAim(p.yaw);
}

/** Blends two unit directions and keeps the result unit length. Opposite
 *  directions have no middle: the nearer end is used. */
export function nlerpAim(a: Aim, b: Aim, t: number): Aim {
  const x = lerp(a[0], b[0], t), y = lerp(a[1], b[1], t), z = lerp(a[2], b[2], t);
  const len = Math.hypot(x, y, z);
  if (len < 1e-4) return t < 0.5 ? [a[0], a[1], a[2]] : [b[0], b[1], b[2]];
  return [x / len, y / len, z / len];
}

/** Distance (world units) above which we snap instead of interpolating,
 *  e.g. a respawn or teleporter. */
export const SNAP_DISTANCE = 12;

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/** Interpolates angles along the short way round. */
export function lerpAngle(a: number, b: number, t: number): number {
  let d = (b - a) % (Math.PI * 2);
  if (d > Math.PI) d -= Math.PI * 2;
  if (d < -Math.PI) d += Math.PI * 2;
  return a + d * t;
}

/** A frame's player as sampled. `shotsFrom` is the frame the sample's shots
 *  are read from (the one at or before the sampled tick), when not this one. */
export function fromFramePlayer(p: FramePlayer, tick?: number, shotsFrom: FramePlayer | undefined = p): SampledPlayer {
  const out: SampledPlayer = { index: p.index, alive: p.alive, x: p.pos[0], y: p.pos[1], z: p.pos[2], yaw: p.yaw, vehicle: p.vehicle ?? null };
  const species = cleanId(p.species), weapon = cleanId(p.weapon), aim = cleanAim(p.aim);
  if (species) out.species = species;
  if (weapon) out.weapon = weapon;
  if (aim) out.aim = aim;
  const shots = tick === undefined || !shotsFrom ? 0 : cleanShots(shotsFrom.shots);
  if (shots && shotsFrom!.alive) { out.shots = shots; out.shotTick = tick; }
  const stance = cleanStance(p.stance), speed = cleanSpeed(p.speed), grenade = cleanId(p.grenade);
  if (stance) out.stance = stance;
  if (speed !== null) out.speed = speed;
  if (grenade) out.grenade = grenade;
  const actions = tick === undefined || !shotsFrom ? [] : cleanActions(shotsFrom.actions);
  if (actions.length) { out.actions = actions; out.actionTick = tick; }
  return out;
}

/** Blends two frames at `tick`. Players missing from `b` hold their
 *  position from `a`; players only in `b` appear when t >= 0.5. */
export function interpolateFrames(a: Frame, b: Frame | undefined, tick: number): SampledPlayer[] {
  return blendPlayers(a.players, b && b.tick > a.tick ? b.players : undefined, a.tick, b?.tick ?? a.tick, tick);
}

function blendPlayers(from: readonly FramePlayer[], to: readonly FramePlayer[] | undefined, tickA: number, tickB: number, tick: number): SampledPlayer[] {
  const a = { tick: tickA, players: from };
  if (!to) return a.players.map((p) => fromFramePlayer(p, a.tick));
  const b = { tick: tickB, players: to };
  const t = Math.min(1, Math.max(0, (tick - a.tick) / (b.tick - a.tick)));
  const next = new Map<number, FramePlayer>();
  for (const p of b.players) next.set(p.index, p);
  const out: SampledPlayer[] = [];
  for (const pa of a.players) {
    const pb = next.get(pa.index);
    next.delete(pa.index);
    if (!pb) { out.push(fromFramePlayer(pa, a.tick)); continue; }
    const dx = pb.pos[0] - pa.pos[0], dy = pb.pos[1] - pa.pos[1], dz = pb.pos[2] - pa.pos[2];
    const far = dx * dx + dy * dy + dz * dz > SNAP_DISTANCE * SNAP_DISTANCE;
    if (!pa.alive || !pb.alive || far) {
      // No sliding a corpse to a spawn point: take the nearer frame.
      out.push(fromFramePlayer(t < 0.5 ? pa : pb, a.tick, pa));
      continue;
    }
    // Everything but the place comes from the nearer frame; shots from `a`.
    const s = fromFramePlayer(t < 0.5 ? pa : pb, a.tick, pa);
    s.x = lerp(pa.pos[0], pb.pos[0], t);
    s.y = lerp(pa.pos[1], pb.pos[1], t);
    s.z = lerp(pa.pos[2], pb.pos[2], t);
    s.yaw = lerpAngle(pa.yaw, pb.yaw, t);
    const aa = cleanAim(pa.aim), ab = cleanAim(pb.aim);
    if (aa && ab) s.aim = nlerpAim(aa, ab, t);
    const va = cleanSpeed(pa.speed), vb = cleanSpeed(pb.speed);
    if (va !== null && vb !== null) s.speed = lerp(va, vb, t);
    out.push(s);
  }
  if (t >= 0.5) for (const pb of next.values()) out.push(fromFramePlayer(pb));
  return out;
}

/** Index of the last frame with tick <= `tick` in a tick-sorted array, or -1. */
export function findFrame(frames: readonly Frame[], tick: number): number {
  let lo = 0, hi = frames.length - 1, ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (frames[mid]!.tick <= tick) { ans = mid; lo = mid + 1; } else hi = mid - 1;
  }
  return ans;
}

/** Samples a sorted frame list at a (fractional) tick. */
export function sampleFrames(frames: readonly Frame[], tick: number): SampledPlayer[] {
  if (frames.length === 0) return [];
  const i = findFrame(frames, tick);
  if (i < 0) return frames[0]!.players.map((p) => fromFramePlayer(p));
  return interpolateFrames(frames[i]!, frames[i + 1], tick);
}

// ---- actors and vehicles ----

const isPos = (v: unknown): v is [number, number, number] =>
  Array.isArray(v) && v.length === 3 && v.every((n) => typeof n === "number" && Number.isFinite(n));

/** A frame's actors as players are sampled (so one blend serves both),
 *  with what makes them actors beside them. Anything off the contract is
 *  left out; no more than the cap are kept. */
function actorsOf(f: Frame): { players: FramePlayer[]; info: Map<number, { id: string; team: string | null }> } {
  const players: FramePlayer[] = [], info = new Map<number, { id: string; team: string | null }>();
  for (const a of f.actors ?? []) {
    if (players.length >= MAX_ACTORS) break;
    const id = cleanId(a?.id);
    if (!id || !isPos(a.pos) || typeof a.yaw !== "number" || !Number.isFinite(a.yaw)) continue;
    const index = actorIndex(id);
    if (info.has(index)) continue;
    info.set(index, { id, team: cleanId(a.team) });
    players.push({
      index, alive: true, pos: a.pos, yaw: a.yaw, vehicle: cleanId(a.vehicle), species: a.species, aim: a.aim, weapon: a.weapon, shots: a.shots,
      stance: a.stance, speed: a.speed, actions: a.actions, grenade: a.grenade,
    });
  }
  return { players, info };
}

const actorCache = new WeakMap<Frame, ReturnType<typeof actorsOf>>();
function actorsCached(f: Frame) {
  let c = actorCache.get(f);
  if (!c) { c = actorsOf(f); actorCache.set(f, c); }
  return c;
}

/** The actors at a (fractional) tick, blended between frames like players.
 *  Only the living are sent, so one missing from the later frame is gone
 *  once that frame is the nearer. */
export function sampleActors(frames: readonly Frame[], tick: number): SampledPlayer[] {
  const i = findFrame(frames, tick);
  if (i < 0) return [];
  const fa = frames[i]!, fb = frames[i + 1];
  if (!fa.actors?.length && !fb?.actors?.length) return [];
  const a = actorsCached(fa), b = fb && fb.tick > fa.tick ? actorsCached(fb) : undefined;
  const late = !!fb && !!b && (tick - fa.tick) / (fb.tick - fa.tick) >= 0.5;
  const out: SampledPlayer[] = [];
  for (const p of blendPlayers(a.players, b?.players, fa.tick, fb?.tick ?? fa.tick, tick)) {
    const info = b?.info.get(p.index) ?? a.info.get(p.index);
    if (!info || (late && !b!.info.has(p.index))) continue;
    p.actor = info;
    out.push(p);
  }
  return out;
}

function fromFrameVehicle(v: FrameVehicle): SampledVehicle | null {
  const id = cleanId(v?.id), kind = cleanId(v?.kind);
  if (!id || !isPos(v.pos)) return null;
  const forward = cleanAim(v.forward) ?? [1, 0, 0];
  return { id, kind: kind ?? "vehicle", x: v.pos[0], y: v.pos[1], z: v.pos[2], forward, up: cleanAim(v.up) ?? [0, 0, 1], occupied: v.occupied !== false };
}

/** The vehicles at a (fractional) tick. One in only the earlier frame
 *  stays until the later frame is the nearer; one only in the later
 *  appears then. A big jump snaps. */
export function sampleVehicles(frames: readonly Frame[], tick: number): SampledVehicle[] {
  const i = findFrame(frames, tick);
  if (i < 0) return [];
  const fa = frames[i]!, fb = frames[i + 1];
  const from = (fa.vehicles ?? []).map(fromFrameVehicle).filter((v): v is SampledVehicle => !!v);
  if (!fb || fb.tick <= fa.tick) return from;
  const t = Math.min(1, Math.max(0, (tick - fa.tick) / (fb.tick - fa.tick)));
  const next = new Map<string, SampledVehicle>();
  for (const raw of fb.vehicles ?? []) { const v = fromFrameVehicle(raw); if (v) next.set(v.id, v); }
  const out: SampledVehicle[] = [];
  for (const a of from) {
    const b = next.get(a.id);
    next.delete(a.id);
    if (!b) { if (t < 0.5) out.push(a); continue; }
    if ((b.x - a.x) ** 2 + (b.y - a.y) ** 2 + (b.z - a.z) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE) { out.push(t < 0.5 ? a : b); continue; }
    out.push({
      id: a.id, kind: t < 0.5 ? a.kind : b.kind,
      x: lerp(a.x, b.x, t), y: lerp(a.y, b.y, t), z: lerp(a.z, b.z, t),
      forward: nlerpAim(a.forward, b.forward, t), up: nlerpAim(a.up, b.up, t),
      occupied: t < 0.5 ? a.occupied : b.occupied,
    });
  }
  if (t >= 0.5) for (const b of next.values()) out.push(b);
  return out;
}

// ---- machines ----

export const MAX_MACHINES = 128;

export interface SampledMachine {
  id: string;
  /** The map glb's `machines` child it moves, when it is one. */
  node: string | null;
  kind: string;
  x: number; y: number; z: number;
  forward: Aim;
  up: Aim;
  /** A device's travel, 0 to 1, when it says. */
  position: number | null;
}

export function fromFrameMachine(m: FrameMachine): SampledMachine | null {
  const id = cleanId(m?.id);
  if (!id || !isPos(m.pos)) return null;
  // A node name is a tag stem and a number; anything else can't name a glb node.
  const node = typeof m.node === "string" && /^[a-z0-9_ .-]{1,64}$/i.test(m.node) ? m.node : null;
  const position = typeof m.position === "number" && Number.isFinite(m.position) ? Math.min(1, Math.max(0, m.position)) : null;
  return { id, node, kind: cleanId(m.kind) ?? "machine", x: m.pos[0], y: m.pos[1], z: m.pos[2], forward: cleanAim(m.forward) ?? [1, 0, 0], up: cleanAim(m.up) ?? [0, 0, 1], position };
}

/** How many frames back a machine's last word is looked for: recorded
 *  frames carry every machine, live ones only those that moved. */
const MACHINE_LOOKBACK = 4;

/** The machines at a (fractional) tick: each one's latest state in the
 *  frames at or just behind it, eased towards the next frame's when that
 *  has it too. One that hasn't been sent lately isn't here: a live view
 *  keeps the last it heard (FrameBuffer). */
export function sampleMachines(frames: readonly Frame[], tick: number): SampledMachine[] {
  const i = findFrame(frames, tick);
  if (i < 0) return [];
  const at = new Map<string, { m: SampledMachine; tick: number }>();
  for (let j = i; j >= 0 && j > i - MACHINE_LOOKBACK; j--) {
    const list = frames[j]!.machines;
    if (!list) continue;
    for (const raw of list.slice(0, MAX_MACHINES)) { const m = fromFrameMachine(raw); if (m && !at.has(m.id)) at.set(m.id, { m, tick: frames[j]!.tick }); }
  }
  const fb = frames[i + 1];
  const next = new Map<string, SampledMachine>();
  if (fb && fb.tick > frames[i]!.tick) for (const raw of (fb.machines ?? []).slice(0, MAX_MACHINES)) { const m = fromFrameMachine(raw); if (m) next.set(m.id, m); }
  const out: SampledMachine[] = [];
  for (const { m: a, tick: ta } of at.values()) {
    const b = next.get(a.id);
    if (!b || (b.x - a.x) ** 2 + (b.y - a.y) ** 2 + (b.z - a.z) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE) { out.push(a); continue; }
    // It was last heard of at `ta`, but it only started for where `b` has it after the frame behind `tick`.
    const from = Math.max(ta, frames[i]!.tick), t = Math.min(1, Math.max(0, (tick - from) / (fb!.tick - from)));
    out.push({
      ...a, x: lerp(a.x, b.x, t), y: lerp(a.y, b.y, t), z: lerp(a.z, b.z, t),
      forward: nlerpAim(a.forward, b.forward, t), up: nlerpAim(a.up, b.up, t),
      position: a.position !== null && b.position !== null ? lerp(a.position, b.position, t) : a.position,
    });
  }
  return out;
}

/** Most scenario objects a frame is read for. */
export const MAX_OBJECTS = 2048;

const finite3 = (v: unknown): v is [number, number, number] =>
  Array.isArray(v) && v.length === 3 && v.every((c) => typeof c === "number" && Number.isFinite(c));

/** Scenario objects not where the scenario put them at `tick`: the frame
 *  behind it says which (moved, or gone), and a moved one eases towards
 *  where the next frame has it. Null with no frame behind `tick`. */
export function sampleObjects(frames: readonly Frame[], tick: number): FrameObject[] | null {
  const i = findFrame(frames, tick);
  if (i < 0) return null;
  const a = frames[i]!, b = frames[i + 1];
  const list = (a.objects ?? []).slice(0, MAX_OBJECTS);
  const next = new Map<string, FrameObject>();
  if (b && b.tick > a.tick) for (const o of (b.objects ?? []).slice(0, MAX_OBJECTS)) if (!o.gone) next.set(`${o.k}:${o.i}`, o);
  const t = b && b.tick > a.tick ? Math.min(1, Math.max(0, (tick - a.tick) / (b.tick - a.tick))) : 0;
  const out: FrameObject[] = [];
  for (const o of list) {
    if (typeof o.k !== "string" || typeof o.i !== "number") continue;
    if (o.gone) { out.push({ k: o.k, i: o.i, gone: true }); continue; }
    if (!finite3(o.p)) continue;
    const f = finite3(o.f) ? o.f : [1, 0, 0] as [number, number, number];
    const u = finite3(o.u) ? o.u : [0, 0, 1] as [number, number, number];
    const n = next.get(`${o.k}:${o.i}`);
    if (!n || !finite3(n.p) || t === 0 || (n.p[0] - o.p[0]) ** 2 + (n.p[1] - o.p[1]) ** 2 + (n.p[2] - o.p[2]) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE) {
      out.push({ k: o.k, i: o.i, p: [o.p[0], o.p[1], o.p[2]], f, u });
      continue;
    }
    out.push({
      k: o.k, i: o.i,
      p: [lerp(o.p[0], n.p[0], t), lerp(o.p[1], n.p[1], t), lerp(o.p[2], n.p[2], t)],
      f: finite3(n.f) ? nlerpAim(f, n.f, t) : f,
      u: finite3(n.u) ? nlerpAim(u, n.u, t) : u,
    });
  }
  return out;
}

export interface DeathMarker { index: number; x: number; y: number; z: number; tick: number; bornAt: number }

/** A live game's recent frames plus a clock mapping wall time to ticks.
 *  Rendering runs `delayTicks` behind the newest frame so there is always a
 *  frame on both sides to interpolate between. */
export class FrameBuffer {
  frames: Frame[] = [];
  markers: DeathMarker[] = [];
  /** Wall time (ms) at which tick 0 would have arrived; smoothed. */
  private offset: number | null = null;
  readonly bounds = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity, minZ: Infinity, maxZ: -Infinity };

  /** Every machine heard of, as last sent. */
  private machines = new Map<string, SampledMachine>();

  constructor(readonly capacity = 48, readonly delayTicks = 5) {}

  get lastTick(): number {
    return this.frames.length ? this.frames[this.frames.length - 1]!.tick : 0;
  }

  push(frame: Frame, now: number): void {
    const last = this.frames[this.frames.length - 1];
    if (last && frame.tick <= last.tick) {
      if (frame.tick < last.tick - TICKS_PER_SECOND * 5) {
        // The game restarted its tick counter: start over.
        this.frames = [];
        this.machines.clear();
        this.offset = null;
      } else return;
    }
    this.frames.push(frame);
    // Machines are only sent when they move: keep the last word on each.
    for (const raw of (frame.machines ?? []).slice(0, MAX_MACHINES)) {
      const m = fromFrameMachine(raw);
      if (m && (this.machines.has(m.id) || this.machines.size < MAX_MACHINES)) this.machines.set(m.id, m);
    }
    if (this.frames.length > this.capacity) this.frames.splice(0, this.frames.length - this.capacity);
    for (const p of frame.players) {
      const [x, y, z] = p.pos;
      const b = this.bounds;
      if (x < b.minX) b.minX = x; if (x > b.maxX) b.maxX = x;
      if (y < b.minY) b.minY = y; if (y > b.maxY) b.maxY = y;
      if (z < b.minZ) b.minZ = z; if (z > b.maxZ) b.maxZ = z;
    }
    const o = now - frame.tick * MS_PER_TICK;
    if (this.offset === null || Math.abs(o - this.offset) > 1000) this.offset = o;
    // Late frames pull the clock back quickly; early ones nudge it slowly.
    else this.offset = o > this.offset ? this.offset * 0.9 + o * 0.1 : this.offset * 0.7 + o * 0.3;
  }

  /** The (fractional) tick to render at wall time `now`. */
  renderTick(now: number): number {
    if (this.offset === null) return this.lastTick;
    const est = (now - this.offset) / MS_PER_TICK - this.delayTicks;
    return Math.min(est, this.lastTick);
  }

  sample(now: number): SampledPlayer[] {
    return sampleFrames(this.frames, this.renderTick(now));
  }

  sampleActors(now: number): SampledPlayer[] {
    return sampleActors(this.frames, this.renderTick(now));
  }

  sampleVehicles(now: number): SampledVehicle[] {
    return sampleVehicles(this.frames, this.renderTick(now));
  }

  sampleHits(now: number) {
    return sampleHits(this.frames, this.renderTick(now));
  }

  sampleBsp(now: number): number | null {
    return sampleBsp(this.frames, this.renderTick(now));
  }

  /** Every machine: the ones moving now where the render tick has them, the rest where they were last sent. */
  sampleMachines(now: number): SampledMachine[] {
    if (!this.machines.size) return [];
    const out = new Map(this.machines);
    for (const m of sampleMachines(this.frames, this.renderTick(now))) out.set(m.id, m);
    return [...out.values()];
  }

  /** Scenario objects out of place at the render tick (null before the
   *  first frame). */
  sampleObjects(now: number): FrameObject[] | null {
    return sampleObjects(this.frames, this.renderTick(now));
  }

  /** Records where a victim died: the kill's `pos` when given, else the
   *  frame nearest the kill tick. */
  addDeath(index: number, tick: number, now: number, pos?: [number, number, number] | null): void {
    let at: { x: number; y: number; z: number } | undefined;
    if (pos) at = { x: pos[0], y: pos[1], z: pos[2] };
    else at = sampleFrames(this.frames, Math.min(tick, this.lastTick)).find((q) => q.index === index);
    if (!at) return;
    this.markers.push({ index, x: at.x, y: at.y, z: at.z, tick, bornAt: now });
    if (this.markers.length > 32) this.markers.shift();
  }

  liveMarkers(now: number, ttlMs: number): DeathMarker[] {
    while (this.markers.length && now - this.markers[0]!.bornAt > ttlMs) this.markers.shift();
    return this.markers;
  }
}

/** All live games' frame buffers, keyed by game id. */
export class FrameHub {
  private buffers = new Map<string, FrameBuffer>();

  get(gameId: string): FrameBuffer {
    let b = this.buffers.get(gameId);
    if (!b) { b = new FrameBuffer(); this.buffers.set(gameId, b); }
    return b;
  }

  has(gameId: string): boolean {
    return this.buffers.has(gameId);
  }

  push(frame: Frame, now: number): void {
    this.get(frame.game_id).push(frame, now);
  }

  drop(gameId: string): void {
    this.buffers.delete(gameId);
  }

  clear(): void {
    this.buffers.clear();
  }
}

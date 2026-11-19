// Frame buffering and interpolation. Frames are not reactive state: they
// arrive at up to 15 Hz per game and are read directly by the canvas loop.

import { isMachineBaseline } from "../api/pack";
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
  /** Velocity, game units a second, when the frame says (version 3 senders). */
  vel?: Aim;
  /** The animation the game plays on it, at the sampled tick (version 4). */
  anim?: SampledAnim;
  /** Its seat in `vehicle` (the vehicle's own seat index), when the frame says. */
  seat?: number;
  /** The ticks it fired on, from the frames' `fire` masks, up to the
   *  sampled tick and no further back than the frame behind it (oldest
   *  first): each shot at its own tick. With masks, `shots` is left out. */
  fireTicks?: number[];
}

/** A unit's animation as the game plays it: its name in the unit's graph
 *  (`combat:rifle:any:move_front`) and the frame, fractional. */
export interface SampledAnim { name: string; frame: number }

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
  /** A turret: the vehicle it is mounted on. Sampled at that one's place
   *  (the frames send a turret's parent-relative one); drawn as part of it. */
  parent?: string;
  /** Where it aims (a turret), game coordinates, unit length. */
  aim?: Aim;
  /** Velocity, game units a second, when the frame says (packed v6). */
  vel?: Aim;
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
  if (!f?.hits?.length || f.cinematic) return null;
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

/** Faster than this (game units a second) is a bad read, not a velocity. */
export const MAX_VELOCITY = 80;

/** A velocity: three finite numbers no faster than [`MAX_VELOCITY`], else null. */
export function cleanVel(v: unknown): Aim | null {
  if (!Array.isArray(v) || v.length !== 3 || !v.every((c) => typeof c === "number" && Number.isFinite(c))) return null;
  const [x, y, z] = v as number[];
  return Math.hypot(x!, y!, z!) <= MAX_VELOCITY ? [x!, y!, z!] : null;
}

const ANIM_NAME = /^[a-z0-9_]+(:[a-z0-9_]+){0,7}$/;

/** An animation as the contract has it: a graph name of lower-case words
 *  joined by colons (at most 96 characters) and a frame from 0, else null. */
export function cleanAnim(v: unknown): SampledAnim | null {
  if (!v || typeof v !== "object") return null;
  const { name, frame } = v as { name?: unknown; frame?: unknown };
  if (typeof name !== "string" || name.length > 96 || !ANIM_NAME.test(name)) return null;
  if (typeof frame !== "number" || !Number.isFinite(frame) || frame < 0 || frame >= 32768) return null;
  return { name, frame };
}

/** A seat index: a whole number from 0 to 31, else null. */
export function cleanSeat(v: unknown): number | null {
  return typeof v === "number" && Number.isInteger(v) && v >= 0 && v < 32 ? v : null;
}

/** The ticks a `fire` mask names, at a frame's `tick`: bit k is a shot at
 *  `tick - k`. Oldest first; only those after `after` and up to `upTo`. */
export function fireTicks(mask: unknown, tick: number, after: number, upTo: number): number[] {
  if (typeof mask !== "number" || !Number.isInteger(mask) || mask <= 0) return [];
  const out: number[] = [];
  for (let k = 31; k >= 0; k--) {
    if (!((mask >>> k) & 1)) continue;
    const t = tick - k;
    if (t > after && t <= upTo) out.push(t);
  }
  return out;
}

/** Cubic Hermite between `a` (velocity `va`) at t = 0 and `b` (velocity
 *  `vb`) at t = 1, the velocities in units per whole span. */
export function hermite(a: number, va: number, b: number, vb: number, t: number): number {
  const t2 = t * t, t3 = t2 * t;
  return (2 * t3 - 3 * t2 + 1) * a + (t3 - 2 * t2 + t) * va + (-2 * t3 + 3 * t2) * b + (t3 - t2) * vb;
}

/** Longest gap (ticks) that is curved by the samples' velocities; a
 *  longer one is a straight line (what the velocities say that far apart
 *  is too old to bend the path by). */
export const HERMITE_MAX_TICKS = 2 * TICKS_PER_SECOND;

/** The animation between two samples at `tick`. One animation on both
 *  sides moving at about the game's pace moves its frame between theirs;
 *  otherwise (a change, or a loop that wrapped between them) the nearer sample's
 *  animation is taken and its frame carried on at the game's rate (one
 *  frame a tick), forward from the earlier or back from the later, so it
 *  stays where the game had it. */
export function blendAnim(a: SampledAnim | null, b: SampledAnim | null, tickA: number, tickB: number, tick: number): SampledAnim | null {
  if (!a && !b) return null;
  const span = tickB - tickA;
  const t = span > 0 ? Math.min(1, Math.max(0, (tick - tickA) / span)) : 0;
  if (a && b && a.name === b.name && span > 0) {
    // Held on one frame (a body lying still, a pose).
    if (b.frame === a.frame) return { name: a.name, frame: a.frame };
    // Moved forward at about the game's pace (half to twice a frame a
    // tick): straight between. Anything else wrapped round a loop.
    const d = b.frame - a.frame;
    if (d >= span * 0.5 && d <= span * 2) return { name: a.name, frame: lerp(a.frame, b.frame, t) };
  }
  if (a && (!b || t < 0.5)) return { name: a.name, frame: a.frame + Math.max(0, tick - tickA) };
  return { name: b!.name, frame: Math.max(0, b!.frame - Math.max(0, tickB - tick)) };
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
  // A frame with a fire mask has its shots placed on their ticks (blendPlayers).
  const shots = tick === undefined || !shotsFrom || shotsFrom.fire != null ? 0 : cleanShots(shotsFrom.shots);
  if (shots && shotsFrom!.alive) { out.shots = shots; out.shotTick = tick; }
  const stance = cleanStance(p.stance), speed = cleanSpeed(p.speed), grenade = cleanId(p.grenade);
  if (stance) out.stance = stance;
  if (speed !== null) out.speed = speed;
  if (grenade) out.grenade = grenade;
  const actions = tick === undefined || !shotsFrom ? [] : cleanActions(shotsFrom.actions);
  if (actions.length) { out.actions = actions; out.actionTick = tick; }
  const vel = cleanVel(p.vel), anim = cleanAnim(p.anim), seat = out.vehicle ? cleanSeat(p.seat) : null;
  if (vel) out.vel = vel;
  if (anim) out.anim = anim;
  if (seat !== null) out.seat = seat;
  return out;
}

/** Blends two frames at `tick`. Players missing from `b` hold their
 *  position from `a`; players only in `b` appear when t >= 0.5. */
export function interpolateFrames(a: Frame, b: Frame | undefined, tick: number): SampledPlayer[] {
  return blendPlayers(a.players, b && b.cut == null && b.tick > a.tick ? b.players : undefined, a.tick, b?.tick ?? a.tick, tick);
}

function blendPlayers(from: readonly FramePlayer[], to: readonly FramePlayer[] | undefined, tickA: number, tickB: number, tick: number): SampledPlayer[] {
  const a = { tick: tickA, players: from };
  // Shots the earlier frame's mask names, up to now (the newest frame of a live stream).
  const fired = (s: SampledPlayer, p: FramePlayer, at: number, after: number) => {
    if (p.fire == null || !p.alive) return;
    const ticks = fireTicks(p.fire, at, after, tick);
    if (ticks.length) s.fireTicks = s.fireTicks ? s.fireTicks.concat(ticks) : ticks;
  };
  if (!to) return a.players.map((p) => { const s = fromFramePlayer(p, a.tick); fired(s, p, a.tick, a.tick - 32); return s; });
  const b = { tick: tickB, players: to };
  const t = Math.min(1, Math.max(0, (tick - a.tick) / (b.tick - a.tick)));
  const next = new Map<number, FramePlayer>();
  for (const p of b.players) next.set(p.index, p);
  const out: SampledPlayer[] = [];
  for (const pa of a.players) {
    const pb = next.get(pa.index);
    next.delete(pa.index);
    if (!pb) { const s = fromFramePlayer(pa, a.tick); fired(s, pa, a.tick, a.tick - 32); out.push(s); continue; }
    const dx = pb.pos[0] - pa.pos[0], dy = pb.pos[1] - pa.pos[1], dz = pb.pos[2] - pa.pos[2];
    const far = dx * dx + dy * dy + dz * dz > SNAP_DISTANCE * SNAP_DISTANCE;
    if (!pa.alive || !pb.alive || far) {
      // No sliding a corpse to a spawn point: take the nearer frame.
      out.push(fromFramePlayer(t < 0.5 ? pa : pb, a.tick, pa));
      continue;
    }
    // Everything but the place comes from the nearer frame; shots from `a`,
    // or with masks each on its tick: the later frame's as the clock passes them.
    const s = fromFramePlayer(t < 0.5 ? pa : pb, a.tick, pa);
    fired(s, pa, a.tick, a.tick - 32);
    fired(s, pb, b.tick, a.tick);
    const va = cleanVel(pa.vel), vb = cleanVel(pb.vel);
    if (va && vb && b.tick - a.tick <= HERMITE_MAX_TICKS) {
      // Both samples say how they were moving: the path that leaves `a`
      // and arrives at `b` at those velocities (game units a second, so
      // scaled to the span), and the velocity along it.
      const span = (b.tick - a.tick) / TICKS_PER_SECOND;
      s.x = hermite(pa.pos[0], va[0] * span, pb.pos[0], vb[0] * span, t);
      s.y = hermite(pa.pos[1], va[1] * span, pb.pos[1], vb[1] * span, t);
      s.z = hermite(pa.pos[2], va[2] * span, pb.pos[2], vb[2] * span, t);
      s.vel = [lerp(va[0], vb[0], t), lerp(va[1], vb[1], t), lerp(va[2], vb[2], t)];
    } else {
      s.x = lerp(pa.pos[0], pb.pos[0], t);
      s.y = lerp(pa.pos[1], pb.pos[1], t);
      s.z = lerp(pa.pos[2], pb.pos[2], t);
    }
    s.yaw = lerpAngle(pa.yaw, pb.yaw, t);
    const anim = blendAnim(cleanAnim(pa.anim), cleanAnim(pb.anim), a.tick, b.tick, tick);
    if (anim) s.anim = anim; else delete s.anim;
    const aa = cleanAim(pa.aim), ab = cleanAim(pb.aim);
    if (aa && ab) s.aim = nlerpAim(aa, ab, t);
    const sa = cleanSpeed(pa.speed), sb = cleanSpeed(pb.speed);
    if (sa !== null && sb !== null) s.speed = lerp(sa, sb, t);
    out.push(s);
  }
  if (t >= 0.5) for (const pb of next.values()) out.push(fromFramePlayer(pb));
  return out;
}

/** Whether a frame player is nowhere: exactly at the origin facing +x,
 *  what older senders wrote for a player whose unit they could not read
 *  (or a dead one who never stood anywhere). Never drawn there. */
export function unplaced(p: FramePlayer): boolean {
  return p.pos[0] === 0 && p.pos[1] === 0 && p.pos[2] === 0 && p.yaw === 0;
}

/** `f` without its unplaced players (`f` itself when it has none). */
export function withoutUnplaced(f: Frame): Frame {
  return f.players.some(unplaced) ? { ...f, players: f.players.filter((p) => !unplaced(p)) } : f;
}

/** Index of the last frame with tick <= `tick` in a tick-sorted array, or -1. */
export function findFrame(frames: readonly { tick: number }[], tick: number): number {
  let lo = 0, hi = frames.length - 1, ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (frames[mid]!.tick <= tick) { ans = mid; lo = mid + 1; } else hi = mid - 1;
  }
  return ans;
}

/** Whether the frame at or before `tick` is a cutscene's (`cinematic`):
 *  the game sent no one in it, and nothing from the frames either side is
 *  drawn into it. */
export function cinematicAt(frames: readonly Frame[], tick: number): boolean {
  const i = findFrame(frames, tick);
  return i >= 0 && frames[i]!.cinematic === true;
}

/** The frame after `i` unless it is a cutscene's: units are not eased
 *  towards a cutscene, nor do the ones after it show up early. */
function nextOf(frames: readonly Frame[], i: number): Frame | undefined {
  const b = frames[i + 1];
  return b?.cinematic || b?.cut != null ? undefined : b;
}

/** Samples a sorted frame list at a (fractional) tick. */
export function sampleFrames(frames: readonly Frame[], tick: number): SampledPlayer[] {
  if (frames.length === 0) return [];
  const i = findFrame(frames, tick);
  if (i < 0) return frames[0]!.cinematic ? [] : frames[0]!.players.map((p) => fromFramePlayer(p));
  if (frames[i]!.cinematic) return [];
  return interpolateFrames(frames[i]!, nextOf(frames, i), tick);
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
      vel: a.vel, fire: a.fire, anim: a.anim, seat: a.seat,
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
  if (i < 0 || frames[i]!.cinematic) return [];
  const fa = frames[i]!, fb = nextOf(frames, i);
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
  const out: SampledVehicle = { id, kind: kind ?? "vehicle", x: v.pos[0], y: v.pos[1], z: v.pos[2], forward, up: cleanAim(v.up) ?? [0, 0, 1], occupied: v.occupied !== false };
  const parent = cleanId(v.parent), aim = cleanAim(v.aim);
  if (parent && parent !== id) out.parent = parent;
  if (aim) out.aim = aim;
  // (Optional on the wire from packed v6; read whatever the frame's type says.)
  const vel = cleanVel((v as { vel?: unknown }).vel);
  if (vel) out.vel = vel;
  return out;
}

/** Puts each turret where the vehicle it is mounted on is (when that one
 *  is in the list): a new list, the turrets new objects (the others may be
 *  a frame's cached ones, never changed). */
function seatTurrets(list: readonly SampledVehicle[]): SampledVehicle[] {
  if (!list.some((v) => v.parent)) return list.slice();
  const byId = new Map(list.map((v) => [v.id, v]));
  return list.map((v) => {
    const p = v.parent ? byId.get(v.parent) : undefined;
    return p && !p.parent ? { ...v, x: p.x, y: p.y, z: p.z, forward: p.forward, up: p.up } : v;
  });
}

/** A frame's vehicles as sampled, in order, and the last of each id: made
 *  once a frame (frames do not change), shared by every repaint. */
function vehiclesOf(f: Frame): { list: SampledVehicle[]; byId: Map<string, SampledVehicle> } {
  const list: SampledVehicle[] = [], byId = new Map<string, SampledVehicle>();
  for (const raw of f.vehicles ?? []) {
    const v = fromFrameVehicle(raw);
    if (v) { list.push(v); byId.set(v.id, v); }
  }
  return { list, byId };
}
const vehicleCache = new WeakMap<Frame, ReturnType<typeof vehiclesOf>>();
function vehiclesCached(f: Frame) {
  let c = vehicleCache.get(f);
  if (!c) { c = vehiclesOf(f); vehicleCache.set(f, c); }
  return c;
}

/** Where a vehicle is between frames `i` (`a`) and `i + 1` (`b`), `t` of
 *  the way. With the game's velocities in both (packed v6), a Hermite
 *  curve by them, as players have. Without, recorded frames come a few a
 *  second, and straight lines between them turn a car's curve into a
 *  polygon and its speed into steps. So the path is a Catmull-Rom curve:
 *  at each end the velocity is the one the frames on either side of it
 *  give (the vehicle in the frame before `a`, and after `b`), else the
 *  segment's own. Straight when the gap is longer than
 *  [`HERMITE_MAX_TICKS`], or a neighbour is a jump away (a respawn). */
export function vehiclePath(frames: readonly Frame[], i: number, a: SampledVehicle, b: SampledVehicle, t: number): [number, number, number] {
  const fa = frames[i]!, fb = frames[i + 1]!;
  const span = fb.tick - fa.tick;
  if (!(span > 0) || span > HERMITE_MAX_TICKS) return [lerp(a.x, b.x, t), lerp(a.y, b.y, t), lerp(a.z, b.z, t)];
  const near = (f: Frame | undefined, from: SampledVehicle, ticks: number): SampledVehicle | null => {
    if (!f || f.cinematic || !(ticks > 0) || ticks > HERMITE_MAX_TICKS) return null;
    const v = vehiclesCached(f).byId.get(a.id);
    if (!v || (v.parent ?? null) !== (a.parent ?? null)) return null;
    return (v.x - from.x) ** 2 + (v.y - from.y) ** 2 + (v.z - from.z) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE ? null : v;
  };
  // The game's own velocities, when both frames carry them (packed v6):
  // the curve the players' paths take (`hermite`).
  if (a.vel && b.vel) {
    const k = span / TICKS_PER_SECOND;
    return [
      hermite(a.x, a.vel[0] * k, b.x, b.vel[0] * k, t),
      hermite(a.y, a.vel[1] * k, b.y, b.vel[1] * k, t),
      hermite(a.z, a.vel[2] * k, b.z, b.vel[2] * k, t),
    ];
  }
  const fp = i > 0 && fa.cut == null ? frames[i - 1] : undefined, fn = nextOf(frames, i + 1);
  const p = fp ? near(fp, a, fa.tick - fp.tick) : null;
  const n = fn ? near(fn, b, fn.tick - fb.tick) : null;
  // Velocities per tick at a and b.
  const va = p ? [(b.x - p.x) / (fb.tick - fp!.tick), (b.y - p.y) / (fb.tick - fp!.tick), (b.z - p.z) / (fb.tick - fp!.tick)] : [(b.x - a.x) / span, (b.y - a.y) / span, (b.z - a.z) / span];
  const vb = n ? [(n.x - a.x) / (fn!.tick - fa.tick), (n.y - a.y) / (fn!.tick - fa.tick), (n.z - a.z) / (fn!.tick - fa.tick)] : [(b.x - a.x) / span, (b.y - a.y) / span, (b.z - a.z) / span];
  return [
    hermite(a.x, va[0]! * span, b.x, vb[0]! * span, t),
    hermite(a.y, va[1]! * span, b.y, vb[1]! * span, t),
    hermite(a.z, va[2]! * span, b.z, vb[2]! * span, t),
  ];
}

/** The vehicles at a (fractional) tick. One in only the earlier frame
 *  stays until the later frame is the nearer; one only in the later
 *  appears then. A big jump snaps. */
export function sampleVehicles(frames: readonly Frame[], tick: number): SampledVehicle[] {
  const i = findFrame(frames, tick);
  if (i < 0 || frames[i]!.cinematic) return [];
  const fa = frames[i]!, fb = nextOf(frames, i);
  const from = vehiclesCached(fa).list;
  if (!fb || fb.tick <= fa.tick) return seatTurrets(from);
  const t = Math.min(1, Math.max(0, (tick - fa.tick) / (fb.tick - fa.tick)));
  const next = vehiclesCached(fb).byId;
  // The later frame's vehicles the earlier one has too.
  const matched = new Set<string>();
  const out: SampledVehicle[] = [];
  for (const a of from) {
    const b = matched.has(a.id) ? undefined : next.get(a.id);
    matched.add(a.id);
    if (!b) { if (t < 0.5) out.push(a); continue; }
    if ((b.x - a.x) ** 2 + (b.y - a.y) ** 2 + (b.z - a.z) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE) { out.push(t < 0.5 ? a : b); continue; }
    const [x, y, z] = vehiclePath(frames, i, a, b, t);
    const v: SampledVehicle = {
      id: a.id, kind: t < 0.5 ? a.kind : b.kind,
      x, y, z,
      forward: nlerpAim(a.forward, b.forward, t), up: nlerpAim(a.up, b.up, t),
      occupied: t < 0.5 ? a.occupied : b.occupied,
    };
    const parent = t < 0.5 ? a.parent : b.parent;
    if (parent) v.parent = parent;
    const aim = a.aim && b.aim ? nlerpAim(a.aim, b.aim, t) : t < 0.5 ? a.aim : b.aim;
    if (aim) v.aim = aim;
    if (a.vel && b.vel) v.vel = [lerp(a.vel[0], b.vel[0], t), lerp(a.vel[1], b.vel[1], t), lerp(a.vel[2], b.vel[2], t)];
    out.push(v);
  }
  if (t >= 0.5) for (const [id, b] of next) if (!matched.has(id)) out.push(b);
  return seatTurrets(out);
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

/** A frame's machines as sampled (at most [`MAX_MACHINES`] read), made
 *  once a frame and shared, as vehicles are. */
function machinesOf(f: Frame): SampledMachine[] {
  const out: SampledMachine[] = [];
  for (const raw of (f.machines ?? []).slice(0, MAX_MACHINES)) { const m = fromFrameMachine(raw); if (m) out.push(m); }
  return out;
}
const machineCache = new WeakMap<Frame, SampledMachine[]>();
function machinesCached(f: Frame): SampledMachine[] {
  let c = machineCache.get(f);
  if (!c) { c = machinesOf(f); machineCache.set(f, c); }
  return c;
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
    if (!frames[j]!.machines) continue;
    for (const m of machinesCached(frames[j]!)) if (!at.has(m.id)) at.set(m.id, { m, tick: frames[j]!.tick });
  }
  const fb = frames[i + 1];
  const next = new Map<string, SampledMachine>();
  if (fb && fb.tick > frames[i]!.tick) for (const m of machinesCached(fb)) next.set(m.id, m);
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

/** The animation endpoints at the render tick. A looping rig can blend
 *  across the seam using the same endpoints. */
export interface SampledMachinePosition {
  from: number;
  to: number;
  mix: number;
}

export const MAX_MACHINE_POSITIONS = 255;

interface TravelSample {
  tick: number;
  position: number;
}

/** Only the received changes are stored. An unchanged device holds its
 *  position through quiet frames, including when a replay seeks there. */
export class MachinePositionHistory {
  private readonly positions = new Map<number, TravelSample[]>();
  private readonly resets: { tick: number }[] = [];

  push(frame: Frame, tick = frame.tick, rewind = false): void {
    if (this.positions.size && (rewind || frame.cut != null || isMachineBaseline(frame))) {
      this.resets.push({ tick });
    }
    if (!Array.isArray(frame.machine_positions)) return;
    const reset = this.resets.at(-1)?.tick ?? -Infinity;
    for (let n = 0; n < Math.min(frame.machine_positions.length, MAX_MACHINE_POSITIONS); n++) {
      const m = frame.machine_positions[n];
      if (!m || !Number.isInteger(m.i) || m.i < 0 || m.i > 0x7fff ||
          !Number.isInteger(m.p) || m.p < 0 || m.p > 255) continue;
      let samples = this.positions.get(m.i);
      if (!samples) { samples = []; this.positions.set(m.i, samples); }
      const position = m.p / 255;
      const last = samples.at(-1);
      if (last?.tick === tick) last.position = position;
      else if (!last || last.tick < reset || last.position !== position) samples.push({ tick, position });
    }
  }

  /** Live only: keep the last word before the buffer's first frame. */
  prune(tick: number): void {
    const resetAt = findFrame(this.resets, tick);
    const reset = this.resets[resetAt]?.tick ?? -Infinity;
    if (resetAt > 0) this.resets.splice(0, resetAt);
    for (const [i, samples] of this.positions) {
      const last = findFrame(samples, tick);
      if (last > 0) samples.splice(0, last);
      if (samples[0]!.tick < reset) samples.shift();
      if (!samples.length) this.positions.delete(i);
    }
  }

  sample(frames: readonly Frame[], tick: number): ReadonlyMap<number, SampledMachinePosition> | undefined {
    if (!this.positions.size) return;
    const at = findFrame(frames, tick);
    if (at < 0 || frames[at]!.cinematic) return;
    const reset = this.resets[findFrame(this.resets, tick)]?.tick ?? -Infinity;
    const before = frames[at]!.tick;
    const next = frames[at + 1];
    const blend = next && next.cut == null && !next.cinematic && next.tick > before;
    const mix = blend ? (tick - before) / (next.tick - before) : 0;
    const out = new Map<number, SampledMachinePosition>();
    for (const [i, samples] of this.positions) {
      const index = findFrame(samples, tick);
      const a = samples[index];
      if (!a || a.tick < reset) continue;
      const b = samples[index + 1];
      const follows = blend && b?.tick === next.tick;
      out.set(i, { from: a.position, to: follows ? b.position : a.position, mix: follows ? mix : 0 });
    }
    return out.size ? out : undefined;
  }
}

/** Most scenario objects a frame is read for. */
export const MAX_OBJECTS = 2048;

const finite3 = (v: unknown): v is [number, number, number] =>
  Array.isArray(v) && v.length === 3 && v.every((c) => typeof c === "number" && Number.isFinite(c));

/** A frame's scenario objects (at most [`MAX_OBJECTS`]), each one's
 *  `kind:index` key, and the ones there (not gone) by key: made once a
 *  frame. */
function objectsOf(f: Frame): { list: FrameObject[]; keys: string[]; there: Map<string, FrameObject> } {
  const list = (f.objects ?? []).slice(0, MAX_OBJECTS);
  const keys = list.map((o) => `${o.k}:${o.i}`);
  const there = new Map<string, FrameObject>();
  list.forEach((o, n) => { if (!o.gone) there.set(keys[n]!, o); });
  return { list, keys, there };
}
const objectCache = new WeakMap<Frame, ReturnType<typeof objectsOf>>();
function objectsCached(f: Frame) {
  let c = objectCache.get(f);
  if (!c) { c = objectsOf(f); objectCache.set(f, c); }
  return c;
}

/** Scenario objects not where the scenario put them at `tick`: the frame
 *  behind it says which (moved, or gone), and a moved one eases towards
 *  where the next frame has it. Null with no frame behind `tick`. */
export function sampleObjects(frames: readonly Frame[], tick: number): FrameObject[] | null {
  const i = findFrame(frames, tick);
  if (i < 0) return null;
  const a = frames[i]!, b = nextOf(frames, i);
  const { list, keys } = objectsCached(a);
  const next = b && b.tick > a.tick ? objectsCached(b).there : undefined;
  const t = b && b.tick > a.tick ? Math.min(1, Math.max(0, (tick - a.tick) / (b.tick - a.tick))) : 0;
  const out: FrameObject[] = [];
  for (let n = 0; n < list.length; n++) {
    const o = list[n]!;
    if (typeof o.k !== "string" || typeof o.i !== "number") continue;
    if (o.gone) { out.push({ k: o.k, i: o.i, gone: true }); continue; }
    if (!finite3(o.p)) continue;
    const f = finite3(o.f) ? o.f : [1, 0, 0] as [number, number, number];
    const u = finite3(o.u) ? o.u : [0, 0, 1] as [number, number, number];
    const m = next?.get(keys[n]!);
    if (!m || !finite3(m.p) || t === 0 || (m.p[0] - o.p[0]) ** 2 + (m.p[1] - o.p[1]) ** 2 + (m.p[2] - o.p[2]) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE) {
      out.push({ k: o.k, i: o.i, p: [o.p[0], o.p[1], o.p[2]], f, u });
      continue;
    }
    out.push({
      k: o.k, i: o.i,
      p: [lerp(o.p[0], m.p[0], t), lerp(o.p[1], m.p[1], t), lerp(o.p[2], m.p[2], t)],
      f: finite3(m.f) ? nlerpAim(f, m.f, t) : f,
      u: finite3(m.u) ? nlerpAim(u, m.u, t) : u,
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
  private travel = new MachinePositionHistory();
  private historyReaders = 0;

  constructor(readonly capacity = 48, readonly delayTicks = 5) {}

  /** Keep about 30 seconds at the live stream's maximum of 15 fps while
   *  a viewer has a scrubber open. Other games keep their shorter buffer. */
  keepPlaybackHistory(): () => void {
    this.historyReaders++;
    return () => {
      this.historyReaders--;
      this.trim();
    };
  }

  private trim(): void {
    const capacity = this.historyReaders ? Math.max(450, this.capacity) : this.capacity;
    if (this.frames.length > capacity) {
      this.frames.splice(0, this.frames.length - capacity);
      this.travel.prune(this.frames[0]?.tick ?? this.lastTick);
    }
  }

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
        this.travel = new MachinePositionHistory();
        this.offset = null;
      } else return;
    }
    this.travel.push(frame);
    frame = withoutUnplaced(frame);
    this.frames.push(frame);
    // Machines are only sent when they move: keep the last word on each.
    for (const raw of (frame.machines ?? []).slice(0, MAX_MACHINES)) {
      const m = fromFrameMachine(raw);
      if (m && (this.machines.has(m.id) || this.machines.size < MAX_MACHINES)) this.machines.set(m.id, m);
    }
    this.trim();
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

  /** A cutscene is playing at the render tick. */
  cinematic(now: number): boolean {
    return cinematicAt(this.frames, this.renderTick(now));
  }

  /** Every machine: the ones moving now where the render tick has them, the rest where they were last sent. */
  sampleMachines(now: number): SampledMachine[] {
    if (!this.machines.size) return [];
    const out = new Map(this.machines);
    for (const m of sampleMachines(this.frames, this.renderTick(now))) out.set(m.id, m);
    return [...out.values()];
  }

  sampleMachinePositions(now: number): ReadonlyMap<number, SampledMachinePosition> | undefined {
    return this.travel.sample(this.frames, this.renderTick(now));
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

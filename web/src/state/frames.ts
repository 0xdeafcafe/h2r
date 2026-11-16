// Frame buffering and interpolation. Frames are not reactive state: they
// arrive at up to 15 Hz per game and are read directly by the canvas loop.

import type { Frame, FramePlayer } from "../api/types";

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
}

export type Aim = [number, number, number];

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
  return out;
}

/** Blends two frames at `tick`. Players missing from `b` hold their
 *  position from `a`; players only in `b` appear when t >= 0.5. */
export function interpolateFrames(a: Frame, b: Frame | undefined, tick: number): SampledPlayer[] {
  if (!b || b.tick <= a.tick) return a.players.map((p) => fromFramePlayer(p, a.tick));
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
        this.offset = null;
      } else return;
    }
    this.frames.push(frame);
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

// Shared replay state between the game page and a popped-out 3D window:
// both run their own ReplayClock over the same recording and keep them in
// step by sending each other the clock's state. Also the smoothing the 3D
// view lays over sampled player positions. Pure, so it can be tested.

import type { ReplayClock } from "../state/replay";
import { aimOf, lerpAngle, nlerpAim, SNAP_DISTANCE, TICKS_PER_SECOND, type Aim, type SampledPlayer, type Stance } from "../state/frames";

/** A replay clock's state at one moment, as it travels between windows. */
export interface ClockState {
  tick: number;
  playing: boolean;
  speed: number;
}

export function readClock(c: ReplayClock, now: number): ClockState {
  const tick = c.tickAt(now); // may pause the clock at the end, so read it first
  return { tick, playing: c.playing, speed: c.speed };
}

/** Puts a clock into `s` as of `now`. */
export function applyClock(c: ReplayClock, s: ClockState, now: number): void {
  if (c.playing) c.pause(now);
  if (Number.isFinite(s.speed) && s.speed > 0) c.setSpeed(s.speed, now);
  c.seek(s.tick, now);
  // `play` from the very end starts over; a sender that is playing is never there.
  if (s.playing && c.timeline.clamp(s.tick) < c.timeline.endTick) c.play(now);
}

/** Whether a clock already agrees with `s` closely enough to leave alone
 *  (a little drift while both play is not worth a visible jump). */
export function clockAgrees(c: ReplayClock, s: ClockState, now: number, slackTicks = TICKS_PER_SECOND / 2): boolean {
  const mine = readClock(c, now);
  if (mine.playing !== s.playing || mine.speed !== s.speed) return false;
  return Math.abs(mine.tick - s.tick) <= (s.playing ? slackTicks * Math.max(1, s.speed) : 0.5);
}

/** A camera the replay asks its 3D views for (a killcam): a mode and the
 *  player it is on. `seq` grows with each request, so the same request
 *  twice is still two. */
export interface CameraRequest {
  mode: CameraMode;
  follow: number;
  seq: number;
  /** Wall time (ms since the epoch) it was made: a 3D view that opens for
   *  it takes it only while it is fresh. */
  at: number;
}

/** How long a camera request stays fresh for a view that opens after it. */
export const CAMERA_FRESH_MS = 3000;
export const CAMERA_MODES = ["follow", "eyes"] as const;
export type CameraMode = (typeof CAMERA_MODES)[number];

export function nextCamera(last: CameraRequest | null, mode: CameraMode, follow: number, at = Date.now()): CameraRequest {
  return { mode, follow, seq: (last?.seq ?? 0) + 1, at };
}

/** Whether a view opening at `now` should take `c`. */
export function cameraFresh(c: CameraRequest | null, now = Date.now()): c is CameraRequest {
  return !!c && now - c.at >= 0 && now - c.at < CAMERA_FRESH_MS;
}

export type SyncMessage =
  | { kind: "state"; from: string; state: ClockState }
  | { kind: "hello"; from: string }
  | { kind: "camera"; from: string; camera: CameraRequest };

export function isSyncMessage(v: unknown): v is SyncMessage {
  if (!v || typeof v !== "object") return false;
  const m = v as { kind?: unknown; from?: unknown; state?: Partial<ClockState>; camera?: Partial<CameraRequest> };
  if (typeof m.from !== "string") return false;
  if (m.kind === "hello") return true;
  if (m.kind === "camera") {
    const c = m.camera;
    return !!c && (CAMERA_MODES as readonly unknown[]).includes(c.mode) && Number.isInteger(c.follow) && Number.isInteger(c.seq) && Number.isFinite(c.at);
  }
  return m.kind === "state" && !!m.state && Number.isFinite(m.state.tick) && typeof m.state.playing === "boolean" && Number.isFinite(m.state.speed);
}

export const syncChannelName = (gameId: string) => "h2live.replay." + gameId;

// ---- smoothing ----

export interface SmoothPlayer {
  x: number; y: number; z: number; yaw: number; alive: boolean;
  /** Where they are looking, eased like the rest: the frame's aim, or
   *  level along the yaw when it has none. Unit length. */
  aim: Aim;
  species: string | null;
  weapon: string | null;
  /** The vehicle they are in (its id), or null on foot. */
  vehicle: string | null;
  /** Set for an AI-controlled biped: its id and side. */
  actor: { id: string; team: string | null } | null;
  /** How they are moving and how fast, when the frame says. */
  stance: Stance | null;
  speed: number | null;
}

/** Eases shown players towards the sampled ones. Sampling already blends
 *  recorded frames linearly; this rounds the corners where one second's
 *  heading turns into the next, and settles live updates. A big jump (a
 *  respawn, a teleport, a seek) or `snap` moves at once. `rate` is per
 *  second. Players no longer sampled are dropped. */
export function smoothPlayers(
  shown: Map<number, SmoothPlayer>, sampled: readonly SampledPlayer[], dtMs: number,
  opts: { rate?: number; snap?: boolean } = {},
): void {
  const t = opts.snap ? 1 : 1 - Math.exp(-(opts.rate ?? 18) * Math.max(0, dtMs) / 1000);
  const seen = new Set<number>();
  for (const p of sampled) {
    seen.add(p.index);
    const s = shown.get(p.index);
    const aim = aimOf(p), species = p.species ?? null, weapon = p.weapon ?? null;
    const vehicle = p.vehicle ?? null, actor = p.actor ?? null;
    const stance = p.stance ?? null, speed = p.speed ?? null;
    if (!s) { shown.set(p.index, { x: p.x, y: p.y, z: p.z, yaw: p.yaw, alive: p.alive, aim, species, weapon, vehicle, actor, stance, speed }); continue; }
    const far = (p.x - s.x) ** 2 + (p.y - s.y) ** 2 + (p.z - s.z) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE;
    const k = far || !s.alive || !p.alive ? 1 : t;
    s.x += (p.x - s.x) * k;
    s.y += (p.y - s.y) * k;
    s.z += (p.z - s.z) * k;
    s.yaw = lerpAngle(s.yaw, p.yaw, k);
    s.aim = k >= 1 ? aim : nlerpAim(s.aim, aim, k);
    s.alive = p.alive;
    s.species = species;
    s.weapon = weapon;
    s.vehicle = vehicle;
    s.actor = actor;
    s.stance = stance;
    s.speed = speed;
  }
  for (const index of shown.keys()) if (!seen.has(index)) shown.delete(index);
}

/** Eases shown vehicles towards the sampled ones, the way players are:
 *  place and both axes glide, a big jump or `snap` moves at once, and a
 *  vehicle no longer sampled (everyone got out) is dropped. */
export function smoothVehicles<T extends { id: string; x: number; y: number; z: number; forward: Aim; up: Aim }>(
  shown: Map<string, T>, sampled: readonly T[], dtMs: number,
  opts: { rate?: number; snap?: boolean } = {},
): void {
  const t = opts.snap ? 1 : 1 - Math.exp(-(opts.rate ?? 18) * Math.max(0, dtMs) / 1000);
  const seen = new Set<string>();
  for (const v of sampled) {
    seen.add(v.id);
    const s = shown.get(v.id);
    if (!s) { shown.set(v.id, { ...v }); continue; }
    const far = (v.x - s.x) ** 2 + (v.y - s.y) ** 2 + (v.z - s.z) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE;
    const k = far ? 1 : t;
    s.x += (v.x - s.x) * k;
    s.y += (v.y - s.y) * k;
    s.z += (v.z - s.z) * k;
    const forward = k >= 1 ? v.forward : nlerpAim(s.forward, v.forward, k);
    const up = k >= 1 ? v.up : nlerpAim(s.up, v.up, k);
    // Everything else (its kind, whether anyone is in it) is simply the newest.
    Object.assign(s, v, { x: s.x, y: s.y, z: s.z, forward, up });
  }
  for (const id of shown.keys()) if (!seen.has(id)) shown.delete(id);
}

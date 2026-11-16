// Shared replay state between the game page and a popped-out 3D window:
// both run their own ReplayClock over the same recording and keep them in
// step by sending each other the clock's state. Also the smoothing the 3D
// view lays over sampled player positions. Pure, so it can be tested.

import type { ReplayClock } from "../state/replay";
import { lerpAngle, SNAP_DISTANCE, TICKS_PER_SECOND, type SampledPlayer } from "../state/frames";

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

export type SyncMessage =
  | { kind: "state"; from: string; state: ClockState }
  | { kind: "hello"; from: string };

export function isSyncMessage(v: unknown): v is SyncMessage {
  if (!v || typeof v !== "object") return false;
  const m = v as { kind?: unknown; from?: unknown; state?: Partial<ClockState> };
  if (typeof m.from !== "string") return false;
  if (m.kind === "hello") return true;
  return m.kind === "state" && !!m.state && Number.isFinite(m.state.tick) && typeof m.state.playing === "boolean" && Number.isFinite(m.state.speed);
}

export const syncChannelName = (gameId: string) => "h2live.replay." + gameId;

// ---- smoothing ----

export interface SmoothPlayer { x: number; y: number; z: number; yaw: number; alive: boolean }

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
    if (!s) { shown.set(p.index, { x: p.x, y: p.y, z: p.z, yaw: p.yaw, alive: p.alive }); continue; }
    const far = (p.x - s.x) ** 2 + (p.y - s.y) ** 2 + (p.z - s.z) ** 2 > SNAP_DISTANCE * SNAP_DISTANCE;
    const k = far || !s.alive || !p.alive ? 1 : t;
    s.x += (p.x - s.x) * k;
    s.y += (p.y - s.y) * k;
    s.z += (p.z - s.z) * k;
    s.yaw = lerpAngle(s.yaw, p.yaw, k);
    s.alive = p.alive;
  }
  for (const index of shown.keys()) if (!seen.has(index)) shown.delete(index);
}

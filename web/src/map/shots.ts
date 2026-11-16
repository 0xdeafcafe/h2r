// Shots as the views draw them. A frame says how many rounds a player fired
// since their last frame; a view watches the sampled players go by, turns
// each new burst into a few short-lived shots (a flash and a tracer each)
// and fades them out. Pure: the 2D maps and the 3D scene each keep a queue
// and draw its shots their own way.

import { aimOf, type Aim, type SampledPlayer } from "../state/frames";

/** How long a shot stays on screen. */
export const SHOT_FADE_MS = 150;
/** A frame's shots are drawn one after another, this far apart. */
export const SHOT_GAP_MS = 40;
/** At most this many tracers for one frame's shots, however many it counted. */
export const MAX_SHOTS_PER_BURST = 3;
/** At most this many shots alive at once, over all players. */
export const MAX_LIVE_SHOTS = 32;
/** With reduced motion nothing flashes: the aim ray shows "firing" instead,
 *  and holds it this long after the last shot so it doesn't flicker. */
export const FIRING_HOLD_MS = 600;

export interface Shot {
  id: number;
  /** The player who fired. */
  index: number;
  /** Wall time it appears. */
  born: number;
  /** Where the player was and was looking when the burst was seen. */
  x: number; y: number; z: number;
  aim: Aim;
}

export interface LiveShot extends Shot {
  /** Milliseconds since it appeared. */
  age: number;
  /** 1 when it appears, 0 when it is gone. */
  alpha: number;
}

export class ShotQueue {
  private shots: Shot[] = [];
  /** The burst last seen per player: its frame tick. */
  private seen = new Map<number, number>();
  private fired = new Map<number, number>();
  private nextId = 1;

  /** Looks at this moment's players and queues any burst not seen before.
   *  `quiet` (a seek, a jump) notes the bursts without drawing them. */
  observe(players: readonly SampledPlayer[], now: number, opts: { quiet?: boolean } = {}): number[] {
    const firedNow: number[] = [];
    for (const p of players) {
      if (!p.shots || p.shotTick === undefined) continue;
      if (this.seen.get(p.index) === p.shotTick) continue;
      this.seen.set(p.index, p.shotTick);
      if (opts.quiet || !p.alive) continue;
      const n = Math.min(MAX_SHOTS_PER_BURST, p.shots);
      const aim = aimOf(p);
      for (let i = 0; i < n; i++) {
        this.shots.push({ id: this.nextId++, index: p.index, born: now + i * SHOT_GAP_MS, x: p.x, y: p.y, z: p.z, aim });
      }
      this.fired.set(p.index, now + (n - 1) * SHOT_GAP_MS);
      firedNow.push(p.index);
    }
    // Over the cap, the oldest go first.
    if (this.shots.length > MAX_LIVE_SHOTS) this.shots.splice(0, this.shots.length - MAX_LIVE_SHOTS);
    // Who started a burst just now (for the fire animation).
    return firedNow;
  }

  /** The shots on screen at `now`, oldest first; spent ones are dropped. */
  live(now: number): LiveShot[] {
    this.shots = this.shots.filter((s) => now - s.born < SHOT_FADE_MS);
    const out: LiveShot[] = [];
    for (const s of this.shots) {
      const age = now - s.born;
      if (age < 0) continue;
      out.push({ ...s, age, alpha: shotAlpha(age) });
    }
    return out;
  }

  /** Whether anything is on screen or about to be. */
  get busy(): boolean { return this.shots.length > 0; }

  /** Whether a player fired within the hold: the steady "firing" state. */
  firing(index: number, now: number): boolean {
    const t = this.fired.get(index);
    return t !== undefined && now - t < FIRING_HOLD_MS;
  }

  /** The players in the steady "firing" state at `now`. */
  firingNow(now: number): Set<number> {
    const out = new Set<number>();
    for (const [index, t] of this.fired) {
      if (now - t >= FIRING_HOLD_MS) this.fired.delete(index);
      else out.add(index);
    }
    return out;
  }

  clear(): void {
    this.shots = [];
    this.seen.clear();
    this.fired.clear();
  }
}

/** A shot's brightness `age` ms after it appeared: full at first, gone by
 *  SHOT_FADE_MS, easing out. */
export function shotAlpha(age: number): number {
  if (age < 0 || age >= SHOT_FADE_MS) return 0;
  const u = 1 - age / SHOT_FADE_MS;
  return u * u;
}

/** The muzzle flash is the first part of a shot's life only. */
export const FLASH_MS = 60;
export function flashAlpha(age: number): number {
  return age < 0 || age >= FLASH_MS ? 0 : 1 - age / FLASH_MS;
}

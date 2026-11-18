// Feet on the floor. A biped's place is its origin, which the game keeps
// on the ground while it stands, walks or crouches. Between two samples the
// viewer only has a curve through the samples, and with a sample a second
// (recorded games) or without velocities (older senders) that curve leaves
// the ground: over a crest it passes above it, down stairs or a dip it
// hangs in the air, and a body eased towards the sample trails behind it
// going downhill. All of that reads as floating. So a biped the game says
// is on its feet is put on the level's surface under it, when there is one
// near where the samples have it; in the air, dead, or riding, it stays
// where the samples put it. Pure but for the probe, so it can be tested.

import type { Stance } from "../state/frames";

/** How far above the sampled place a floor may be (a curve cutting
 *  through a crest or a stair's nose), game units. */
export const GROUND_UP = 0.35;
/** How far below it (a curve hanging over a dip or down stairs). Further
 *  is a drop the samples don't show: left where they put it. */
export const GROUND_DOWN = 0.6;
/** A biped moved less than this (game units) since its floor was probed
 *  stands on the same surface: its plane gives the height. */
export const GROUND_STEP = 0.08;
/** At most this many probes in one update; the rest use the surface they
 *  were last over until the next. */
export const GROUND_BUDGET = 16;

/** Whether the game has this biped on its feet: alive, out of vehicles,
 *  and standing or crouching. A sender that says no stance isn't guessed
 *  at (it may be mid-jump). */
export function onFoot(s: { alive: boolean; stance: Stance | null | undefined; vehicle: string | null | undefined }): boolean {
  return s.alive && !s.vehicle && (s.stance === "stand" || s.stance === "crouch");
}

/** Where a standing biped's feet go: the floor, when it is within reach
 *  of the sampled height, else the sampled height. */
export function footZ(z: number, floor: number | null): number {
  return floor !== null && floor <= z + GROUND_UP && floor >= z - GROUND_DOWN ? floor : z;
}

/** A surface found by a probe: a point on it and its unit normal, game coordinates. */
export interface Floor { x: number; y: number; z: number; nx: number; ny: number; nz: number }

/** The surface's height at (x, y): on its plane, or the point's own height
 *  when the plane is too steep to stand on. */
export function floorAt(f: Floor, x: number, y: number): number {
  if (f.nz < 0.3) return f.z;
  return f.z - (f.nx * (x - f.x) + f.ny * (y - f.y)) / f.nz;
}

/** Finds the first surface facing up below (x, y, z) within `reach`
 *  game units; null for none. */
export type FloorProbe = (x: number, y: number, z: number, reach: number) => Floor | null;

/** Each biped's floor: probed again when it has moved, kept otherwise (a
 *  level doesn't move under a replay; machines aren't probed). `begin`
 *  each update renews the probe budget. */
export class GroundCache {
  private last = new Map<number, { x: number; y: number; z: number; floor: Floor | null }>();
  private budget = GROUND_BUDGET;

  constructor(private probe: FloorProbe) {}

  begin(): void {
    this.budget = GROUND_BUDGET;
  }

  /** The floor height under a biped (`key` names it) near (x, y, z), or null. */
  floor(key: number, x: number, y: number, z: number): number | null {
    const was = this.last.get(key);
    const near = was && Math.hypot(x - was.x, y - was.y) < GROUND_STEP && Math.abs(z - was.z) < GROUND_UP / 2;
    if (was && (near || this.budget <= 0)) {
      if (!was.floor) return null;
      return floorAt(was.floor, x, y);
    }
    if (this.budget <= 0) return null;
    this.budget--;
    const floor = this.probe(x, y, z + GROUND_UP, GROUND_UP + GROUND_DOWN);
    this.last.set(key, { x, y, z, floor });
    return floor ? floorAt(floor, x, y) : null;
  }

  /** Where a biped's feet go this update (see [`footZ`]). */
  place(key: number, s: { x: number; y: number; z: number; alive: boolean; stance: Stance | null | undefined; vehicle: string | null | undefined }): number {
    return onFoot(s) ? footZ(s.z, this.floor(key, s.x, s.y, s.z)) : s.z;
  }

  /** Forgets bipeds no longer shown. */
  keep(keys: { has(k: number): boolean }): void {
    for (const k of this.last.keys()) if (!keys.has(k)) this.last.delete(k);
  }
}

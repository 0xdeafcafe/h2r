// Vehicle turrets, the parts that need no three.js. A turret (the
// Warthog's chaingun, the Scorpion's cannon, the Wraith's mortar) is a
// vehicle of its own in the frames, with `parent` the vehicle it is
// mounted on and `aim` where it points. The vehicle's glb carries it as
// parts (docs/live/live-art.md, "Turrets"): a `turret_<n>` node that turns
// about its own up (yaw) with a `turret_<n>_pitch_<k>` node per gun that
// turns about its own side (pitch); the yaw node's extras name the turret
// tag (`{"turret": "chaingun"}`). viewer/scene.ts poses them.

import type { Aim } from "../state/frames";

/** Furthest a gun is drawn raised or lowered from level (radians): past
 *  what any turret in the game reaches, so they only guard against
 *  nonsense, not the game's own limits. */
export const PITCH_UP = 1.25;
export const PITCH_DOWN = 0.7;

export interface TurretAim { kind: string; aim: Aim }

/** What the frame's turrets say: for each turret and gunner's vehicle id,
 *  the vehicle that is drawn for it (`hullOf`), and for each drawn vehicle
 *  its turrets' aims. A turret's aim is its own, else its gunner's (a
 *  rider whose vehicle is the turret). One whose parent is itself a
 *  turret, or missing from the list, is left out. */
export function mountedTurrets(
  vehicles: Iterable<{ id: string; kind: string; parent?: string; aim?: Aim }>,
  riders: Iterable<{ vehicle: string | null; aim: Aim; alive: boolean }>,
): { hullOf: Map<string, string>; aims: Map<string, TurretAim[]> } {
  const list = [...vehicles];
  const ids = new Map(list.map((v) => [v.id, v]));
  const gunner = new Map<string, Aim>();
  for (const r of riders) if (r.vehicle && r.alive && !gunner.has(r.vehicle)) gunner.set(r.vehicle, r.aim);
  const hullOf = new Map<string, string>();
  const aims = new Map<string, TurretAim[]>();
  for (const v of list) {
    const hull = v.parent ? ids.get(v.parent) : undefined;
    if (!hull || hull.parent) continue;
    hullOf.set(v.id, hull.id);
    const aim = v.aim ?? gunner.get(v.id);
    if (!aim) continue;
    const at = aims.get(hull.id) ?? [];
    at.push({ kind: v.kind, aim });
    aims.set(hull.id, at);
  }
  return { hullOf, aims };
}

/** Which of a model's turrets (by the tag stems their nodes name, in
 *  order) a turret of `kind` is: the one named so, else, when the model
 *  has one turret, that one; -1 when it can't be told. */
export function pickTurret(parts: readonly string[], kind: string): number {
  const i = parts.indexOf(kind);
  if (i >= 0) return i;
  return parts.length === 1 ? 0 : -1;
}

/** The yaw (about the turret's up, from its forward towards its left) and
 *  pitch (up positive, clamped) that point it along `local`, a direction
 *  in the turret's own rest frame, game axes (x forward, y left, z up).
 *  Straight up or down keeps yaw 0. */
export function turretAngles(local: readonly [number, number, number]): { yaw: number; pitch: number } {
  const [x, y, z] = local;
  const flat = Math.hypot(x, y);
  if (![x, y, z].every(Number.isFinite) || flat + Math.abs(z) < 1e-9) return { yaw: 0, pitch: 0 };
  const yaw = flat < 1e-9 ? 0 : Math.atan2(y, x);
  const pitch = Math.max(-PITCH_DOWN, Math.min(PITCH_UP, Math.atan2(z, flat)));
  return { yaw, pitch };
}

/** Turns `from` toward `to` the short way round, frame-rate independent:
 *  after `tau` seconds 63% of the way; no `tau`, all the way. */
export function easeAngle(from: number, to: number, dt: number, tau: number): number {
  const d = Math.atan2(Math.sin(to - from), Math.cos(to - from));
  if (tau <= 0) return to;
  const k = 1 - Math.exp(-Math.max(0, dt) / tau);
  const next = from + d * k;
  return Math.atan2(Math.sin(next), Math.cos(next));
}

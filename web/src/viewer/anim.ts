// Which animation a biped plays: the looping state its frame's stance and
// speed call for, and the one-shot (a grenade throw, a melee, a reload, a
// weapon swap, firing, landing) laid over it. Pure: viewer/players3d.ts
// turns the answer into three.js mixer actions.
//
// A v2 biped glb names its clips `<weapon_class>:<state>`; the manifest
// gives each weapon stem its class, so the clip set follows the held weapon.

import type { Action, Stance } from "../state/frames";

export const LOOPS = ["idle", "walk", "run", "crouch", "airborne", "die"] as const;
export const ONE_SHOTS = ["grenade", "melee", "reload", "switch", "fire", "land"] as const;
export type Loop = (typeof LOOPS)[number];
export type OneShot = (typeof ONE_SHOTS)[number];
export type AnimState = Loop | OneShot;

/** Game units a second: slower than the first is standing, faster than the second is running. */
export const WALK_SPEED = 0.2;
export const RUN_SPEED = 1.4;
/** The pace the walk and run clips are taken to be made for, to match feet to ground. */
export const WALK_PACE = 0.9;
export const RUN_PACE = 2.25;

/** The class a clip set is chosen by: the manifest's for the held weapon,
 *  `rifle` for a weapon it doesn't class, `unarmed` for empty hands. */
export function weaponClass(classes: Readonly<Record<string, string>> | null | undefined, weapon: string | null | undefined): string {
  if (!weapon) return "unarmed";
  return classes?.[weapon] ?? "rifle";
}

/** The looping state for a moment: by stance first, then by speed. A frame
 *  without a stance is standing; without a speed, still. */
export function locomotion(p: { alive: boolean; stance?: Stance | null; speed?: number | null }): Loop {
  if (!p.alive || p.stance === "dead") return "die";
  if (p.stance === "airborne") return "airborne";
  if (p.stance === "vehicle") return "idle";
  if (p.stance === "crouch") return "crouch";
  const v = p.speed ?? 0;
  return v >= RUN_SPEED ? "run" : v >= WALK_SPEED ? "walk" : "idle";
}

/** How fast to play a walk or run clip so the feet keep up with the ground. */
export function paceScale(state: AnimState, speed: number | null | undefined): number {
  if (speed == null || (state !== "walk" && state !== "run")) return 1;
  return Math.min(2, Math.max(0.5, speed / (state === "walk" ? WALK_PACE : RUN_PACE)));
}

const STATE_FALLBACK: Partial<Record<AnimState, AnimState[]>> = {
  run: ["walk", "idle"], walk: ["run", "idle"], crouch: ["idle"], airborne: ["idle"], idle: [],
};

/** The clip to play for a state with the held weapon's class: that class's,
 *  else the rifle's, the unarmed, or any class that has the state; a loop
 *  the glb lacks falls back to a nearer loop (run to walk to idle), a
 *  one-shot it lacks is simply not played (null). */
export function clipFor(clips: Iterable<string>, cls: string, state: AnimState): string | null {
  const have = clips instanceof Set ? (clips as Set<string>) : new Set(clips);
  for (const s of [state, ...(STATE_FALLBACK[state] ?? [])]) {
    for (const c of [cls, "rifle", "unarmed"]) if (have.has(`${c}:${s}`)) return `${c}:${s}`;
    const any = [...have].filter((n) => n.endsWith(":" + s)).sort()[0];
    if (any) return any;
  }
  return null;
}

/** The one-shot an action starts, if any: a jump shows as the airborne
 *  stance, and a pickup has nothing to show. */
export function actionShot(a: Action): OneShot | null {
  return a === "grenade" || a === "melee" || a === "reload" || a === "switch" ? a : null;
}

/** A one-shot holds the body this long when the glb's clip doesn't say (ms). */
export const ONE_SHOT_MS: Record<OneShot, number> = { grenade: 900, melee: 600, reload: 1500, switch: 600, fire: 180, land: 300 };
/** A more important one-shot interrupts a lesser; a lesser waits its turn out. */
const RANK: Record<OneShot, number> = { melee: 4, grenade: 4, switch: 3, reload: 2, land: 1, fire: 0 };

export interface AnimNow {
  loop: Loop;
  /** The one-shot in progress and a number that changes each time one starts. */
  shot: OneShot | null;
  shotId: number;
}

/** One biped's animation state over time. */
export class BipedAnim {
  loop: Loop = "idle";
  private shot: { state: OneShot; until: number } | null = null;
  private shotId = 0;

  /** Starts a one-shot unless a more important one is still running.
   *  `ms` is how long it lasts (the clip's length, when known). */
  trigger(state: OneShot, now: number, ms: number = ONE_SHOT_MS[state]): boolean {
    const cur = this.shot && now < this.shot.until ? this.shot : null;
    if (cur && RANK[cur.state] > RANK[state]) return false;
    this.shot = { state, until: now + ms };
    this.shotId++;
    return true;
  }

  /** Moves to this moment's looping state. Coming down from the air lands;
   *  dying ends whatever was going on. */
  update(now: number, p: { alive: boolean; stance?: Stance | null; speed?: number | null }): AnimNow {
    const next = locomotion(p);
    if (this.loop === "airborne" && next !== "airborne" && next !== "die") this.trigger("land", now);
    if (next === "die") this.shot = null;
    this.loop = next;
    if (this.shot && now >= this.shot.until) this.shot = null;
    return { loop: next, shot: this.shot?.state ?? null, shotId: this.shotId };
  }
}

/** Speed for a sender that doesn't give one: from how far the eased
 *  position moved, itself eased so one long frame doesn't read as a sprint.
 *  It keeps the eased velocity too, for which way the legs go. */
export class SpeedMeter {
  private x = NaN; private y = NaN;
  speed = 0;
  vx = 0; vy = 0;
  update(x: number, y: number, dtMs: number): number {
    if (Number.isFinite(this.x) && dtMs > 0) {
      const s = dtMs / 1000, k = Math.min(1, dtMs / 150);
      const vx = (x - this.x) / s, vy = (y - this.y) / s;
      // A jump of more than a sprint is a respawn or a seek, not running.
      if (Math.hypot(vx, vy) > 12) this.vx = this.vy = 0;
      else { this.vx += (vx - this.vx) * k; this.vy += (vy - this.vy) * k; }
      this.speed = Math.hypot(this.vx, this.vy);
    }
    this.x = x; this.y = y;
    return this.speed;
  }
  /** The heading of the movement (game yaw), or null when barely moving. */
  get heading(): number | null {
    return Math.hypot(this.vx, this.vy) >= WALK_SPEED ? Math.atan2(this.vy, this.vx) : null;
  }
}

// ---- aiming ----

/** An angle brought into -pi..pi. */
export function wrapAngle(a: number): number {
  return a - 2 * Math.PI * Math.round(a / (2 * Math.PI));
}

/** The yaw (game, from +x towards +y) and pitch (up positive) of an aim vector. */
export function aimAngles(aim: readonly [number, number, number]): { yaw: number; pitch: number } {
  const flat = Math.hypot(aim[0], aim[1]);
  return { yaw: Math.atan2(aim[1], aim[0]), pitch: Math.atan2(aim[2], flat) };
}

/** How far the legs may turn from where the body aims (radians). */
export const MAX_TWIST = 1.0;
/** Pitch past this is held (radians): the spine bends only so far. */
export const MAX_PITCH = 1.2;

/** How far the legs turn from the aim to follow the movement: towards its
 *  heading, or away from it when backing up or strafing back (so the legs
 *  never face more than a right angle off the aim), at most MAX_TWIST.
 *  Standing still they face the aim. */
export function legTwist(aimYaw: number, moveYaw: number | null): number {
  if (moveYaw === null) return 0;
  let d = wrapAngle(moveYaw - aimYaw);
  if (Math.abs(d) > Math.PI / 2) d = wrapAngle(d + Math.PI);
  return Math.max(-MAX_TWIST, Math.min(MAX_TWIST, d));
}

/** Which bones carry the aim, and each one's share of the pitch and of the
 *  twist back from the legs: the two spine bones, so the arms (and the gun
 *  in them) and the head above all point along it. A rig with only one of
 *  them gives it the whole turn. */
export const AIM_SHARES: ReadonlyArray<[RegExp, number, number]> = [
  [/^(joint_)?spine$/i, 0.5, 0.5],
  [/^(joint_)?spine1$/i, 0.5, 0.5],
];

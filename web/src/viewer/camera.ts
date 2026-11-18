// Framing for the 3D viewer, in game coordinates (z up). Pure.

import type { Vec3 } from "../map/views";

export interface Box3 { min: Vec3; max: Vec3 }

export interface CameraFit {
  centre: Vec3;
  /** Half the box's diagonal. */
  radius: number;
  /** From the centre to the camera for the opening three-quarter view. */
  offset: Vec3;
  /** How far above the centre a straight-down camera sits to see it all. */
  topDistance: number;
}

const rad = (deg: number) => (deg * Math.PI) / 180;

/** Where to put a perspective camera (vertical `fovDeg`, `aspect` =
 *  width / height) to see the whole box. */
export function cameraFit(b: Box3, fovDeg: number, aspect: number): CameraFit {
  const size: Vec3 = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
  const centre: Vec3 = [(b.min[0] + b.max[0]) / 2, (b.min[1] + b.max[1]) / 2, (b.min[2] + b.max[2]) / 2];
  const radius = Math.max(1, Math.hypot(size[0], size[1], size[2]) / 2);
  const v = rad(fovDeg) / 2, h = Math.atan(Math.tan(v) * Math.max(aspect, 0.05));
  // Maps are wide and low, so a sphere fit leaves a lot of air: come in a bit.
  const dist = (radius / Math.sin(Math.min(v, h))) * 0.68;
  // From the south-east, 35° up: the sun is behind the camera's left shoulder.
  const dir: Vec3 = [0.53, -0.62, 0.57];
  const len = Math.hypot(dir[0], dir[1], dir[2]);
  const topDistance = Math.max(size[1] / 2 / Math.tan(v), size[0] / 2 / Math.tan(h), 1) * 1.08 + size[2] / 2;
  return { centre, radius, offset: [dir[0] / len * dist, dir[1] / len * dist, dir[2] / len * dist], topDistance };
}

/** How far the follow camera comes in to its player, game units: about
 *  seven heights of a life-size model. */
export const FOLLOW_DISTANCE = 5;

/** How far from its player the follow camera sits when it starts: close
 *  to them whatever the size of the level (not a share of the map), no
 *  farther than the camera already was, and no nearer than twice the
 *  closest the controls allow. A capsule is drawn `scale` times life
 *  size, so it is followed from that much farther back. */
export function followDistance(current: number, scale: number, lifeSize: boolean, minDistance: number): number {
  const close = lifeSize ? FOLLOW_DISTANCE : FOLLOW_DISTANCE * Math.max(1, scale);
  return Math.max(Math.min(current, close), minDistance * 2);
}

/** How much bigger than life to draw players, so they can be found on a
 *  large map: life size up to a 45-unit map, then growing with it, to 4×. */
export function markerScale(b: Box3): number {
  const across = Math.hypot(b.max[0] - b.min[0], b.max[1] - b.min[1]);
  return Math.min(4, Math.max(1, across / 45));
}

/** A standing Spartan's eyes above their feet, game units, when the model's
 *  height isn't known; and the share of a model's height the eyes are at,
 *  standing and crouched. */
export const EYE_HEIGHT = 0.62;
export const EYE_SHARE = 0.9;
export const CROUCH_EYE_SHARE = 0.55;

/** The first-person camera for a player: where their eyes are and a point
 *  one unit along where they look (game coordinates). `height` is their
 *  model's, feet to head, or null. Their aim when the frame has one, else
 *  level along their yaw. */
export function eyeView(
  p: { x: number; y: number; z: number; yaw: number; aim?: readonly [number, number, number] | null; stance?: string | null },
  height: number | null,
): { eye: Vec3; look: Vec3 } {
  const crouched = p.stance === "crouch";
  const h = height && height > 0 ? height * (crouched ? CROUCH_EYE_SHARE : EYE_SHARE) : EYE_HEIGHT * (crouched ? CROUCH_EYE_SHARE / EYE_SHARE : 1);
  const eye: Vec3 = [p.x, p.y, p.z + h];
  const a = p.aim && p.aim.every(Number.isFinite) && Math.hypot(p.aim[0], p.aim[1], p.aim[2]) > 1e-6
    ? p.aim
    : [Math.cos(p.yaw), Math.sin(p.yaw), 0];
  const len = Math.hypot(a[0], a[1], a[2]);
  return { eye, look: [eye[0] + a[0] / len, eye[1] + a[1] / len, eye[2] + a[2] / len] };
}

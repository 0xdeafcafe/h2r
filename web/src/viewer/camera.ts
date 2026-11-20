// Framing for the 3D viewer, in game coordinates (z up). Pure.

import type { Vec3 } from "../map/views";

export type DirectorMode = "eyes" | "follow" | "orbit" | "free" | "top-down";

export interface CameraSelection { mode: DirectorMode; follow: number | null }
export interface ViewerDirector {
  selection(): CameraSelection;
  pick(mode: DirectorMode, follow: number | null): void;
}

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

const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

export function fitBox(box: Box3, dir: Vec3, fovDeg: number, aspect: number, margin = 0.10): number {
  const length = Math.hypot(...dir);
  const d: Vec3 = [dir[0] / length, dir[1] / length, dir[2] / length];
  const across = Math.hypot(d[0], d[1]);
  const r: Vec3 = across ? [-d[1] / across, d[0] / across, 0] : [1, 0, 0];
  const u: Vec3 = [-d[2] * r[1], d[2] * r[0], d[0] * r[1] - d[1] * r[0]];
  const v = Math.tan(rad(fovDeg) / 2) * (1 - margin), h = v * aspect;
  const half = box.max.map((n, i) => (n - box.min[i]!) / 2);
  let distance = 0;
  for (const x of [-half[0]!, half[0]!]) for (const y of [-half[1]!, half[1]!]) for (const z of [-half[2]!, half[2]!]) {
    const c: Vec3 = [x, y, z];
    distance = Math.max(distance, dot(c, d) + Math.max(Math.abs(dot(c, r)) / h, Math.abs(dot(c, u)) / v));
  }
  return distance;
}

export function framingPitch(size: Vec3): number {
  return Math.min(44, Math.max(26, 35 + (0.25 - size[2] / Math.max(size[0], size[1], 1e-6)) * 40));
}

export function orbitDirection(pitch: number, clockwise = 0): Vec3 {
  const azimuth = Math.atan2(-0.62, 0.53) - rad(clockwise);
  const p = rad(pitch);
  return [Math.cos(azimuth) * Math.cos(p), Math.sin(azimuth) * Math.cos(p), Math.sin(p)];
}

/** Fit the eight corners in the camera's frustum. */
export function cameraFit(b: Box3, fovDeg: number, aspect: number): CameraFit {
  const size: Vec3 = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
  const centre: Vec3 = [(b.min[0] + b.max[0]) / 2, (b.min[1] + b.max[1]) / 2, (b.min[2] + b.max[2]) / 2];
  const radius = Math.max(1, Math.hypot(...size) / 2);
  const dir = orbitDirection(framingPitch(size));
  const dist = fitBox(b, dir, fovDeg, aspect);
  const topDistance = fitBox(b, [0, 0, 1], fovDeg, aspect);
  return { centre, radius, offset: [dir[0] * dist, dir[1] * dist, dir[2] * dist], topDistance };
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

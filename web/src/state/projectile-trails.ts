import type { Frame, FrameProjectile } from "../api/types";
import { findFrame } from "./frames";

type Point = FrameProjectile["pos"];

export interface ProjectileTrail {
  /** Full salted object ID from the telemetry frame. */
  id: string;
  kind: string;
  points: Point[];
}

export const MAX_TRAIL_POINTS = 64;
const MAX_FRAME_PROJECTILES = 64;

const projectiles = new WeakMap<FrameProjectile[], Map<string, FrameProjectile>>();
const noProjectiles: ReadonlyMap<string, FrameProjectile> = new Map();

function projectilesIn(frame: Frame | undefined): ReadonlyMap<string, FrameProjectile> {
  const list = frame?.projectiles;
  if (frame?.cinematic || frame?.film_world || !Array.isArray(list)) return noProjectiles;
  const cached = projectiles.get(list);
  if (cached) return cached;
  const byId = new Map<string, FrameProjectile>();
  for (const p of list.slice(0, MAX_FRAME_PROJECTILES)) {
    if (!p || typeof p.kind !== "string" || !p.kind) continue;
    if (typeof p.id !== "string" || !p.id || !Array.isArray(p.pos) || p.pos.length !== 3 || !p.pos.every(Number.isFinite)) continue;
    byId.set(p.id, p);
  }
  projectiles.set(list, byId);
  return byId;
}

function continues(a: FrameProjectile, b: FrameProjectile | undefined): b is FrameProjectile {
  return !!b && a.kind === b.kind && (a.parent ?? null) === (b.parent ?? null);
}

/** Advance along a single observed segment. A missing projectile or
 *  attachment change holds the last observation until the next frame. */
function onSegment(a: FrameProjectile, b: FrameProjectile | undefined, mix: number): Point {
  if (mix <= 0 || !continues(a, b)) return a.pos;
  const [x, y, z] = a.pos;
  return [x + (b.pos[0] - x) * mix, y + (b.pos[1] - y) * mix, z + (b.pos[2] - z) * mix];
}

function samePoint(a: Point | undefined, b: Point): boolean {
  return !!a && a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

/** Sample the trail from the current projectile back through consecutive
 *  observations. The window starts over at a seek, cut or attachment
 *  change, and the current census decides which trails still exist. */
export function sampleProjectileTrails(
  frames: readonly Frame[],
  tick: number,
  since = -Infinity,
  cuts: readonly number[] = [],
): ProjectileTrail[] {
  const i = findFrame(frames, tick);
  const current = frames[i];
  const active = projectilesIn(current);
  if (!current || !active.size) return [];

  let start = Math.min(tick, since), nextCut = Infinity;
  for (const cut of cuts) {
    if (cut > tick) { nextCut = cut; break; }
    if (cut > current.tick) return []; // The first frame of the new run has yet to arrive.
    start = Math.max(start, cut);
  }
  const next = frames[i + 1];
  const blend = next && next.cut == null && next.tick > current.tick && next.tick < nextCut;
  const following = blend ? projectilesIn(next) : noProjectiles;
  const mix = blend ? (tick - current.tick) / (next.tick - current.tick) : 0;
  const out: ProjectileTrail[] = [];

  for (const [id, projectile] of active) {
    const points: Point[] = [];
    const after = following.get(id);
    const position = onSegment(projectile, after, mix);
    let newer = projectile;
    let newerFrame = next;
    for (let j = i; j >= 0 && points.length < MAX_TRAIL_POINTS - 1; j--) {
      const frame = frames[j]!;
      const p = projectilesIn(frame).get(id);
      if (!continues(newer, p)) break;
      if (frame.tick < start) {
        if (newerFrame && newerFrame.tick > start) {
          const elapsed = (start - frame.tick) / (newerFrame.tick - frame.tick);
          points.push(onSegment(p, j === i ? after : newer, elapsed));
        }
        break;
      }
      points.push(p.pos);
      if (frame.cut != null) break;
      newer = p;
      newerFrame = frame;
    }
    points.reverse();
    if (!samePoint(points.at(-1), position)) points.push(position);
    out.push({ id, kind: projectile.kind, points });
  }
  return out;
}

/** Use the observed projectile's tag stem, regardless of its owner's weapon. */
export function projectileColour(kind: string): string {
  if (kind === "frag_grenade") return "#d8e86a";
  if (kind === "plasma_grenade") return "#6fd0ff";
  if (kind.includes("rocket")) return "#ff8a2a";
  if (kind.includes("needle")) return "#ff4fd0";
  if (kind.includes("plasma")) return "#4f9cff";
  if (kind.includes("grenade")) return "#ff7a26";
  return "#e5dfd7";
}

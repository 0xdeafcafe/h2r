// The opening shot and the playable area it frames. No renderer dependency.
import type { Bounds } from "../map/render";
import type { Vec3 } from "../map/views";
import type { GamePlayerState } from "../state/reducer";
import type { Box3 } from "./camera";

export type CameraStart = { kind: "player"; index: number } | { kind: "bounds" };

type Player = Pick<GamePlayerState, "index" | "xuid" | "name" | "game_name">;

export function addressPlayer(players: readonly Player[], player?: string): number | null {
  const key = player?.trim().toLowerCase();
  if (!key) return null;
  let index: number | null = null;
  for (const p of players) {
    if ([p.xuid, p.name, p.game_name].some(name => name?.trim().toLowerCase() === key)) {
      if (index === null || p.index < index) index = p.index;
    }
  }
  return index;
}

export function startCamera(
  players: readonly Player[],
  present: readonly { index: number }[],
  player?: string,
  xuid?: string | null,
  focus?: number | null,
): CameraStart {
  const signedIn = xuid ? players.find(p => p.xuid === xuid && present.some(s => s.index === p.index))?.index : null;
  for (const index of [addressPlayer(players, player), signedIn, focus]) {
    if (index != null && present.some(p => p.index === index)) return { kind: "player", index };
  }
  return { kind: "bounds" };
}

const PLAYABLE_KINDS = new Set(["netgame", "weap", "eqip", "vehi", "ctrl"]);

export function playableBounds(
  level: Box3,
  spots: Iterable<{ kind: string; at: Vec3 }>,
  seen: Bounds | null,
  features?: Iterable<{ x: number; y: number; z: number }>,
): Box3 {
  const points: Vec3[] = [];
  for (const s of spots) if (PLAYABLE_KINDS.has(s.kind)) points.push(s.at);
  if (features) for (const f of features) points.push([f.x, f.y, f.z]);
  if (seen && ![seen.minX, seen.maxX, seen.minY, seen.maxY].every(Number.isFinite)) seen = null;
  if (!seen && points.length < 6) return level;
  if (seen) {
    const z = points.length ? Math.min(...points.map(p => p[2])) : level.min[2];
    for (const x of [seen.minX, seen.maxX]) for (const y of [seen.minY, seen.maxY]) points.push([x, y, z]);
  }
  const min: Vec3 = [Infinity, Infinity, Infinity], max: Vec3 = [-Infinity, -Infinity, -Infinity];
  for (const p of points) for (const axis of [0, 1, 2] as const) {
    min[axis] = Math.min(min[axis], p[axis]);
    max[axis] = Math.max(max[axis], p[axis]);
  }
  const pad = Math.max(4, 0.12 * Math.max(max[0] - min[0], max[1] - min[1]));
  for (const axis of [0, 1, 2] as const) {
    min[axis] = Math.max(level.min[axis], min[axis] - (axis === 2 ? 1 : pad));
    max[axis] = Math.min(level.max[axis], max[axis] + (axis === 2 ? 3 : pad));
    if (min[axis] > max[axis]) return level;
    if (max[axis] - min[axis] < 6) {
      const middle = (min[axis] + max[axis]) / 2;
      min[axis] = middle - 3;
      max[axis] = middle + 3;
    }
  }
  const floor = (b: Box3) => (b.max[0] - b.min[0]) * (b.max[1] - b.min[1]);
  return floor({ min, max }) > 0.85 * floor(level) ? level : { min, max };
}

export const easeOutExpo = (t: number) => t === 1 ? 1 : 1 - 2 ** (-10 * t);

import type { FilmEffect, FilmVector, Frame, FrameActor, FrameHit, FramePlayer } from "./types";

type Data = Record<string, unknown>;
interface SourceEffect { kind: string; data: Data; age: number }
interface ParentSpace { objectPoint(parent: number, pos: FilmVector): FilmVector | null }

const kinds = new Set(["Damage", "WeaponFire", "Pickup", "Impact", "ProjectileEnded"]);
export const isFilmEffect = (kind: string) => kinds.has(kind);
const vector = (v: unknown): v is FilmVector => Array.isArray(v) && v.length === 3 && v.every(Number.isFinite);
const obj = (v: unknown): v is Data => v !== null && typeof v === "object" && !Array.isArray(v);
const text = (v: unknown) => typeof v === "string" ? v : null;
const radial = new Set([4, 15, 16, 17, 21, 22, 25]);

function point(raw: unknown, parent: unknown, poses?: ParentSpace) {
  if (!vector(raw)) return null;
  if (parent == null) return raw;
  if (typeof parent !== "number" || !Number.isInteger(parent)) return null;
  return poses?.objectPoint(parent, raw) ?? null;
}

/** Apply observations to the frame that delivered them. The guest timestamp
 * gives the visual's exact position on the playback clock even for a batch. */
export function applyFilmEffects(f: Frame, effects: readonly SourceEffect[], poses?: ParentSpace) {
  const units = new Map<string, FramePlayer | FrameActor>();
  f.players.forEach(p => units.set("p" + p.index, p));
  f.actors?.forEach(a => units.set(a.id, a));
  const bursts = new Map<string, { count: number; mask: number }>();
  const pickups = new Map<string, number>();
  const visuals: FilmEffect[] = [];
  const blastOrigins = new Set<string>();
  for (const {kind, data: d, age} of effects) {
    const tick = f.tick - age;
    if (kind === "Damage" && vector(d.pos)) {
      const hit: FrameHit = {pos: d.pos, from: text(d.from), to: text(d.to), kind: text(d.kind),
        weapon: text(d.weapon), amount: typeof d.amount === "number" && Number.isFinite(d.amount) ? d.amount : null, ago: age};
      (f.hits ??= []).push(hit);
    } else if (kind === "Pickup" && typeof d.unit === "string") {
      pickups.set(d.unit, age);
    } else if (kind === "WeaponFire" && typeof d.unit === "string" && typeof d.count === "number" && Number.isInteger(d.count) && d.count > 0) {
      const burst = bursts.get(d.unit) ?? {count: 0, mask: 0};
      burst.count += d.count;
      if (age <= 31 && age >= 0) burst.mask = (burst.mask | (1 << age)) >>> 0;
      bursts.set(d.unit, burst);
      if (obj(d.barrel)) {
        const pos = point(d.barrel.origin, d.barrel.parent, poses);
        if (pos) visuals.push({kind: "barrel", tick, pos, unit: d.unit,
          direction: vector(d.barrel.aim) ? d.barrel.aim : undefined});
      }
    } else if (kind === "Impact" && vector(d.origin)) {
      const blast = typeof d.code === "number" && radial.has(d.code & 63);
      if (blast) {
        const key = [tick, d.source, d.effect, ...d.origin].join("/");
        if (blastOrigins.has(key)) continue;
        blastOrigins.add(key);
      }
      visuals.push({kind: blast ? "blast" : "impact", tick, pos: d.origin,
        direction: vector(d.direction) ? d.direction : undefined});
    } else if (kind === "ProjectileEnded") {
      const pos = point(d.position, d.parent, poses);
      if (pos) visuals.push({kind: "terminal", tick, pos, direction: vector(d.velocity) ? d.velocity : undefined});
    }
  }
  if (visuals.length) f.film_effects = visuals;
  for (const [id, unit] of units) {
    const burst = bursts.get(id), pickup = pickups.get(id);
    if (burst) { unit.shots = burst.count; if (burst.mask) unit.fire = burst.mask; }
    if (pickup !== undefined && !unit.actions?.includes("pickup")) {
      const old = unit.actions ?? [];
      unit.actions = [...old, "pickup"];
      unit.actions_ago = [...(unit.actions_ago ?? old.map(() => 0)), pickup];
    }
  }
}

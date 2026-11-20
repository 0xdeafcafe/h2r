import type { FilmProp, FilmWorld, Frame } from "../api/types";
import { findFrame, nlerpAim, SNAP_DISTANCE } from "../state/frames";

const byDatum = new WeakMap<FilmWorld, Map<string, FilmProp>>();
function props(world: FilmWorld) {
  let m = byDatum.get(world);
  if (!m) { m = new Map(world.props.map(p => [p.id, p])); byDatum.set(world, m); }
  return m;
}

/** Creation, removal, pickups and hill changes take effect on the recorded tick.
 * Interpolate only the same salted object across two adjacent observations. */
export function sampleFilmWorld(frames: readonly Frame[], tick: number): FilmWorld | undefined {
  const i = findFrame(frames, tick), a = frames[i], b = frames[i + 1];
  const old = a?.film_world, next = b?.film_world;
  if (!old || !a || !next || !b || a.cinematic || b.cinematic || b.cut != null || tick <= a.tick || b.tick <= a.tick) return old;
  const t = (tick - a.tick) / (b.tick - a.tick);
  const now = props(next);
  return { hill: old.hill, props: old.props.map(p => {
    const q = now.get(p.id);
    if (q === p || !q || q.kind !== p.kind || q.class !== p.class) return p;
    const dx = q.pos[0] - p.pos[0], dy = q.pos[1] - p.pos[1], dz = q.pos[2] - p.pos[2];
    if (dx * dx + dy * dy + dz * dz > SNAP_DISTANCE * SNAP_DISTANCE) return p;
    return { ...p, pos: [p.pos[0] + dx * t, p.pos[1] + dy * t, p.pos[2] + dz * t],
      forward: nlerpAim(p.forward, q.forward, t), up: nlerpAim(p.up, q.up, t) };
  }) };
}

/** Index exact event times once per replay window. Event delivery and display
 * may lie on opposite sides of a pose sample. */
export type VisibleFilmEffect = import("../api/types").FilmEffect & { age: number; id: number };
type IndexedEffect = Omit<VisibleFilmEffect, "age">;
const eventTimes = new WeakMap<readonly Frame[], IndexedEffect[]>();
export function sampleFilmEffects(frames: readonly Frame[], tick: number): VisibleFilmEffect[] {
  let times = eventTimes.get(frames);
  if (!times) {
    times = [];
    for (const f of frames) for (const e of f.film_effects ?? []) times.push({...e, id: times.length});
    times.sort((a, b) => a.tick - b.tick);
    eventTimes.set(frames, times);
  }
  const last = findFrame(times, tick);
  const out: VisibleFilmEffect[] = [];
  for (let i = last; i >= 0 && tick - times[i]!.tick < 12; i--) {
    const e = times[i]!, age = tick - e.tick;
    if (age < (e.kind === "blast" ? 12 : e.kind === "barrel" ? 5 : 4)) out.push({...e, age});
  }
  return out;
}

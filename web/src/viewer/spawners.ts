// Effects a level's scripts play (`asset.extras.spawners` in the map glb,
// docs/live/live-art.md "Scripted spawns"): where, which effect, and how
// often a looping script plays it. A mod's turret salvo every few seconds,
// say. The 3D view marks each with a burst that repeats at its period
// (level3d.ts); the effect itself (particles, projectiles) is not drawn.
// Pure: no three.js here.

export interface Spawner {
  script: string;
  effect: string;
  /** The cutscene flag or object it plays at. */
  at: string;
  /** Game coordinates. */
  pos: [number, number, number];
  looping: boolean;
  /** Seconds between plays, when the script's sleeps say. */
  period: number | null;
}

const MAX_SPAWNERS = 256;
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const text = (v: unknown) => (typeof v === "string" ? v.slice(0, 128) : "");

/** The spawners a glb's asset extras list, read defensively. */
export function spawnersOf(extras: unknown): Spawner[] {
  const list = (extras as { spawners?: unknown } | null)?.spawners;
  if (!Array.isArray(list)) return [];
  const out: Spawner[] = [];
  for (const v of list.slice(0, MAX_SPAWNERS)) {
    const e = (v ?? {}) as Record<string, unknown>;
    const p = e.pos;
    if (!Array.isArray(p) || p.length !== 3) continue;
    const [x, y, z] = p as unknown[];
    if (!isNum(x) || !isNum(y) || !isNum(z)) continue;
    out.push({
      script: text(e.script),
      effect: text(e.effect),
      at: text(e.at),
      pos: [x, y, z],
      looping: e.looping === true,
      period: isNum(e.period) && e.period > 0 ? e.period : null,
    });
  }
  return out;
}

/** How a spawner's mark pulses at `time` seconds: 1 as it plays, fading to
 *  0 over half a second (or its period, if shorter). A looping one with a
 *  period plays every period; one with no period (or not looping) glows
 *  steadily at a half. */
export function pulse(s: Pick<Spawner, "looping" | "period">, time: number): number {
  if (!s.looping || !s.period) return 0.5;
  const into = ((time % s.period) + s.period) % s.period;
  const fade = Math.min(0.5, s.period);
  return into >= fade ? 0 : 1 - into / fade;
}
